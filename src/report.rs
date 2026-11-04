//! Offline reports over saved Runs: one Run in full, several side by side, and a history list.
//! Accuracy, validity, reliability and latency are reported separately and never merged into one
//! number; there is no overall micro-average because label imbalance makes it misleading.

use std::collections::BTreeMap;
use std::fmt;

use crate::cost::{CostTally, attempt_cost};
use crate::dataset::Effort;
use crate::evidence::{CandidateRecord, Manifest, RunFile, RunState, RunSummary, TrialRecord};
use crate::grade::{Grade, Tally, Target};
use crate::provider::{Disposition, normalize};

/// An instruction set as a column: name and full sha256.
type InsId = (String, String);

pub struct Report<'a>(&'a RunFile);

pub fn report(run: &RunFile) -> Report<'_> {
    Report(run)
}

pub struct Comparison<'a>(pub &'a [RunFile]);

fn state(run: &RunFile) -> RunState {
    run.end.as_ref().map_or(RunState::Partial, |e| e.state)
}

fn state_text(run: &RunFile) -> String {
    const NOT_COMPARABLE: &str = "not comparable to a complete run";
    match (&run.end, state(run)) {
        (None, _) => format!("partial (interrupted: no end record; {NOT_COMPARABLE})"),
        (Some(_), RunState::Partial) => format!("partial (cancelled; {NOT_COMPARABLE})"),
        (Some(_), RunState::Failed) => format!("failed ({NOT_COMPARABLE})"),
        (Some(_), RunState::Complete) => "complete".to_string(),
    }
}

fn short(sha: &str) -> String {
    sha.chars().take(12).collect()
}

fn label(ins: &InsId) -> String {
    format!("{} ({})", ins.0, short(&ins.1))
}

fn describe(c: &CandidateRecord) -> String {
    format!(
        "{} ({} {})",
        c.candidate.name,
        c.candidate.provider.name(),
        c.candidate.model
    )
}

/// `k/n (pct)`, or `n/a (0 what)` when there is nothing to divide by.
fn ratio(k: usize, n: usize, what: &str) -> String {
    if n == 0 {
        format!("n/a (0 {what})")
    } else {
        format!("{k}/{n} ({:.1}%)", 100.0 * k as f64 / n as f64)
    }
}

/// Instruction sets in manifest order, then any a Trial names that the manifest does not.
fn instruction_ids(run: &RunFile) -> Vec<InsId> {
    let mut ids: Vec<InsId> = run
        .manifest
        .instructions
        .iter()
        .map(|i| (i.name.clone(), i.sha256.clone()))
        .collect();
    for t in &run.trials {
        let id = (t.instructions.name.clone(), t.instructions.sha256.clone());
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids
}

fn group<'a>(run: &'a RunFile, candidate: &str, ins: &InsId) -> Vec<&'a TrialRecord> {
    run.trials
        .iter()
        .filter(|t| {
            t.candidate == candidate
                && t.instructions.name == ins.0
                && t.instructions.sha256 == ins.1
        })
        .collect()
}

fn tally(trials: &[&TrialRecord], target: Target) -> Tally {
    let mut tally = Tally::default();
    for (_, grade) in trials
        .iter()
        .filter_map(|t| t.grades())
        .flatten()
        .filter(|(t, _)| *t == target)
    {
        tally.add(grade);
    }
    tally
}

fn valid_scored(t: &Tally) -> usize {
    t.hit + t.under + t.over
}

fn accuracy_cell(t: &Tally) -> String {
    ratio(t.hit, valid_scored(t), "valid scored")
}

/// Valid effort per asked target of an answered Trial; `None` when the final Attempt did not answer.
fn efforts(trial: &TrialRecord) -> Option<Vec<(Target, Option<Effort>)>> {
    let attempt = trial
        .attempts
        .last()
        .filter(|a| a.disposition == Disposition::Answered)?;
    let raw = normalize(attempt.raw_response.as_deref().unwrap_or_default()).efforts;
    Some(
        trial
            .targets
            .iter()
            .map(|t| (*t, raw[t].as_str().and_then(Effort::parse)))
            .collect(),
    )
}

/// For one answered Trial: whether it is eligible for joint hit and if so whether it hit, then
/// (agreeing, eligible) pairs for gap agreement.
fn cross_model(
    trial: &TrialRecord,
    efforts: &[(Target, Option<Effort>)],
) -> (Option<bool>, usize, usize) {
    let labelled: Vec<_> = efforts
        .iter()
        .filter_map(|(t, e)| Some((trial.labels.get(*t)?, *e)))
        .collect();
    let joint = (labelled.len() >= 2 && labelled.iter().all(|(_, e)| e.is_some())).then(|| {
        labelled
            .iter()
            .all(|(r, e)| e.is_some_and(|e| r.from <= e && e <= r.to))
    });
    let (mut agree, mut eligible) = (0, 0);
    for (i, (ra, ea)) in labelled.iter().enumerate() {
        for (rb, eb) in &labelled[i + 1..] {
            let (Some(ea), Some(eb)) = (ea, eb) else {
                continue;
            };
            let expected = if ra.to < rb.from {
                std::cmp::Ordering::Less
            } else if rb.to < ra.from {
                std::cmp::Ordering::Greater
            } else {
                continue;
            };
            eligible += 1;
            agree += usize::from(ea.cmp(eb) == expected);
        }
    }
    (joint, agree, eligible)
}

/// Nearest-rank percentile of an ascending slice: the value at index ceil(pct * n / 100) - 1.
fn percentile(sorted: &[u64], pct: usize) -> Option<u64> {
    (!sorted.is_empty()).then(|| sorted[(pct * sorted.len()).div_ceil(100) - 1])
}

fn spread(mut values: Vec<u64>) -> String {
    values.sort_unstable();
    match (percentile(&values, 50), percentile(&values, 95)) {
        (Some(median), Some(p95)) => {
            format!("median {median} ms, p95 {p95} ms (n={})", values.len())
        }
        _ => "n/a (n=0)".to_string(),
    }
}

/// Space-padded columns, left aligned.
fn table(indent: &str, rows: &[Vec<String>]) -> String {
    let widths: Vec<usize> = (0..rows[0].len())
        .map(|c| rows.iter().map(|r| r[c].chars().count()).max().unwrap_or(0))
        .collect();
    rows.iter()
        .map(|row| {
            let cells: Vec<String> = row
                .iter()
                .zip(&widths)
                .map(|(cell, w)| format!("{cell:<w$}"))
                .collect();
            format!("{indent}{}\n", cells.join("  ").trim_end())
        })
        .collect()
}

fn duplicate_hash_warnings(ids: &[InsId]) -> Vec<String> {
    let mut names: Vec<&str> = Vec::new();
    for (name, _) in ids {
        if !names.contains(&name.as_str()) {
            names.push(name);
        }
    }
    names
        .into_iter()
        .filter_map(|name| {
            let hashes: Vec<String> = ids
                .iter()
                .filter(|(n, _)| n == name)
                .map(|(_, sha)| short(sha))
                .collect();
            (hashes.len() > 1).then(|| {
                format!(
                    "instructions `{name}` appears with more than one hash: {}",
                    hashes.join(", ")
                )
            })
        })
        .collect()
}

impl fmt::Display for Report<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let run = self.0;
        let m = &run.manifest;
        let ids = instruction_ids(run);

        writeln!(f, "== Run ==")?;
        writeln!(f, "id: {}", m.id)?;
        writeln!(f, "created: {} (unix ms)", m.created_unix_ms)?;
        writeln!(f, "state: {}", state_text(run))?;
        writeln!(
            f,
            "trials: {} recorded of {} planned",
            run.trials.len(),
            m.planned_trials
        )?;
        writeln!(
            f,
            "dataset: {} sha256 {}",
            m.dataset.name,
            short(&m.dataset.sha256)
        )?;
        writeln!(f, "grading revision: {}", m.grading_revision)?;
        writeln!(f, "schema: {}", m.schema_version)?;

        writeln!(
            f,
            "\n== Accuracy: hit / valid scored (hit + under + over; invalid and unscored are excluded and counted below) =="
        )?;
        for c in &m.candidates {
            let name = &c.candidate.name;
            writeln!(f, "candidate: {}", describe(c))?;
            let mut rows = vec![
                ["target".to_string()]
                    .into_iter()
                    .chain(ids.iter().map(label))
                    .collect::<Vec<_>>(),
            ];
            for target in &m.targets {
                let cells = ids
                    .iter()
                    .map(|i| accuracy_cell(&tally(&group(run, name, i), *target)));
                rows.push([target.to_string()].into_iter().chain(cells).collect());
            }
            write!(f, "{}", table("  ", &rows))?;
            writeln!(f, "  counts per prediction (n = all predictions):")?;
            for i in &ids {
                for target in &m.targets {
                    let t = tally(&group(run, name, i), *target);
                    writeln!(
                        f,
                        "    {} {target}: hit {}, under {}, over {}, invalid {}, unscored {}; n={}",
                        label(i),
                        t.hit,
                        t.under,
                        t.over,
                        t.invalid,
                        t.unscored,
                        valid_scored(&t) + t.invalid + t.unscored
                    )?;
                }
            }
        }

        writeln!(
            f,
            "\n== Cross-model (eligible Trials only; joint hit needs 2+ labelled targets with valid efforts, gap agreement needs non-overlapping labels) =="
        )?;
        for c in &m.candidates {
            writeln!(f, "candidate: {}", describe(c))?;
            for i in &ids {
                let (mut joint_n, mut joint_k, mut gap_k, mut gap_n) = (0, 0, 0, 0);
                for trial in group(run, &c.candidate.name, i) {
                    let Some(e) = efforts(trial) else { continue };
                    let (joint, agree, eligible) = cross_model(trial, &e);
                    joint_n += usize::from(joint.is_some());
                    joint_k += usize::from(joint == Some(true));
                    gap_k += agree;
                    gap_n += eligible;
                }
                writeln!(
                    f,
                    "  {}: joint hit {}; gap agreement {}",
                    label(i),
                    ratio(joint_k, joint_n, "eligible"),
                    ratio(gap_k, gap_n, "eligible")
                )?;
            }
        }

        writeln!(
            f,
            "\n== Consistency (Cases with 2+ valid efforts across repetitions; consistent = all equal) =="
        )?;
        for c in &m.candidates {
            writeln!(f, "candidate: {}", describe(c))?;
            for i in &ids {
                let trials = group(run, &c.candidate.name, i);
                for target in &m.targets {
                    let mut by_case: BTreeMap<&str, Vec<Effort>> = BTreeMap::new();
                    for trial in &trials {
                        for (t, effort) in efforts(trial).into_iter().flatten() {
                            if let (true, Some(effort)) = (t == *target, effort) {
                                by_case.entry(&trial.case_id).or_default().push(effort);
                            }
                        }
                    }
                    let groups: Vec<_> = by_case.values().filter(|v| v.len() >= 2).collect();
                    let same = groups
                        .iter()
                        .filter(|v| v.iter().all(|e| *e == v[0]))
                        .count();
                    writeln!(
                        f,
                        "  {} {target}: consistent {}",
                        label(i),
                        ratio(same, groups.len(), "cases with >=2 valid efforts")
                    )?;
                }
            }
        }

        writeln!(f, "\n== Coverage & validity (separate from accuracy) ==")?;
        for c in &m.candidates {
            writeln!(f, "candidate: {}", describe(c))?;
            for i in &ids {
                let trials = group(run, &c.candidate.name, i);
                let answered = trials.iter().filter(|t| t.answered()).count();
                let grades: Vec<Grade> = trials
                    .iter()
                    .filter_map(|t| t.grades())
                    .flatten()
                    .map(|(_, g)| g)
                    .filter(|g| *g != Grade::Unscored)
                    .collect();
                let invalid = grades.iter().filter(|g| **g == Grade::Invalid).count();
                writeln!(
                    f,
                    "  {}: answered trials {}; invalid labelled predictions {}",
                    label(i),
                    ratio(answered, trials.len(), "recorded trials"),
                    ratio(invalid, grades.len(), "labelled predictions")
                )?;
            }
        }

        writeln!(f, "\n== Reliability ==")?;
        let mine = |c: &CandidateRecord| -> Vec<&TrialRecord> {
            run.trials
                .iter()
                .filter(|t| t.candidate == c.candidate.name)
                .collect()
        };
        for c in &m.candidates {
            let trials = mine(c);
            let ended = |d| {
                trials
                    .iter()
                    .filter(|t| t.attempts.last().is_some_and(|a| a.disposition == d))
                    .count()
            };
            let attempts: usize = trials.iter().map(|t| t.attempts.len()).sum();
            let failed = trials
                .iter()
                .flat_map(|t| &t.attempts)
                .filter(|a| a.disposition != Disposition::Answered)
                .count();
            let retried = trials.iter().filter(|t| t.attempts.len() > 1).count();
            writeln!(f, "candidate: {}", describe(c))?;
            writeln!(
                f,
                "  outcomes: {} provider_error, {} transport_error; attempts: {attempts}, retried trials: {retried}, failed attempts: {failed}",
                ended(Disposition::ProviderError),
                ended(Disposition::TransportError)
            )?;
        }

        writeln!(f, "\n== Latency (nearest-rank percentiles) ==")?;
        for c in &m.candidates {
            let trials = mine(c);
            let per_attempt = trials
                .iter()
                .flat_map(|t| &t.attempts)
                .filter(|a| a.disposition == Disposition::Answered)
                .map(|a| a.latency_ms)
                .collect();
            let per_trial = trials
                .iter()
                .filter(|t| t.answered())
                .map(|t| t.elapsed_ms)
                .collect();
            writeln!(f, "candidate: {}", describe(c))?;
            writeln!(
                f,
                "  per-attempt (answered attempts): {}",
                spread(per_attempt)
            )?;
            writeln!(
                f,
                "  logical trial elapsed (answered trials): {}",
                spread(per_trial)
            )?;
        }

        writeln!(f, "\n== Cost ==")?;
        for c in &m.candidates {
            let mut cost = CostTally::default();
            for attempt in mine(c).iter().flat_map(|t| &t.attempts) {
                cost.add(attempt_cost(c.candidate.price.as_ref(), attempt));
            }
            writeln!(f, "candidate: {}\n  {cost}", describe(c))?;
        }

        writeln!(f, "\n== Warnings ==")?;
        let warnings: Vec<String> = run
            .warnings
            .iter()
            .cloned()
            .chain(duplicate_hash_warnings(&ids))
            .collect();
        if warnings.is_empty() {
            writeln!(f, "none")?;
        }
        for w in warnings {
            writeln!(f, "{w}")?;
        }
        Ok(())
    }
}

/// Every way the other Runs differ from the first, which would make a side-by-side misleading.
fn mismatches(runs: &[RunFile]) -> Vec<String> {
    let first = &runs[0].manifest;
    let mut found = Vec::new();
    for run in runs {
        let m = &run.manifest;
        if state(run) != RunState::Complete {
            found.push(format!(
                "{}: {} run (not comparable to a complete run)",
                m.id,
                state(run).name()
            ));
        }
        if std::ptr::eq(run, &runs[0]) {
            continue;
        }
        let mut differ = |what: &str, ours: String, theirs: String| {
            if ours != theirs {
                found.push(format!(
                    "{}: {what} differs from {} ({ours} vs {theirs})",
                    m.id, first.id
                ));
            }
        };
        differ(
            "dataset",
            short(&m.dataset.sha256),
            short(&first.dataset.sha256),
        );
        differ(
            "grading revision",
            m.grading_revision.to_string(),
            first.grading_revision.to_string(),
        );
        differ(
            "schema version",
            m.schema_version.to_string(),
            first.schema_version.to_string(),
        );
        let names = |targets: &[Target]| {
            targets
                .iter()
                .map(Target::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        };
        differ("target set", names(&m.targets), names(&first.targets));
        differ(
            "repetitions",
            m.settings.repetitions.to_string(),
            first.settings.repetitions.to_string(),
        );
        differ(
            "unscored inclusion",
            m.settings.include_unscored.to_string(),
            first.settings.include_unscored.to_string(),
        );
        let (mut ours, mut theirs) = (m.cases.clone(), first.cases.clone());
        ours.sort();
        theirs.sort();
        if ours != theirs {
            let only_in = |id: &str, a: &[String], b: &[String]| {
                let ids: Vec<&str> = a
                    .iter()
                    .filter(|c| !b.contains(c))
                    .map(String::as_str)
                    .collect();
                match ids.len() {
                    0 => String::new(),
                    n if n <= 10 => format!("; only in {id}: {}", ids.join(", ")),
                    n => format!(
                        "; only in {id}: {}, … +{} more",
                        ids[..10].join(", "),
                        n - 10
                    ),
                }
            };
            found.push(format!(
                "{}: case selection differs from {} ({} vs {} cases{}{})",
                m.id,
                first.id,
                ours.len(),
                theirs.len(),
                only_in(&m.id, &ours, &theirs),
                only_in(&first.id, &theirs, &ours),
            ));
        }
        for i in &m.instructions {
            if let Some(j) = first.instructions.iter().find(|j| j.name == i.name)
                && j.sha256 != i.sha256
            {
                found.push(format!(
                    "{}: instructions `{}` differ in content ({} vs {})",
                    m.id,
                    i.name,
                    short(&i.sha256),
                    short(&j.sha256)
                ));
            }
        }
        let missing_instructions = |from: &Manifest, other: &Manifest| {
            from.instructions
                .iter()
                .filter(|i| !other.instructions.iter().any(|j| j.name == i.name))
                .map(|i| {
                    format!(
                        "{}: instructions `{}` is not in {}",
                        from.id, i.name, other.id
                    )
                })
                .collect::<Vec<_>>()
        };
        found.extend(missing_instructions(m, first));
        found.extend(missing_instructions(first, m));
        let missing = |from: &Manifest, other: &Manifest| {
            from.candidates
                .iter()
                .filter(|c| {
                    !other
                        .candidates
                        .iter()
                        .any(|d| d.candidate.name == c.candidate.name)
                })
                .map(|c| {
                    format!(
                        "{}: candidate `{}` is not in {}",
                        from.id, c.candidate.name, other.id
                    )
                })
                .collect::<Vec<_>>()
        };
        found.extend(missing(m, first));
        found.extend(missing(first, m));
        for c in &m.candidates {
            let route = |c: &CandidateRecord| {
                format!(
                    "{} {} {}",
                    c.candidate.provider.name(),
                    c.candidate.model,
                    c.url
                )
            };
            if let Some(d) = first
                .candidates
                .iter()
                .find(|d| d.candidate.name == c.candidate.name)
                && route(c) != route(d)
            {
                found.push(format!(
                    "{}: candidate `{}` route differs ({} {} vs {} {})",
                    m.id,
                    c.candidate.name,
                    c.candidate.provider.name(),
                    c.candidate.model,
                    d.candidate.provider.name(),
                    d.candidate.model
                ));
            }
        }
    }
    found
}

/// Settings that change reliability and latency but not what the answers are worth, so they never
/// make Runs incomparable.
fn setting_differences(runs: &[RunFile]) -> Vec<String> {
    let first = &runs[0].manifest;
    runs[1..]
        .iter()
        .flat_map(|run| {
            let (m, a, b) = (&run.manifest, &run.manifest.settings, &first.settings);
            [
                (
                    "timeout",
                    format!("{}s", a.timeout_secs),
                    format!("{}s", b.timeout_secs),
                ),
                (
                    "concurrency",
                    a.concurrency.to_string(),
                    b.concurrency.to_string(),
                ),
                ("retries", a.retries.to_string(), b.retries.to_string()),
            ]
            .into_iter()
            .filter(|(_, ours, theirs)| ours != theirs)
            .map(|(what, ours, theirs)| format!("{}: {what} {ours} vs {theirs}", m.id))
            .collect::<Vec<_>>()
        })
        .collect()
}

impl fmt::Display for Comparison<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let runs = self.0;
        let found = mismatches(runs);
        if found.is_empty() {
            writeln!(
                f,
                "no differences found in: dataset, grading, schema, targets, cases, repetitions, unscored inclusion, instructions, candidates and routes"
            )?;
        } else {
            writeln!(f, "Not directly comparable:")?;
            for line in found {
                writeln!(f, "  - {line}")?;
            }
        }
        let settings = setting_differences(runs);
        if !settings.is_empty() {
            writeln!(
                f,
                "\nsettings differ (affect reliability and latency, not accuracy):"
            )?;
            for line in settings {
                writeln!(f, "  - {line}")?;
            }
        }
        writeln!(f, "\nAccuracy: hit / valid scored per Run")?;
        let mut keys: Vec<(String, InsId)> = Vec::new();
        let mut targets: Vec<Target> = Vec::new();
        for run in runs {
            for c in &run.manifest.candidates {
                for i in instruction_ids(run) {
                    let key = (c.candidate.name.clone(), i);
                    if !keys.contains(&key) {
                        keys.push(key);
                    }
                }
            }
            targets.extend(&run.manifest.targets);
        }
        targets.sort();
        targets.dedup();
        let mut rows = vec![
            ["candidate", "instructions", "target"]
                .map(String::from)
                .into_iter()
                .chain(runs.iter().map(|r| r.manifest.id.clone()))
                .collect::<Vec<_>>(),
        ];
        for (name, ins) in &keys {
            for target in &targets {
                let cells = runs.iter().map(|run| {
                    let asked = run
                        .manifest
                        .candidates
                        .iter()
                        .any(|c| &c.candidate.name == name)
                        && instruction_ids(run).contains(ins)
                        && run.manifest.targets.contains(target);
                    if asked {
                        accuracy_cell(&tally(&group(run, name, ins), *target))
                    } else {
                        "-".to_string()
                    }
                });
                rows.push(
                    [name.clone(), label(ins), target.to_string()]
                        .into_iter()
                        .chain(cells)
                        .collect(),
                );
            }
        }
        write!(f, "{}", table("  ", &rows))
    }
}

/// One line per readable Run by created time, then the unreadable files. Entries are
/// (file name, parsed file or read error).
pub fn history(mut entries: Vec<(String, Result<RunSummary, String>)>) -> String {
    entries.sort_by_key(|(name, run)| {
        (
            run.as_ref()
                .map_or(u64::MAX, |r| r.manifest.created_unix_ms),
            name.clone(),
        )
    });
    let mut rows = vec![
        ["id", "created", "state", "trials", "candidates", "dataset"]
            .map(String::from)
            .to_vec(),
    ];
    let mut unreadable = String::new();
    for (name, run) in &entries {
        match run {
            Ok(run) => {
                let m = &run.manifest;
                let candidates: Vec<&str> = m
                    .candidates
                    .iter()
                    .map(|c| c.candidate.name.as_str())
                    .collect();
                rows.push(vec![
                    m.id.clone(),
                    m.created_unix_ms.to_string(),
                    run.state().name().to_string(),
                    format!("{}/{}", run.trials, m.planned_trials),
                    candidates.join(","),
                    short(&m.dataset.sha256),
                ]);
            }
            Err(e) => unreadable.push_str(&format!("{name}  unreadable: {e}\n")),
        }
    }
    let readable = if rows.len() > 1 {
        table("", &rows)
    } else {
        String::new()
    };
    readable + &unreadable
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dataset::Labels;
    use crate::evidence::Hashed;
    use serde_json::json;

    #[test]
    fn nearest_rank_percentiles() {
        let v = [100, 200, 300, 400, 1000];
        assert_eq!(percentile(&v, 50), Some(300)); // rank ceil(2.5) = 3
        assert_eq!(percentile(&v, 95), Some(1000)); // rank ceil(4.75) = 5
        let even = [1, 2, 3, 4];
        assert_eq!(percentile(&even, 50), Some(2)); // rank 2, the lower middle
        assert_eq!(percentile(&even, 95), Some(4));
        assert_eq!(percentile(&[7], 95), Some(7));
        assert_eq!(percentile(&[], 50), None);
        assert_eq!(spread(vec![]), "n/a (n=0)");
        assert_eq!(spread(vec![5, 1, 3]), "median 3 ms, p95 5 ms (n=3)");
    }

    fn trial(labels: serde_json::Value) -> TrialRecord {
        TrialRecord {
            seq: 0,
            candidate: "c".into(),
            instructions: Hashed {
                name: "n".into(),
                sha256: "a".into(),
            },
            case_id: "c1".into(),
            targets: Target::ALL.to_vec(),
            repetition: 1,
            labels: serde_json::from_value::<Labels>(labels).unwrap(),
            request: json!({}),
            attempts: Vec::new(),
            started_unix_ms: 0,
            elapsed_ms: 0,
        }
    }

    fn e(name: &str) -> Option<Effort> {
        Effort::parse(name)
    }

    #[test]
    fn gap_needs_non_overlapping_labels_and_a_strict_order() {
        // haiku low < sonnet medium-high < opus max < fable? fable is unlabelled.
        let t = trial(json!({"haiku": "low", "sonnet": "medium-high", "opus": "max"}));
        let all = |h, s, o| {
            vec![
                (Target::Haiku, e(h)),
                (Target::Sonnet, e(s)),
                (Target::Opus, e(o)),
                (Target::Fable, e("max")),
            ]
        };
        // Strictly increasing: 3 of 3 pairs.
        assert_eq!(
            cross_model(&t, &all("low", "high", "max")),
            (Some(true), 3, 3)
        );
        // haiku = sonnet is not strict (pair 1 disagrees); the other two agree.
        assert_eq!(
            cross_model(&t, &all("medium", "medium", "max")),
            (Some(false), 2, 3)
        );
        // Reversed haiku/sonnet disagrees too.
        assert_eq!(cross_model(&t, &all("max", "low", "max")).1, 1);
        // Overlapping labels form no pair.
        let t = trial(json!({"sonnet": "low-high", "opus": "medium-max"}));
        assert_eq!(
            cross_model(&t, &all("low", "low", "low")),
            (Some(false), 0, 0)
        );
        // An invalid effort drops the pairs it is in and makes the Trial ineligible for joint hit.
        let t = trial(json!({"haiku": "low", "sonnet": "max"}));
        let mut invalid = all("low", "max", "low");
        invalid[1].1 = None;
        assert_eq!(cross_model(&t, &invalid), (None, 0, 0));
    }

    #[test]
    fn history_orders_by_created_time_and_keeps_unreadable_files() {
        assert_eq!(history(Vec::new()), "");
        let bad = Err::<RunSummary, String>("line 1: x".into());
        let text = history(vec![("b.jsonl".into(), bad)]);
        assert_eq!(text, "b.jsonl  unreadable: line 1: x\n");
    }
}
