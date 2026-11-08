//! The tuning scorer: per-model hit rate, the hit rate of the best fixed answer (the floor) and
//! their difference (the lift), over the Cases of one saved Run (ADR-0007).
//!
//! This is the only place a number meant to be optimised is computed, and `show` and `compare` never
//! print it (SPEC §6). It refuses to read holdout results unless asked, and refuses a Run whose
//! instructions quote a whole Case message. That is a tripwire for copying, not proof against
//! memorisation: a fragment or a paraphrase passes, and only the holdout shows overfitting.

use std::collections::{BTreeMap, HashMap};

use serde::Serialize;

use crate::dataset::{Case, Effort, Split};
use crate::evidence::{RunFile, RunState, TrialRecord};
use crate::grade::{Grade, Target};

/// Messages shorter than this are too generic to count as a quote.
const MIN_QUOTE_CHARS: usize = 20;

#[derive(Debug, Serialize)]
pub struct Score {
    pub run: String,
    /// `partial` or `failed` means fewer Trials than planned, which `coverage` does not show.
    pub state: RunState,
    pub planned_trials: usize,
    pub recorded_trials: usize,
    pub dataset_sha256: String,
    pub scores: Vec<VariantScore>,
}

/// One Candidate × Judge Instructions of the Run.
#[derive(Debug, Serialize)]
pub struct VariantScore {
    pub candidate: String,
    pub instructions: String,
    pub instructions_sha256: String,
    pub pooled: Rates,
    pub targets: BTreeMap<Target, Rates>,
}

/// Counts and rates over labelled predictions. Invalid answers are misses in the denominator;
/// Trials without a valid reply (`failed`) are outside it and lower `coverage` instead.
#[derive(Debug, Default, Serialize)]
pub struct Rates {
    pub hit: usize,
    pub under: usize,
    pub over: usize,
    pub invalid: usize,
    pub failed: usize,
    /// Hits of the best single effort answered for every row, summed over targets when pooled.
    #[serde(skip)]
    best_constant_hits: usize,
    pub coverage: Option<f64>,
    pub hit_rate: Option<f64>,
    pub floor: Option<f64>,
    pub lift: Option<f64>,
}

impl Rates {
    fn rows(&self) -> usize {
        self.hit + self.under + self.over + self.invalid
    }

    fn finish(&mut self) {
        let (rows, all) = (self.rows(), self.rows() + self.failed);
        let ratio = |n: usize, d: usize| (d > 0).then(|| n as f64 / d as f64);
        self.coverage = ratio(rows, all);
        self.hit_rate = ratio(self.hit, rows);
        self.floor = ratio(self.best_constant_hits, rows);
        self.lift = self.hit_rate.zip(self.floor).map(|(h, f)| h - f);
    }
}

fn normalized(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Scores `run` against `cases`, whose SHA-256 must be the revision the Run recorded.
pub fn score(
    run: &RunFile,
    dataset_sha256: &str,
    cases: &[Case],
    allow_holdout: bool,
) -> Result<Score, String> {
    if run.manifest.dataset.sha256 != dataset_sha256 {
        return Err(format!(
            "the run used dataset revision {}, not {dataset_sha256}; pass the dataset it used with --dataset",
            run.manifest.dataset.sha256
        ));
    }
    let by_id: HashMap<&str, &Case> = cases.iter().map(|c| (c.id.as_str(), c)).collect();
    // The selection, not the recorded Trials: a run stopped before its holdout Trials still selected them.
    for id in &run.manifest.cases {
        let case = by_id
            .get(id.as_str())
            .ok_or_else(|| format!("the run selects case {id}, not in the dataset"))?;
        if case.split == Split::Holdout && !allow_holdout {
            return Err(format!(
                "the run includes holdout case {id}; pass --allow-holdout to score it (once, at the end of tuning)"
            ));
        }
    }
    for trial in &run.trials {
        if !run.manifest.cases.contains(&trial.case_id) {
            return Err(format!(
                "the run has a trial for case {}, which the run did not select",
                trial.case_id
            ));
        }
        if let Some(t) = trial
            .targets
            .iter()
            .find(|t| !run.manifest.targets.contains(t))
        {
            return Err(format!(
                "trial {} asks about target {t}, which the run did not select",
                trial.seq
            ));
        }
    }
    for set in &run.manifest.instructions {
        let text = normalized(&set.text);
        let quoted = cases.iter().find(|c| {
            let message = normalized(&c.message);
            message.chars().count() >= MIN_QUOTE_CHARS && text.contains(&message)
        });
        if let Some(case) = quoted {
            return Err(format!(
                "instructions `{}` quotes the message of case {}, so this run measures memorisation",
                set.name, case.id
            ));
        }
    }

    // Every selected variant gets a score, even one the run stopped before it started.
    let mut groups: BTreeMap<_, Vec<&TrialRecord>> = BTreeMap::new();
    for candidate in &run.manifest.candidates {
        for set in &run.manifest.instructions {
            groups.insert(
                (
                    candidate.candidate.name.as_str(),
                    set.name.as_str(),
                    set.sha256.as_str(),
                ),
                Vec::new(),
            );
        }
    }
    for trial in &run.trials {
        let key = (
            trial.candidate.as_str(),
            trial.instructions.name.as_str(),
            trial.instructions.sha256.as_str(),
        );
        groups
            .get_mut(&key)
            .ok_or_else(|| {
                format!(
                    "trial {} is for candidate {} with instructions {}, which the run did not select",
                    trial.seq, key.0, key.1
                )
            })?
            .push(trial);
    }
    let scores = groups
        .into_iter()
        .map(|((candidate, name, sha), trials)| VariantScore {
            candidate: candidate.to_string(),
            instructions: name.to_string(),
            instructions_sha256: sha.to_string(),
            pooled: pooled(&trials),
            targets: run
                .manifest
                .targets
                .iter()
                .map(|t| (*t, rates(&trials, *t)))
                .collect(),
        })
        .collect();
    Ok(Score {
        run: run.manifest.id.clone(),
        state: run.end.as_ref().map_or(RunState::Partial, |e| e.state),
        planned_trials: run.manifest.planned_trials,
        recorded_trials: run.trials.len(),
        dataset_sha256: dataset_sha256.to_string(),
        scores,
    })
}

/// All targets together. The floor lets each target have its own best constant answer, then divides
/// their hits by all rows.
fn pooled(trials: &[&TrialRecord]) -> Rates {
    let mut total = Rates::default();
    for target in Target::ALL {
        let r = rates(trials, target);
        total.hit += r.hit;
        total.under += r.under;
        total.over += r.over;
        total.invalid += r.invalid;
        total.failed += r.failed;
        total.best_constant_hits += r.best_constant_hits;
    }
    total.finish();
    total
}

fn rates(trials: &[&TrialRecord], target: Target) -> Rates {
    let mut r = Rates::default();
    // Rows that count, as the label range of each, to find the best fixed answer.
    let mut constant_hits = [0usize; Effort::ALL.len()];
    for trial in trials {
        let grades = trial.grades();
        for t in trial.targets.iter().filter(|t| **t == target) {
            let Some(range) = trial.labels.get(*t) else {
                continue;
            };
            let Some(grade) = grades
                .as_ref()
                .and_then(|g| g.iter().find(|(gt, _)| gt == t))
                .map(|(_, g)| *g)
            else {
                r.failed += 1;
                continue;
            };
            match grade {
                Grade::Hit => r.hit += 1,
                Grade::Under => r.under += 1,
                Grade::Over => r.over += 1,
                Grade::Invalid => r.invalid += 1,
                Grade::Unscored => continue,
            }
            for (i, effort) in Effort::ALL.iter().enumerate() {
                constant_hits[i] += usize::from(range.from <= *effort && *effort <= range.to);
            }
        }
    }
    r.best_constant_hits = constant_hits.into_iter().max().unwrap_or(0);
    r.finish();
    r
}
