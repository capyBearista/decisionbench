//! The Run engine: plans Trials and executes them on worker threads. No async runtime.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::dataset::{Case, EffortRange};
use crate::evidence::{Hashed, TrialRecord};
use crate::grade::Target;
use crate::instructions::JudgeInstructions;
use crate::provider::{Candidate, agent, now_unix_ms, request_body, retryable, send};

// shortcut: fixed 1 s retry pause, no Retry-After; make it configurable if providers need backoff
const RETRY_PAUSE: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// Repetitions of every Candidate × instructions × Case × Target combination.
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
    pub target: Target,
    /// Starts at 1.
    pub repetition: u32,
}

#[derive(Debug)]
pub struct Plan {
    pub trials: Vec<TrialSpec>,
    /// Case × Target pairs planned.
    pub pairs: usize,
    /// Unlabelled pairs left out.
    pub skipped: usize,
}

pub fn label(case: &Case, target: Target) -> Option<EffortRange> {
    match target {
        Target::Sonnet => case.ok.sonnet,
        Target::Opus => case.ok.opus,
    }
}

/// Repetition-first: repetition 1 of every combination, then repetition 2, and so on, so a
/// cancelled Run covers as many combinations as possible. Unlabelled pairs are skipped unless
/// `include_unscored`.
pub fn plan(
    candidates: usize,
    instructions: usize,
    cases: &[Case],
    targets: &[Target],
    settings: &Settings,
) -> Plan {
    let all_pairs = cases.len() * targets.len();
    let pairs: Vec<(usize, Target)> = (0..cases.len())
        .flat_map(|case| targets.iter().map(move |t| (case, *t)))
        .filter(|(case, target)| {
            settings.include_unscored || label(&cases[*case], *target).is_some()
        })
        .collect();
    let mut trials = Vec::new();
    for repetition in 1..=settings.repetitions {
        for candidate in 0..candidates {
            for instructions in 0..instructions {
                for &(case, target) in &pairs {
                    trials.push(TrialSpec {
                        seq: trials.len(),
                        candidate,
                        instructions,
                        case,
                        target,
                        repetition,
                    });
                }
            }
        }
    }
    Plan {
        trials,
        pairs: pairs.len(),
        skipped: all_pairs - pairs.len(),
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
}

fn run_trial(spec: &TrialSpec, job: &Job, settings: &Settings, agent: &ureq::Agent) -> TrialRecord {
    let candidate = &job.candidates[spec.candidate];
    let instructions = &job.instructions[spec.instructions];
    let case = &job.cases[spec.case];
    let request = request_body(&candidate.model, instructions, case, spec.target);
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
        target: spec.target,
        repetition: spec.repetition,
        label: label(case, spec.target),
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
            br#"[{"id":"a","message":"1","ok":{"sonnet":"low","opus":"low"}},
                 {"id":"b","message":"2","ok":{"sonnet":"low"}}]"#,
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

    fn key(t: &TrialSpec) -> (u32, usize, usize, usize, Target) {
        (t.repetition, t.candidate, t.instructions, t.case, t.target)
    }

    #[test]
    fn plans_repetition_first_and_skips_unlabelled_pairs() {
        let both = [Target::Sonnet, Target::Opus];
        let plan = plan(2, 1, &cases(), &both, &settings(2, false));
        assert_eq!((plan.pairs, plan.skipped), (3, 1));
        assert_eq!(plan.trials.len(), 2 * 2 * 3);
        assert!(plan.trials.iter().enumerate().all(|(i, t)| t.seq == i));
        let keys: Vec<_> = plan.trials.iter().map(key).collect();
        // Repetition 1 of every candidate and pair, then repetition 2.
        assert_eq!(keys[0], (1, 0, 0, 0, Target::Sonnet));
        assert_eq!(keys[1], (1, 0, 0, 0, Target::Opus));
        assert_eq!(keys[2], (1, 0, 0, 1, Target::Sonnet));
        assert_eq!(keys[3], (1, 1, 0, 0, Target::Sonnet));
        assert_eq!(keys[6], (2, 0, 0, 0, Target::Sonnet));
        assert!(!keys.contains(&(1, 0, 0, 1, Target::Opus)));
    }

    #[test]
    fn include_unscored_keeps_every_pair() {
        let both = [Target::Sonnet, Target::Opus];
        let plan = plan(1, 2, &cases(), &both, &settings(1, true));
        assert_eq!((plan.pairs, plan.skipped, plan.trials.len()), (4, 0, 8));
        assert!(
            plan.trials
                .iter()
                .any(|t| t.case == 1 && t.target == Target::Opus)
        );
    }

    #[test]
    fn only_unlabelled_pairs_plan_nothing() {
        let plan = plan(1, 1, &cases()[1..], &[Target::Opus], &settings(3, false));
        assert!(plan.trials.is_empty());
        assert_eq!((plan.pairs, plan.skipped), (0, 1));
    }
}
