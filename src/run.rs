//! The Run engine: plans Trials and executes them on worker threads. No async runtime.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::dataset::Case;
use crate::evidence::{Hashed, TrialRecord};
use crate::grade::Target;
use crate::instructions::JudgeInstructions;
use crate::provider::{Candidate, agent, now_unix_ms, request_body, retryable, send};

// shortcut: fixed 1 s retry pause, no Retry-After; make it configurable if providers need backoff
const RETRY_PAUSE: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// Repetitions of every Candidate × instructions × Case combination.
    pub repetitions: u32,
    /// Trials in flight at once.
    pub concurrency: u32,
    pub timeout_secs: u64,
    /// Extra Attempts a Trial may make after a retryable failure.
    pub retries: u32,
    pub include_unscored: bool,
}

/// One planned Trial; the indexes point into the slices given to `plan` and `execute`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrialSpec {
    pub seq: usize,
    pub candidate: usize,
    pub instructions: usize,
    pub case: usize,
    /// Starts at 1.
    pub repetition: u32,
}

#[derive(Debug)]
pub struct Plan {
    pub trials: Vec<TrialSpec>,
    /// Cases planned.
    pub cases: usize,
    /// Cases left out because none of the selected Targets has a label.
    pub skipped: usize,
}

/// A Case is planned when any selected Target has a label for it, or when unscored Cases are kept.
pub fn eligible(case: &Case, targets: &[Target], include_unscored: bool) -> bool {
    include_unscored || targets.iter().any(|t| case.ok.get(*t).is_some())
}

/// Trials in a plan: every repetition of every Candidate × instructions × Case combination.
pub fn trial_count(
    candidates: usize,
    instructions: usize,
    cases: usize,
    repetitions: u32,
) -> usize {
    repetitions as usize * candidates * instructions * cases
}

/// The most requests `trials` can send: each may be retried `retries` times.
pub fn request_count(trials: usize, retries: u32) -> usize {
    trials * (1 + retries as usize)
}

/// Repetition-first: repetition 1 of every combination, then repetition 2, and so on, so a
/// cancelled Run covers as many combinations as possible. A Case is skipped when none of the
/// selected `targets` has a label, unless `include_unscored`.
pub fn plan(
    candidates: usize,
    instructions: usize,
    cases: &[Case],
    targets: &[Target],
    settings: &Settings,
) -> Plan {
    let planned: Vec<usize> = (0..cases.len())
        .filter(|&case| eligible(&cases[case], targets, settings.include_unscored))
        .collect();
    let mut trials = Vec::with_capacity(trial_count(
        candidates,
        instructions,
        planned.len(),
        settings.repetitions,
    ));
    for repetition in 1..=settings.repetitions {
        for candidate in 0..candidates {
            for instructions in 0..instructions {
                for &case in &planned {
                    trials.push(TrialSpec {
                        seq: trials.len(),
                        candidate,
                        instructions,
                        case,
                        repetition,
                    });
                }
            }
        }
    }
    Plan {
        trials,
        cases: planned.len(),
        skipped: cases.len() - planned.len(),
    }
}

/// What `execute` needs to turn a `TrialSpec` into a request; the slices are parallel to the ones
/// given to `plan` (`urls` and `keys` to `candidates`).
pub struct Job<'a> {
    pub candidates: &'a [Candidate],
    pub urls: &'a [String],
    pub keys: &'a [Option<String>],
    pub instructions: &'a [JudgeInstructions],
    pub cases: &'a [Case],
    pub targets: &'a [Target],
}

fn run_trial(spec: &TrialSpec, job: &Job, settings: &Settings, agent: &ureq::Agent) -> TrialRecord {
    let candidate = &job.candidates[spec.candidate];
    let instructions = &job.instructions[spec.instructions];
    let case = &job.cases[spec.case];
    let request = request_body(&candidate.model, instructions, case, job.targets);
    let started_unix_ms = now_unix_ms();
    let clock = Instant::now();
    let mut attempts = Vec::new();
    loop {
        let attempt = send(
            agent,
            &job.urls[spec.candidate],
            job.keys[spec.candidate].as_deref(),
            &request,
        );
        let retry = retryable(&attempt) && attempts.len() < settings.retries as usize;
        attempts.push(attempt);
        if !retry {
            break;
        }
        thread::sleep(RETRY_PAUSE);
    }
    TrialRecord {
        seq: spec.seq,
        candidate: candidate.name.clone(),
        instructions: Hashed {
            name: instructions.name.clone(),
            sha256: instructions.sha256.clone(),
        },
        case_id: case.id.clone(),
        targets: job.targets.to_vec(),
        repetition: spec.repetition,
        labels: case.ok.clone(),
        request,
        attempts,
        started_unix_ms,
        elapsed_ms: clock.elapsed().as_millis() as u64,
    }
}

/// Runs the planned Trials on `settings.concurrency` workers, sending each finished record over
/// `tx`, and returns when every worker is done. A worker checks `cancel` before taking a new
/// Trial; a Trial already started finishes, retries included. `started` counts the Trials taken.
pub fn execute(
    job: &Job,
    trials: &[TrialSpec],
    settings: &Settings,
    cancel: &AtomicBool,
    started: &AtomicUsize,
    tx: Sender<TrialRecord>,
) {
    let agent = agent(Duration::from_secs(settings.timeout_secs));
    let next = AtomicUsize::new(0);
    let workers = (settings.concurrency as usize).min(trials.len());
    thread::scope(|scope| {
        for _ in 0..workers {
            let tx = tx.clone();
            let (agent, next) = (&agent, &next);
            scope.spawn(move || {
                while !cancel.load(Ordering::Relaxed) {
                    let Some(spec) = trials.get(next.fetch_add(1, Ordering::Relaxed)) else {
                        break;
                    };
                    started.fetch_add(1, Ordering::Relaxed);
                    if tx.send(run_trial(spec, job, settings, agent)).is_err() {
                        break;
                    }
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dataset::parse_cases;

    fn cases() -> Vec<Case> {
        parse_cases(
            br#"[{"id":"a","message":"1","split":"train","ok":{"sonnet":"low","opus":"low"}},
                 {"id":"b","message":"2","split":"train","ok":{"sonnet":"low"}}]"#,
        )
        .unwrap()
    }

    fn settings(repetitions: u32, include_unscored: bool) -> Settings {
        Settings {
            repetitions,
            concurrency: 1,
            timeout_secs: 60,
            retries: 1,
            include_unscored,
        }
    }

    fn key(t: &TrialSpec) -> (u32, usize, usize, usize) {
        (t.repetition, t.candidate, t.instructions, t.case)
    }

    #[test]
    fn plans_repetition_first_and_skips_cases_no_selected_target_labels() {
        let both = [Target::Sonnet, Target::Opus];
        let plan = plan(2, 1, &cases(), &both, &settings(2, false));
        assert_eq!((plan.cases, plan.skipped), (2, 0));
        assert_eq!(plan.trials.len(), 2 * 2 * 2);
        assert!(plan.trials.iter().enumerate().all(|(i, t)| t.seq == i));
        let keys: Vec<_> = plan.trials.iter().map(key).collect();
        // Repetition 1 of every candidate and case, then repetition 2.
        assert_eq!(
            keys[..4],
            [(1, 0, 0, 0), (1, 0, 0, 1), (1, 1, 0, 0), (1, 1, 0, 1)]
        );
        assert_eq!(keys[4], (2, 0, 0, 0));
    }

    #[test]
    fn a_case_is_kept_when_any_selected_target_is_labelled() {
        // Case b is labelled for sonnet only.
        let plan = plan(
            1,
            1,
            &cases(),
            &[Target::Opus, Target::Sonnet],
            &settings(1, false),
        );
        assert_eq!((plan.cases, plan.skipped, plan.trials.len()), (2, 0, 2));
        // With only opus selected, b has nothing to score.
        let plan = plan_for(&[Target::Opus], false);
        assert_eq!((plan.cases, plan.skipped, plan.trials.len()), (1, 1, 1));
        assert_eq!(plan.trials[0].case, 0);
        // A target nobody labelled drops every case.
        let plan = plan_for(&[Target::Haiku, Target::Fable], false);
        assert_eq!((plan.cases, plan.skipped, plan.trials.len()), (0, 2, 0));
    }

    fn plan_for(targets: &[Target], include_unscored: bool) -> Plan {
        plan(1, 1, &cases(), targets, &settings(1, include_unscored))
    }

    #[test]
    fn include_unscored_keeps_every_case() {
        let plan = plan(1, 2, &cases(), &[Target::Opus], &settings(1, true));
        assert_eq!((plan.cases, plan.skipped, plan.trials.len()), (2, 0, 4));
    }

    #[test]
    fn the_counts_match_what_plan_builds() {
        let both = [Target::Sonnet, Target::Opus];
        let cases = cases();
        for settings in [settings(3, false), settings(1, true)] {
            let built = plan(2, 3, &cases, &both, &settings);
            let eligible = cases
                .iter()
                .filter(|c| eligible(c, &both, settings.include_unscored))
                .count();
            assert_eq!(eligible, built.cases);
            let trials = trial_count(2, 3, eligible, settings.repetitions);
            assert_eq!(trials, built.trials.len());
            // One retry means each Trial may send two requests.
            assert_eq!(request_count(trials, 1), trials * 2);
        }
        assert!(!eligible(&cases[1], &[Target::Opus], false));
    }

    #[test]
    fn only_unlabelled_cases_plan_nothing() {
        let plan = plan(1, 1, &cases()[1..], &[Target::Opus], &settings(3, false));
        assert!(plan.trials.is_empty());
        assert_eq!((plan.cases, plan.skipped), (0, 1));
    }
}
