//! The tuning scorer: per-model hit rate, the hit rate of the best fixed answer (the floor) and
//! their difference (the lift), over the Cases of one saved Run (ADR-0007).
//!
//! This is the only place a number meant to be optimised is computed, and `show` and `compare` never
//! print it (SPEC §6). It refuses to read holdout results unless asked, and refuses a Run whose
//! instructions quote a Case message, so a tuned prompt cannot memorise the dataset.

use std::collections::{BTreeMap, HashMap};

use serde::Serialize;

use crate::dataset::{Case, Effort, Split};
use crate::evidence::{RunFile, TrialRecord};
use crate::grade::{Grade, Target};

/// Messages shorter than this are too generic to count as a quote.
const MIN_QUOTE_CHARS: usize = 20;

#[derive(Debug, Serialize)]
pub struct Score {
    pub run: String,
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
    if !allow_holdout {
        let holdout = run.trials.iter().find_map(|t| {
            by_id
                .get(t.case_id.as_str())
                .filter(|c| c.split == Split::Holdout)
        });
        if let Some(case) = holdout {
            return Err(format!(
                "the run includes holdout case {}; pass --allow-holdout to score it (once, at the end of tuning)",
                case.id
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

    let mut groups = BTreeMap::new();
    for trial in &run.trials {
        groups
            .entry((
                trial.candidate.as_str(),
                trial.instructions.name.as_str(),
                trial.instructions.sha256.as_str(),
            ))
            .or_insert_with(Vec::new)
            .push(trial);
    }
    let scores = groups
        .into_iter()
        .map(|((candidate, name, sha), trials)| VariantScore {
            candidate: candidate.to_string(),
            instructions: name.to_string(),
            instructions_sha256: sha.to_string(),
            pooled: pooled(&trials),
            targets: Target::ALL
                .into_iter()
                .filter(|t| trials.iter().any(|tr| tr.targets.contains(t)))
                .map(|t| (t, rates(&trials, t)))
                .collect(),
        })
        .collect();
    Ok(Score {
        run: run.manifest.id.clone(),
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
