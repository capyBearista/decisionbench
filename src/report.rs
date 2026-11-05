//! Offline reports over saved Runs: one Run in full, several side by side, and a history list.
//! Accuracy, validity, reliability and latency are reported separately and never merged into one
//! number; there is no overall micro-average because label imbalance makes it misleading.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use clap::ValueEnum;

use crate::cost::{CostTally, attempt_cost};
use crate::dataset::Effort;
use crate::evidence::{CandidateRecord, Manifest, RunFile, RunState, RunSummary, TrialRecord};
use crate::grade::{Grade, Tally, Target};
use crate::provider::{Disposition, normalize};

/// An instruction set as a column: name and full sha256.
type InsId = (String, String);

/// What the columns of the summary matrix are. `Auto` picks one; a built `Matrix` never holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Axis {
    /// Prompts when there are several, else candidates when there are several, else prompts.
    Auto,
    /// One column per prompt in each run; rows are candidate x target.
    Prompt,
    /// One column per candidate in each run; rows are prompt x target.
    Candidate,
}

impl Axis {
    /// What the columns are called in the text.
    pub fn name(self) -> &'static str {
        match self {
            Axis::Candidate => "candidates",
            _ => "prompts",
        }
    }

    /// Prompts become candidates and candidates prompts; `Auto` is not either, so it too gives candidates.
    pub fn toggled(self) -> Axis {
        match self {
            Axis::Candidate => Axis::Prompt,
            _ => Axis::Candidate,
        }
    }
}

pub struct Report<'a>(&'a RunFile, Axis);

pub fn report(run: &RunFile, axis: Axis) -> Report<'_> {
    Report(run, axis)
}

pub struct Comparison<'a>(pub &'a [RunFile], pub Axis);

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

/// One prompt or candidate in one Run: a column of the matrix. The first column is the baseline.
pub struct Column {
    pub label: String,
    pub run_id: String,
}

/// Paired trials gained and lost against the baseline, and the cases behind them: `cases_up` and
/// `cases_down` count the cases whose net change over their repetitions is up or down (a case that
/// netted zero is left out). `noise`: an exact two-sided sign test on those cases is not
/// significant (`p >= 0.05`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delta {
    pub gained: usize,
    pub lost: usize,
    pub cases_up: usize,
    pub cases_down: usize,
    pub noise: bool,
}

impl Delta {
    fn new(gained: usize, lost: usize, cases_up: usize, cases_down: usize) -> Self {
        Self {
            gained,
            lost,
            cases_up,
            cases_down,
            noise: sign_test_p(cases_up, cases_down) >= 0.05,
        }
    }
}

/// Exact two-sided sign test: the chance, if each changed case were equally likely to move either
/// way, of a split at least as lopsided as `up` against `down`. Summed in log space so that a count
/// in the thousands neither overflows nor panics; the terms are at most 1 and may underflow to 0.
fn sign_test_p(up: usize, down: usize) -> f64 {
    let (n, k) = (up + down, up.min(down));
    let mut log_choose = 0.0; // ln C(n, i), built up from i = 0
    let mut tail = 0.0;
    for i in 0..=k {
        if i > 0 {
            log_choose += ((n - i + 1) as f64).ln() - (i as f64).ln();
        }
        tail += (log_choose - n as f64 * std::f64::consts::LN_2).exp();
    }
    (2.0 * tail).min(1.0)
}

pub struct Cell {
    pub tally: Tally,
    /// `None` for the baseline column and when no trial pairs with the baseline's.
    pub delta: Option<Delta>,
}

/// How a delta reads: significantly more cases up than down, the reverse, or noise (not significant,
/// which includes no changed case).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeltaKind {
    Gained,
    Lost,
    Noise,
}

/// A matrix cell as text: the counts, and the paired delta (`+g -l`, then ` ~` when not significant) when
/// there is one. `Display` joins them as `Display for Matrix` prints them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellText {
    pub counts: String,
    pub delta: Option<(String, DeltaKind)>,
}

impl CellText {
    fn plain(text: impl Into<String>) -> Self {
        Self {
            counts: text.into(),
            delta: None,
        }
    }
}

impl fmt::Display for CellText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.counts)?;
        if let Some((delta, _)) = &self.delta {
            write!(f, "  {delta}")?;
        }
        Ok(())
    }
}

/// The text of one matrix cell; `-` when the Run did not ask it.
pub fn cell_parts(cell: Option<&Cell>) -> CellText {
    let Some(cell) = cell else {
        return CellText::plain("-");
    };
    CellText {
        counts: accuracy_cell(&cell.tally),
        delta: cell.delta.as_ref().map(|d| {
            let kind = if d.noise {
                DeltaKind::Noise
            } else if d.cases_up > d.cases_down {
                DeltaKind::Gained
            } else {
                DeltaKind::Lost
            };
            let noise = if d.noise { " ~" } else { "" };
            (format!("+{} -{}{noise}", d.gained, d.lost), kind)
        }),
    }
}

fn pooled_note(axis: Axis) -> &'static str {
    match axis {
        Axis::Candidate => {
            "pooled = rows present in every column, all prompts and targets together, over valid scored predictions; it is not a score"
        }
        _ => {
            "pooled = rows present in every column, all candidates and targets together, over valid scored predictions; it is not a score"
        }
    }
}
const DELTA_NOTE: &str = "+g -l = paired trials (case x repetition) gained and lost against the baseline; ~ = not significant (exact sign test on the cases that changed, p >= 0.05)";

pub struct MatrixRow {
    /// The candidate on the prompt axis, the prompt on the candidate axis.
    pub label: String,
    pub target: Target,
    /// `None` when that column's Run did not ask this candidate, target or prompt.
    pub cells: Vec<Option<Cell>>,
}

/// Accuracy per row and column, with paired deltas against the first column. On the prompt axis the
/// columns are prompts and the rows (candidate, target); on the candidate axis the columns are
/// candidates and the rows (prompt, target).
pub struct Matrix {
    /// The axis the columns are on; never `Auto`.
    pub axis: Axis,
    pub columns: Vec<Column>,
    pub rows: Vec<MatrixRow>,
    /// Per column: the rows present in every column added together, so all columns cover the same
    /// rows; `None` when no row is present in every column.
    pub pooled: Vec<Option<Cell>>,
}

/// The name; plus the hash when another of `ids` shares the name with other content.
fn named(ids: &[&InsId]) -> Vec<String> {
    ids.iter()
        .enumerate()
        .map(|(i, ins)| {
            if ids
                .iter()
                .enumerate()
                .any(|(j, o)| j != i && o.0 == ins.0 && o.1 != ins.1)
            {
                label(ins)
            } else {
                ins.0.clone()
            }
        })
        .collect()
}

/// `labels`, plus ` in <run id>` where another label is the same (the same name in several Runs).
fn apart(labels: Vec<String>, run_ids: &[&str]) -> Vec<String> {
    labels
        .iter()
        .enumerate()
        .map(|(i, l)| {
            if labels.iter().enumerate().any(|(j, o)| j != i && o == l) {
                format!("{l} in {}", run_ids[i])
            } else {
                l.clone()
            }
        })
        .collect()
}

/// The valid scored grade of the target's prediction in each (case, repetition) Trial.
fn scored<'a>(
    run: &'a RunFile,
    candidate: &str,
    ins: &InsId,
    target: Target,
) -> BTreeMap<(&'a str, u32), Grade> {
    group(run, candidate, ins)
        .into_iter()
        .filter_map(|t| {
            let (_, grade) = t.grades()?.into_iter().find(|(x, _)| *x == target)?;
            matches!(grade, Grade::Hit | Grade::Under | Grade::Over)
                .then(|| ((t.case_id.as_str(), t.repetition), grade))
        })
        .collect()
}

/// The axis `Auto` stands for: prompts when the Runs between them have several, else candidates when
/// they have several, else prompts. Any other axis is kept.
fn resolve(runs: &[RunFile], axis: Axis) -> Axis {
    if axis != Axis::Auto {
        return axis;
    }
    let prompts: BTreeSet<InsId> = runs.iter().flat_map(instruction_ids).collect();
    let candidates: BTreeSet<&str> = runs
        .iter()
        .flat_map(|r| &r.manifest.candidates)
        .map(|c| c.candidate.name.as_str())
        .collect();
    if prompts.len() <= 1 && candidates.len() > 1 {
        Axis::Candidate
    } else {
        Axis::Prompt
    }
}

impl Matrix {
    pub fn from_runs(runs: &[RunFile], axis: Axis) -> Matrix {
        let axis = resolve(runs, axis);
        let by_prompt = axis == Axis::Prompt;
        // Every prompt and candidate of the Runs, first seen first. A column is a Run and an index
        // into one of them (the one the axis names); a row is an index into the other.
        let mut prompts: Vec<InsId> = Vec::new();
        let mut candidates: Vec<&str> = Vec::new();
        for run in runs {
            for ins in instruction_ids(run) {
                if !prompts.contains(&ins) {
                    prompts.push(ins);
                }
            }
            for c in &run.manifest.candidates {
                if !candidates.contains(&c.candidate.name.as_str()) {
                    candidates.push(&c.candidate.name);
                }
            }
        }
        // With no prompt or no candidate there is nothing to compare, on either axis.
        let nothing = prompts.is_empty() || candidates.is_empty();
        let sources: Vec<(usize, usize)> = runs
            .iter()
            .enumerate()
            .filter(|_| !nothing)
            .flat_map(|(r, run)| {
                let found: Vec<usize> = if by_prompt {
                    instruction_ids(run)
                        .iter()
                        .filter_map(|i| prompts.iter().position(|p| p == i))
                        .collect()
                } else {
                    run.manifest
                        .candidates
                        .iter()
                        .filter_map(|c| candidates.iter().position(|n| *n == c.candidate.name))
                        .collect()
                };
                found.into_iter().map(move |k| (r, k))
            })
            .collect();
        let pair = |column: usize, row: usize| {
            if by_prompt {
                (candidates[row], &prompts[column])
            } else {
                (candidates[column], &prompts[row])
            }
        };
        let run_ids: Vec<&str> = sources
            .iter()
            .map(|(r, _)| runs[*r].manifest.id.as_str())
            .collect();
        let names = if by_prompt {
            named(
                &sources
                    .iter()
                    .map(|(_, k)| &prompts[*k])
                    .collect::<Vec<_>>(),
            )
        } else {
            sources
                .iter()
                .map(|(_, k)| candidates[*k].to_string())
                .collect()
        };
        let columns: Vec<Column> = apart(names, &run_ids)
            .into_iter()
            .zip(&run_ids)
            .map(|(label, run_id)| Column {
                label,
                run_id: run_id.to_string(),
            })
            .collect();
        let Some(base) = sources.first() else {
            return Matrix {
                axis,
                columns,
                rows: Vec::new(),
                pooled: Vec::new(),
            };
        };
        let row_labels: Vec<String> = if by_prompt {
            candidates.iter().map(ToString::to_string).collect()
        } else {
            named(&prompts.iter().collect::<Vec<_>>())
        };
        let mut targets: Vec<Target> = runs
            .iter()
            .flat_map(|run| run.manifest.targets.iter().copied())
            .collect();
        targets.sort();
        targets.dedup();

        let mut rows = Vec::new();
        for (row, row_label) in row_labels.iter().enumerate() {
            for target in &targets {
                let mut cells: Vec<Option<Cell>> = sources
                    .iter()
                    .map(|(r, k)| {
                        let run = &runs[*r];
                        let (candidate, ins) = pair(*k, row);
                        let asked = run
                            .manifest
                            .candidates
                            .iter()
                            .any(|c| c.candidate.name == candidate)
                            && instruction_ids(run).contains(ins)
                            && run.manifest.targets.contains(target);
                        asked.then(|| Cell {
                            tally: tally(&group(run, candidate, ins), *target),
                            delta: None,
                        })
                    })
                    .collect();
                if cells[0].is_some() {
                    let (candidate, ins) = pair(base.1, row);
                    let before = scored(&runs[base.0], candidate, ins, *target);
                    for (j, (r, k)) in sources.iter().enumerate().skip(1) {
                        let Some(cell) = cells[j].as_mut() else {
                            continue;
                        };
                        let (candidate, ins) = pair(*k, row);
                        let after = scored(&runs[*r], candidate, ins, *target);
                        let (mut gained, mut lost, mut pairs) = (0, 0, 0);
                        let mut net: BTreeMap<&str, i64> = BTreeMap::new();
                        for (key, was) in &before {
                            let Some(now) = after.get(key) else { continue };
                            pairs += 1;
                            let up = *was != Grade::Hit && *now == Grade::Hit;
                            let down = *was == Grade::Hit && *now != Grade::Hit;
                            (gained, lost) = (gained + usize::from(up), lost + usize::from(down));
                            *net.entry(key.0).or_default() += i64::from(up) - i64::from(down);
                        }
                        let up = net.values().filter(|n| **n > 0).count();
                        let down = net.values().filter(|n| **n < 0).count();
                        cell.delta = (pairs > 0).then(|| Delta::new(gained, lost, up, down));
                    }
                }
                rows.push(MatrixRow {
                    label: row_label.clone(),
                    target: *target,
                    cells,
                });
            }
        }

        let pooled = sources
            .iter()
            .enumerate()
            .map(|(j, _)| {
                let cells: Vec<&Cell> = rows
                    .iter()
                    .filter(|row| row.cells.iter().all(Option::is_some))
                    .filter_map(|row| row.cells[j].as_ref())
                    .collect();
                if cells.is_empty() {
                    return None;
                }
                let mut tally = Tally::default();
                let (mut gained, mut lost, mut up, mut down, mut any) = (0, 0, 0, 0, false);
                for cell in &cells {
                    tally.hit += cell.tally.hit;
                    tally.under += cell.tally.under;
                    tally.over += cell.tally.over;
                    tally.invalid += cell.tally.invalid;
                    tally.unscored += cell.tally.unscored;
                    if let Some(d) = &cell.delta {
                        gained += d.gained;
                        lost += d.lost;
                        up += d.cases_up;
                        down += d.cases_down;
                        any = true;
                    }
                }
                Some(Cell {
                    tally,
                    delta: any.then(|| Delta::new(gained, lost, up, down)),
                })
            })
            .collect();
        Matrix {
            axis,
            columns,
            rows,
            pooled,
        }
    }

    /// The header, then one row per (candidate or prompt, target); see `pooled_row` for the last row.
    pub fn table(&self) -> Vec<Vec<CellText>> {
        let rows = if self.axis == Axis::Candidate {
            "prompt"
        } else {
            "candidate"
        };
        let mut header = vec![CellText::plain(rows), CellText::plain("target")];
        header.extend(self.columns.iter().enumerate().map(|(i, c)| {
            CellText::plain(if i == 0 {
                format!("{} (baseline)", c.label)
            } else {
                c.label.clone()
            })
        }));
        let mut table = vec![header];
        for row in &self.rows {
            table.push(
                [
                    CellText::plain(&row.label),
                    CellText::plain(row.target.to_string()),
                ]
                .into_iter()
                .chain(row.cells.iter().map(|c| cell_parts(c.as_ref())))
                .collect(),
            );
        }
        table
    }

    /// The rows present in every column added together; `None` when there is nothing to pool.
    pub fn pooled_row(&self) -> Option<Vec<CellText>> {
        self.pooled.iter().any(Option::is_some).then(|| {
            [CellText::plain("pooled"), CellText::plain("all")]
                .into_iter()
                .chain(self.pooled.iter().map(|c| cell_parts(c.as_ref())))
                .collect()
        })
    }

    /// The lines under the table: what the pooled row (when shown) and the deltas mean.
    pub fn notes(&self, pooled: bool) -> Vec<&'static str> {
        let mut notes = Vec::new();
        if pooled {
            notes.push(pooled_note(self.axis));
        }
        if self
            .rows
            .iter()
            .any(|row| row.cells.iter().flatten().any(|c| c.delta.is_some()))
        {
            notes.push(DELTA_NOTE);
        }
        notes
    }

    /// The table text; the pooled row only when asked for and there is something to pool.
    fn render(&self, pooled: bool) -> String {
        let labels: Vec<&str> = self.columns.iter().map(|c| c.label.as_str()).collect();
        let Some(baseline) = labels.first() else {
            return String::new();
        };
        let mut cells = self.table();
        let pooled = pooled.then(|| self.pooled_row()).flatten();
        let shown = pooled.is_some();
        cells.extend(pooled);
        let text: Vec<Vec<String>> = cells
            .iter()
            .map(|row| row.iter().map(ToString::to_string).collect())
            .collect();
        let mut out = format!(
            "  baseline: {baseline}\n  columns: {} ({})\n{}",
            labels.join(", "),
            self.axis.name(),
            table("  ", &text)
        );
        for note in self.notes(shown) {
            out += &format!("  {note}\n");
        }
        out
    }
}

impl fmt::Display for Matrix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render(true))
    }
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

        writeln!(f, "\n== Summary ==")?;
        let summary = Matrix::from_runs(std::slice::from_ref(run), self.1);
        write!(f, "{}", summary.render(summary.rows.len() > 1))?;

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
/// What makes the Runs not directly comparable. With the candidates as the columns, a Candidate in
/// only some Runs is a column, not a difference (as a prompt is on the prompt axis).
fn mismatches(runs: &[RunFile], axis: Axis) -> Vec<String> {
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
        if axis != Axis::Candidate {
            found.extend(missing(m, first));
            found.extend(missing(first, m));
        }
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

/// What a comparison opens with: whether the Runs are not directly comparable, and the lines that
/// say so (or that nothing differs). `axis` is the one the matrix uses, `Auto` resolved as it does.
pub fn comparability(runs: &[RunFile], axis: Axis) -> (bool, Vec<String>) {
    let axis = resolve(runs, axis);
    let found = mismatches(runs, axis);
    if found.is_empty() {
        let candidates = if axis == Axis::Candidate {
            " and"
        } else {
            ", candidates and"
        };
        let none = format!(
            "no differences found in: dataset, grading, schema, targets, cases, repetitions, unscored inclusion{candidates} routes"
        );
        return (false, vec![none]);
    }
    let lines = std::iter::once("Not directly comparable:".to_string())
        .chain(found.into_iter().map(|line| format!("  - {line}")))
        .collect();
    (true, lines)
}

impl fmt::Display for Comparison<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let runs = self.0;
        for line in comparability(runs, self.1).1 {
            writeln!(f, "{line}")?;
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
        let matrix = Matrix::from_runs(runs, self.1);
        let per = if matrix.axis == Axis::Candidate {
            "candidate"
        } else {
            "prompt"
        };
        writeln!(f, "\nAccuracy: hit / valid scored per {per}")?;
        write!(f, "{matrix}")
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

    /// A Run of target `sonnet` (label high) built through the file reader. Each trial is
    /// (candidate, instruction index, case, repetition, effort); "-" is an unanswered Trial.
    fn run_of(
        id: &str,
        repetitions: u32,
        instructions: &[(&str, &str)],
        candidates: &[&str],
        trials: &[(&str, usize, &str, u32, &str)],
    ) -> RunFile {
        let mut lines = vec![json!({
            "type": "run", "schema_version": crate::evidence::SCHEMA_VERSION, "id": id,
            "created_unix_ms": 1, "dataset": {"name": "d", "sha256": "d"},
            "instructions": instructions.iter()
                .map(|(n, s)| json!({"name": n, "sha256": s, "text": "t"})).collect::<Vec<_>>(),
            "grading_revision": crate::evidence::GRADING_REVISION,
            "candidates": candidates.iter()
                .map(|c| json!({"name": c, "provider": "zen", "model": "m", "url": "u"}))
                .collect::<Vec<_>>(),
            "cases": ["c1"], "targets": ["sonnet"],
            "settings": {"repetitions": repetitions, "concurrency": 1, "timeout_secs": 60,
                "retries": 0, "include_unscored": false},
            "planned_trials": trials.len(),
        })];
        for (seq, (candidate, ins, case, repetition, effort)) in trials.iter().enumerate() {
            let attempt = if *effort == "-" {
                json!({"started_unix_ms": 0, "latency_ms": 1, "disposition": "provider_error",
                    "http_status": 503, "raw_response": "busy", "error": null})
            } else {
                let body =
                    json!({"answers": {"effort_sonnet": {"type": "choice", "choice": effort}}});
                json!({"started_unix_ms": 0, "latency_ms": 1, "disposition": "answered",
                    "http_status": 200, "raw_response": body.to_string(), "error": null})
            };
            lines.push(json!({
                "type": "trial", "seq": seq, "candidate": candidate,
                "instructions": {"name": instructions[*ins].0, "sha256": instructions[*ins].1},
                "case_id": case, "targets": ["sonnet"], "repetition": repetition,
                "labels": {"sonnet": "high"}, "request": {}, "attempts": [attempt],
                "started_unix_ms": 0, "elapsed_ms": 1,
            }));
        }
        let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
        crate::evidence::read_run(text.as_bytes()).unwrap()
    }

    fn labels(m: &Matrix) -> Vec<&str> {
        m.columns.iter().map(|c| c.label.as_str()).collect()
    }

    fn delta(
        (gained, lost): (usize, usize),
        (cases_up, cases_down): (usize, usize),
        noise: bool,
    ) -> Option<Delta> {
        Some(Delta {
            gained,
            lost,
            cases_up,
            cases_down,
            noise,
        })
    }

    #[test]
    fn sign_test_p_is_the_exact_two_sided_binomial_tail() {
        let close = |got: f64, want: f64| assert!((got - want).abs() < 1e-9, "{got} vs {want}");
        close(sign_test_p(0, 0), 1.0);
        close(sign_test_p(3, 0), 0.25);
        close(sign_test_p(0, 6), 0.03125);
        close(sign_test_p(6, 3), 260.0 / 512.0);
        close(sign_test_p(10, 0), 2.0 / 1024.0);
        close(sign_test_p(4, 4), 1.0); // 2 * P(X <= 4) is above 1: capped
        for (a, b) in [(6, 3), (7, 1), (0, 5), (2, 2)] {
            assert_eq!(sign_test_p(a, b), sign_test_p(b, a));
        }
        // Thousands of cases neither overflow nor underflow into a panic or a NaN.
        for (a, b) in [(500, 450), (5000, 0), (0, 4000), (3000, 2900)] {
            let p = sign_test_p(a, b);
            assert!((0.0..=1.0).contains(&p), "({a}, {b}) gave {p}");
        }
        assert!(sign_test_p(500, 450) > 0.05);
        assert!(sign_test_p(600, 400) < 0.001);
    }

    #[test]
    fn matrix_columns_are_prompts_with_the_first_as_baseline() {
        let x = run_of(
            "X1",
            1,
            &[("neutral", "aaa"), ("effortless", "bbb")],
            &["c"],
            &[],
        );
        let y = run_of("Y1", 1, &[("terse", "ccc")], &["c"], &[]);
        let m = Matrix::from_runs(&[x, y], Axis::Prompt);
        assert_eq!(labels(&m), ["neutral", "effortless", "terse"]);
        assert_eq!(m.columns[0].run_id, "X1");
        assert_eq!(m.columns[2].run_id, "Y1");
    }

    #[test]
    fn matrix_labels_stay_unique_when_names_collide() {
        // Same name, different content: the hash tells them apart.
        let x = run_of("X1", 1, &[("neutral", "aaaaaaaaaaaaaaaa")], &["c"], &[]);
        let y = run_of("Y1", 1, &[("neutral", "bbbbbbbbbbbbbbbb")], &["c"], &[]);
        let m = Matrix::from_runs(&[x, y], Axis::Prompt);
        assert_eq!(
            labels(&m),
            ["neutral (aaaaaaaaaaaa)", "neutral (bbbbbbbbbbbb)"]
        );
        // Same content in two runs: the run id tells them apart.
        let x = run_of("X1", 1, &[("neutral", "aaa")], &["c"], &[]);
        let y = run_of("Y1", 1, &[("neutral", "aaa")], &["c"], &[]);
        let m = Matrix::from_runs(&[x, y], Axis::Prompt);
        assert_eq!(labels(&m), ["neutral in X1", "neutral in Y1"]);
        // Both rules at once: only the colliding pair gets the run id.
        let x = run_of("X1", 1, &[("neutral", "aaa")], &["c"], &[]);
        let y = run_of("Y1", 1, &[("neutral", "aaa")], &["c"], &[]);
        let z = run_of("Z1", 1, &[("neutral", "bbb")], &["c"], &[]);
        let m = Matrix::from_runs(&[x, y, z], Axis::Prompt);
        assert_eq!(
            labels(&m),
            [
                "neutral (aaa) in X1",
                "neutral (aaa) in Y1",
                "neutral (bbb)"
            ]
        );
    }

    #[test]
    fn matrix_marks_what_a_run_did_not_ask_and_counts_what_it_did() {
        let x = run_of("X1", 1, &[("p", "a")], &["c"], &[("c", 0, "c1", 1, "high")]);
        let y = run_of(
            "Y1",
            1,
            &[("q", "b")],
            &["c", "d"],
            &[("d", 0, "c1", 1, "low"), ("c", 0, "c1", 1, "max")],
        );
        let m = Matrix::from_runs(&[x, y], Axis::Prompt);
        let rows: Vec<_> = m.rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(rows, ["c", "d"]);
        // Different prompts in different runs: every cell is its own, `d` was not asked in X1.
        assert_eq!(m.rows[0].cells[0].as_ref().unwrap().tally.hit, 1);
        assert_eq!(m.rows[0].cells[1].as_ref().unwrap().tally.over, 1);
        assert!(m.rows[1].cells[0].is_none());
        assert_eq!(m.rows[1].cells[1].as_ref().unwrap().tally.under, 1);
        assert!(m.rows[0].cells[0].as_ref().unwrap().delta.is_none());
        // `d` has no baseline cell, so no delta either.
        assert!(m.rows[1].cells[1].as_ref().unwrap().delta.is_none());
    }

    /// Baseline `p` and variant `q` in one run, one candidate, repetition 1.
    /// (case, baseline effort, variant effort); a missing side is `None`.
    fn paired(repetitions: u32, cases: &[(&str, Option<&str>, Option<&str>)]) -> Matrix {
        let mut trials = Vec::new();
        for (case, base, variant) in cases {
            for (ins, effort) in [(0, base), (1, variant)] {
                if let Some(effort) = effort {
                    trials.push(("c", ins, *case, 1, *effort));
                }
            }
        }
        Matrix::from_runs(
            &[run_of(
                "X1",
                repetitions,
                &[("p", "a"), ("q", "b")],
                &["c"],
                &trials,
            )],
            Axis::Prompt,
        )
    }

    #[test]
    fn matrix_delta_counts_only_pairs_of_valid_scored_predictions() {
        let m = paired(
            1,
            &[
                ("a", Some("high"), Some("high")), // hit -> hit: no change
                ("b", Some("low"), Some("high")),  // gained
                ("c", Some("max"), Some("high")),  // gained
                ("d", Some("high"), Some("low")),  // lost
                ("e", Some("?"), Some("high")),    // invalid baseline: ignored
                ("f", Some("high"), Some("-")),    // unanswered variant: ignored
                ("g", Some("high"), None),         // missing from the variant: ignored
                ("h", None, Some("high")),         // missing from the baseline: ignored
                ("i", Some("low"), Some("max")),   // miss -> miss: no change
            ],
        );
        let cell = m.rows[0].cells[1].as_ref().unwrap();
        assert_eq!(cell.delta, delta((2, 1), (2, 1), true));
        // The baseline column never carries a delta.
        assert!(m.rows[0].cells[0].as_ref().unwrap().delta.is_none());
    }

    #[test]
    fn matrix_delta_has_none_without_pairs() {
        let m = paired(1, &[("a", Some("high"), None), ("b", None, Some("high"))]);
        assert!(m.rows[0].cells[1].as_ref().unwrap().delta.is_none());
    }

    #[test]
    fn matrix_delta_is_significant_only_by_a_sign_test_on_the_cases_that_changed() {
        let gains = |n: usize, losses: usize| {
            let ids: Vec<String> = (0..n + losses).map(|i| format!("k{i}")).collect();
            let cases: Vec<_> = ids
                .iter()
                .enumerate()
                .map(|(i, id)| {
                    let (b, v) = if i < n {
                        ("low", "high")
                    } else {
                        ("high", "low")
                    };
                    (id.as_str(), Some(b), Some(v))
                })
                .collect();
            paired(1, &cases).rows[0].cells[1]
                .as_ref()
                .unwrap()
                .delta
                .clone()
        };
        assert_eq!(gains(5, 0), delta((5, 0), (5, 0), true)); // p = 0.0625
        assert_eq!(gains(6, 0), delta((6, 0), (6, 0), false)); // p = 0.03125
        assert_eq!(gains(0, 6), delta((0, 6), (0, 6), false));
        assert_eq!(gains(7, 1), delta((7, 1), (7, 1), true)); // p = 0.0703
        assert_eq!(gains(8, 1), delta((8, 1), (8, 1), false)); // p = 0.0391
        assert_eq!(gains(1, 3), delta((1, 3), (1, 3), true));
        assert_eq!(gains(0, 0), None);
    }

    #[test]
    fn matrix_delta_counts_a_case_once_however_many_repetitions_flip() {
        // Every case gains in both repetitions: `+2` trials per case, but one flipped case each.
        let flips = |cases: usize| {
            let ids: Vec<String> = (0..cases).map(|i| format!("k{i}")).collect();
            let mut trials = Vec::new();
            for id in &ids {
                for rep in [1, 2] {
                    trials.push(("c", 0, id.as_str(), rep, "low"));
                    trials.push(("c", 1, id.as_str(), rep, "high"));
                }
            }
            let run = run_of("X1", 2, &[("p", "a"), ("q", "b")], &["c"], &trials);
            Matrix::from_runs(&[run], Axis::Prompt).rows[0].cells[1]
                .as_ref()
                .unwrap()
                .delta
                .clone()
        };
        // Three cases x two repetitions: 6 trials gained, but 3 cases, p = 0.25.
        assert_eq!(flips(3), delta((6, 0), (3, 0), true));
        // Six cases: p = 0.03125, however the trials are counted.
        assert_eq!(flips(6), delta((12, 0), (6, 0), false));
    }

    #[test]
    fn matrix_delta_ignores_a_case_whose_repetitions_cancel() {
        // k0 gains in repetition 1 and loses in repetition 2: two flipped trials, no flipped case.
        let run = run_of(
            "X1",
            2,
            &[("p", "a"), ("q", "b")],
            &["c"],
            &[
                ("c", 0, "k0", 1, "low"),
                ("c", 1, "k0", 1, "high"),
                ("c", 0, "k0", 2, "high"),
                ("c", 1, "k0", 2, "low"),
            ],
        );
        let m = Matrix::from_runs(&[run], Axis::Prompt);
        assert_eq!(
            m.rows[0].cells[1].as_ref().unwrap().delta,
            delta((1, 1), (0, 0), true)
        );
    }

    #[test]
    fn matrix_pools_every_row_per_column() {
        let base = run_of(
            "X1",
            1,
            &[("p", "a")],
            &["c", "d"],
            &[
                ("c", 0, "c1", 1, "high"),
                ("c", 0, "c2", 1, "low"),
                ("d", 0, "c1", 1, "low"),
                ("d", 0, "c2", 1, "?"),
            ],
        );
        let variant = run_of(
            "Y1",
            1,
            &[("q", "b")],
            &["c", "d"],
            &[
                ("c", 0, "c1", 1, "low"),  // c1: lost
                ("c", 0, "c2", 1, "high"), // c2: gained
                ("d", 0, "c1", 1, "high"), // gained
                ("d", 0, "c2", 1, "high"), // baseline invalid: ignored
            ],
        );
        let m = Matrix::from_runs(&[base, variant], Axis::Prompt);
        let (b, v) = (m.pooled[0].as_ref().unwrap(), m.pooled[1].as_ref().unwrap());
        assert_eq!((b.tally.hit, b.tally.under, b.tally.invalid), (1, 2, 1));
        assert_eq!((v.tally.hit, v.tally.under), (3, 1));
        assert!(b.delta.is_none());
        assert_eq!(v.delta, delta((2, 1), (2, 1), true));
    }

    #[test]
    fn matrix_pools_only_rows_present_in_every_column() {
        // `d` was only asked in Y1, so its hits must not lift Y1's pooled rate above X1's.
        let x = run_of("X1", 1, &[("p", "a")], &["c"], &[("c", 0, "c1", 1, "low")]);
        let y = run_of(
            "Y1",
            1,
            &[("q", "b")],
            &["c", "d"],
            &[("c", 0, "c1", 1, "low"), ("d", 0, "c1", 1, "high")],
        );
        let m = Matrix::from_runs(&[x, y], Axis::Prompt);
        // The row itself still shows everything.
        assert_eq!(m.rows[1].cells[1].as_ref().unwrap().tally.hit, 1);
        let (b, v) = (m.pooled[0].as_ref().unwrap(), m.pooled[1].as_ref().unwrap());
        assert_eq!((b.tally.hit, b.tally.under), (0, 1));
        assert_eq!((v.tally.hit, v.tally.under), (0, 1));
        assert_eq!(v.delta, delta((0, 0), (0, 0), true));
    }

    #[test]
    fn matrix_without_a_row_in_every_column_has_no_pooled_row() {
        let x = run_of("X1", 1, &[("p", "a")], &["c"], &[("c", 0, "c1", 1, "high")]);
        let y = run_of("Y1", 1, &[("q", "b")], &["d"], &[("d", 0, "c1", 1, "high")]);
        let m = Matrix::from_runs(&[x, y], Axis::Prompt);
        assert!(m.pooled.iter().all(Option::is_none));
        assert!(m.pooled_row().is_none());
        assert!(!m.to_string().contains("pooled"), "{m}");
    }

    #[test]
    fn matrix_from_one_run_with_three_prompts_has_three_columns() {
        let m = paired(1, &[("a", Some("high"), Some("high"))]);
        assert_eq!(labels(&m), ["p", "q"]);
        let x = run_of(
            "X1",
            1,
            &[("p", "a"), ("q", "b"), ("r", "c")],
            &["c"],
            &[
                ("c", 0, "c1", 1, "low"),
                ("c", 1, "c1", 1, "high"),
                ("c", 2, "c1", 1, "low"),
            ],
        );
        let m = Matrix::from_runs(&[x], Axis::Prompt);
        assert_eq!(labels(&m), ["p", "q", "r"]);
        let deltas: Vec<_> = m.rows[0]
            .cells
            .iter()
            .map(|c| c.as_ref().unwrap().delta.clone())
            .collect();
        assert_eq!(
            deltas,
            [
                None,
                delta((1, 0), (1, 0), true),
                delta((0, 0), (0, 0), true)
            ]
        );
    }

    #[test]
    fn matrix_text_has_the_baseline_columns_deltas_and_pooled_row() {
        let x = run_of(
            "X1",
            1,
            &[("p", "a"), ("q", "b")],
            &["c", "d"],
            &[
                ("c", 0, "c1", 1, "low"),
                ("c", 1, "c1", 1, "high"),
                ("c", 0, "c2", 1, "high"),
                ("c", 1, "c2", 1, "high"),
                ("d", 0, "c1", 1, "high"),
            ],
        );
        let text = Matrix::from_runs(&[x], Axis::Prompt).to_string();
        let lines: Vec<String> = text
            .lines()
            .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
            .collect();
        assert_eq!(
            lines,
            [
                "baseline: p",
                "columns: p, q (prompts)",
                "candidate target p (baseline) q",
                "c sonnet 1/2 (50.0%) 2/2 (100.0%) +1 -0 ~",
                "d sonnet 1/1 (100.0%) n/a (0 valid scored)",
                "pooled all 2/3 (66.7%) 2/2 (100.0%) +1 -0 ~",
                "pooled = rows present in every column, all candidates and targets together, over valid scored predictions; it is not a score",
                "+g -l = paired trials (case x repetition) gained and lost against the baseline; ~ = not significant (exact sign test on the cases that changed, p >= 0.05)",
            ]
        );
    }

    #[test]
    fn cell_parts_split_the_counts_from_a_delta_and_say_how_to_read_it() {
        let cell = |d| Cell {
            tally: Tally {
                hit: 1,
                ..Tally::default()
            },
            delta: d,
        };
        let parts = |d| cell_parts(Some(&cell(d)));
        assert_eq!(cell_parts(None), CellText::plain("-"));
        assert_eq!(parts(None), CellText::plain("1/1 (100.0%)"));
        let read = |p: CellText| p.delta.unwrap();
        // Significant: the colour follows which side has more flipped cases.
        assert_eq!(
            read(parts(delta((9, 2), (7, 1), false))),
            ("+9 -2".into(), DeltaKind::Gained)
        );
        assert_eq!(
            read(parts(delta((2, 9), (1, 7), false))),
            ("+2 -9".into(), DeltaKind::Lost)
        );
        // Not significant is dim and marked, whichever side is ahead.
        assert_eq!(
            read(parts(delta((3, 1), (3, 1), true))),
            ("+3 -1 ~".into(), DeltaKind::Noise)
        );
        assert_eq!(
            read(parts(delta((2, 2), (0, 0), true))),
            ("+2 -2 ~".into(), DeltaKind::Noise)
        );
        assert_eq!(
            parts(delta((9, 2), (7, 1), false)).to_string(),
            "1/1 (100.0%)  +9 -2"
        );
    }

    #[test]
    fn matrix_table_is_the_text_the_display_prints() {
        let x = run_of(
            "X1",
            1,
            &[("p", "a"), ("q", "b")],
            &["c"],
            &[("c", 0, "c1", 1, "low"), ("c", 1, "c1", 1, "high")],
        );
        let m = Matrix::from_runs(&[x], Axis::Prompt);
        let rows: Vec<Vec<String>> = m
            .table()
            .iter()
            .chain(m.pooled_row().iter())
            .map(|r| r.iter().map(ToString::to_string).collect())
            .collect();
        assert_eq!(
            rows,
            [
                vec!["candidate", "target", "p (baseline)", "q"],
                vec!["c", "sonnet", "0/1 (0.0%)", "1/1 (100.0%)  +1 -0 ~"],
                vec!["pooled", "all", "0/1 (0.0%)", "1/1 (100.0%)  +1 -0 ~"],
            ]
        );
        assert_eq!(m.notes(true).len(), 2);
        assert_eq!(m.notes(false), [DELTA_NOTE]);
    }

    #[test]
    fn comparability_is_the_text_the_comparison_opens_with() {
        let complete = |id, repetitions| {
            let mut run = run_of(id, repetitions, &[("p", "a")], &["c"], &[]);
            run.end = Some(crate::evidence::EndRecord {
                state: RunState::Complete,
                finished_unix_ms: 9,
            });
            run
        };
        let (incomparable, lines) =
            comparability(&[complete("X1", 1), complete("Y1", 1)], Axis::Auto);
        assert!(!incomparable);
        assert!(lines.len() == 1 && lines[0].starts_with("no differences found in: "));
        let (incomparable, lines) =
            comparability(&[complete("X1", 1), complete("Y1", 2)], Axis::Auto);
        assert!(incomparable);
        assert_eq!(lines[0], "Not directly comparable:");
        assert!(lines[1].starts_with("  - Y1: repetitions differs from X1"));
    }

    #[test]
    fn candidates_are_not_a_difference_when_they_are_the_columns() {
        let run = |id, candidates: &[&str]| {
            let mut run = run_of(id, 1, &[("p", "a")], candidates, &[]);
            run.end = Some(crate::evidence::EndRecord {
                state: RunState::Complete,
                finished_unix_ms: 9,
            });
            run
        };
        let runs = [run("X1", &["c", "d"]), run("Y1", &["d", "e"])];
        let lines = |axis| comparability(&runs, axis).1.join("\n");
        // One prompt, different candidates: Auto puts the candidates in the columns.
        for axis in [Axis::Auto, Axis::Candidate] {
            let text = lines(axis);
            assert!(!text.contains("is not in"), "{text}");
            assert!(text.ends_with("unscored inclusion and routes"), "{text}");
        }
        let text = lines(Axis::Prompt);
        assert!(text.contains("X1: candidate `c` is not in Y1"), "{text}");
        assert!(text.contains("Y1: candidate `e` is not in X1"), "{text}");
        // Routes are still compared: `d` differs in model.
        let mut y = run("Y1", &["d"]);
        y.manifest.candidates[0].candidate.model = "other".into();
        let text = comparability(&[run("X1", &["d"]), y], Axis::Candidate)
            .1
            .join("\n");
        assert!(text.contains("candidate `d` route differs"), "{text}");
        // The comparison prints the same lines.
        let printed = Comparison(&runs, Axis::Auto).to_string();
        assert!(!printed.contains("is not in"), "{printed}");
    }

    #[test]
    fn matrix_of_candidates_without_prompts_is_empty_on_every_axis() {
        for axis in [Axis::Auto, Axis::Prompt, Axis::Candidate] {
            let m = Matrix::from_runs(&[run_of("X1", 1, &[], &["c", "d"], &[])], axis);
            assert!(m.columns.is_empty() && m.rows.is_empty() && m.pooled.is_empty());
            assert_eq!(m.to_string(), "");
        }
    }

    #[test]
    fn matrix_of_a_run_with_no_prompts_is_empty() {
        let m = Matrix::from_runs(&[run_of("X1", 1, &[], &["c"], &[])], Axis::Auto);
        assert!(m.columns.is_empty() && m.rows.is_empty() && m.pooled.is_empty());
        assert_eq!(m.to_string(), "");
    }

    #[test]
    fn matrix_text_without_a_delta_has_no_legend() {
        let x = run_of("X1", 1, &[("p", "a")], &["c"], &[("c", 0, "c1", 1, "high")]);
        let text = Matrix::from_runs(&[x], Axis::Prompt).to_string();
        assert!(!text.contains("+g -l"), "{text}");
        assert!(
            text.contains("\n  c          sonnet  1/1 (100.0%)\n"),
            "{text}"
        );
    }

    /// Lines of a matrix's text with runs of spaces collapsed.
    fn squashed(m: &Matrix) -> Vec<String> {
        m.to_string()
            .lines()
            .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
            .collect()
    }

    /// Candidates c (baseline) and d, prompts p and q, one case: c misses p and hits q, d the reverse.
    fn crossed() -> RunFile {
        run_of(
            "X1",
            1,
            &[("p", "a"), ("q", "b")],
            &["c", "d"],
            &[
                ("c", 0, "c1", 1, "low"),
                ("c", 1, "c1", 1, "high"),
                ("d", 0, "c1", 1, "high"),
                ("d", 1, "c1", 1, "low"),
            ],
        )
    }

    #[test]
    fn candidate_axis_has_candidate_columns_prompt_rows_and_a_paired_delta_between_candidates() {
        let m = Matrix::from_runs(&[crossed()], Axis::Candidate);
        assert_eq!(m.axis, Axis::Candidate);
        assert_eq!(m.axis.toggled(), Axis::Prompt);
        assert_eq!(labels(&m), ["c", "d"]);
        let rows: Vec<_> = m.rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(rows, ["p", "q"]);
        // d against the baseline c, for the same prompt: gained on p, lost on q.
        assert_eq!(
            m.rows[0].cells[1].as_ref().unwrap().delta,
            delta((1, 0), (1, 0), true)
        );
        assert_eq!(
            m.rows[1].cells[1].as_ref().unwrap().delta,
            delta((0, 1), (0, 1), true)
        );
        assert!(m.rows[0].cells[0].as_ref().unwrap().delta.is_none());
        assert_eq!(
            squashed(&m),
            [
                "baseline: c",
                "columns: c, d (candidates)",
                "prompt target c (baseline) d",
                "p sonnet 0/1 (0.0%) 1/1 (100.0%) +1 -0 ~",
                "q sonnet 1/1 (100.0%) 0/1 (0.0%) +0 -1 ~",
                "pooled all 1/2 (50.0%) 1/2 (50.0%) +1 -1 ~",
                "pooled = rows present in every column, all prompts and targets together, over valid scored predictions; it is not a score",
                "+g -l = paired trials (case x repetition) gained and lost against the baseline; ~ = not significant (exact sign test on the cases that changed, p >= 0.05)",
            ]
        );
        // The prompt axis of the same run is the transpose: it keeps its own words and header.
        let m = Matrix::from_runs(&[crossed()], Axis::Prompt);
        assert_eq!((m.axis, m.axis.toggled()), (Axis::Prompt, Axis::Candidate));
        assert_eq!(squashed(&m)[1], "columns: p, q (prompts)");
        assert_eq!(squashed(&m)[2], "candidate target p (baseline) q");
    }

    #[test]
    fn candidate_axis_labels_a_column_with_its_run_and_a_row_with_its_hash_only_on_a_collision() {
        let x = run_of("X1", 1, &[("neutral", "aaaaaaaaaaaaaaaa")], &["c"], &[]);
        let y = run_of("Y1", 1, &[("neutral", "bbbbbbbbbbbbbbbb")], &["c"], &[]);
        let m = Matrix::from_runs(&[x, y], Axis::Candidate);
        assert_eq!(labels(&m), ["c in X1", "c in Y1"]);
        let rows: Vec<_> = m.rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(rows, ["neutral (aaaaaaaaaaaa)", "neutral (bbbbbbbbbbbb)"]);
        // The same prompt in both runs is one row, and a candidate alone in its run needs no run id.
        let x = run_of("X1", 1, &[("neutral", "aaa")], &["c"], &[]);
        let y = run_of("Y1", 1, &[("neutral", "aaa")], &["c", "d"], &[]);
        let m = Matrix::from_runs(&[x, y], Axis::Candidate);
        assert_eq!(labels(&m), ["c in X1", "c in Y1", "d"]);
        assert_eq!(m.rows.len(), 1);
        assert_eq!(m.rows[0].label, "neutral");
        assert_eq!(m.columns[1].run_id, "Y1");
    }

    #[test]
    fn candidate_axis_marks_what_a_run_did_not_ask_and_pools_only_rows_every_column_has() {
        let x = run_of(
            "X1",
            1,
            &[("p", "a"), ("q", "b")],
            &["c", "d"],
            &[
                ("c", 0, "c1", 1, "high"),
                ("c", 1, "c1", 1, "high"),
                ("d", 0, "c1", 1, "low"),
                ("d", 1, "c1", 1, "high"),
            ],
        );
        let y = run_of(
            "Y1",
            1,
            &[("r", "c"), ("p", "a")],
            &["c"],
            &[("c", 1, "c1", 1, "low")],
        );
        let m = Matrix::from_runs(&[x, y], Axis::Candidate);
        assert_eq!(labels(&m), ["c in X1", "d", "c in Y1"]);
        // Prompts in first-seen order across the runs.
        let rows: Vec<_> = m.rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(rows, ["p", "q", "r"]);
        let asked: Vec<Vec<bool>> = m
            .rows
            .iter()
            .map(|r| r.cells.iter().map(Option::is_some).collect())
            .collect();
        assert_eq!(
            asked,
            [
                [true, true, true],
                [true, true, false],
                [false, false, true]
            ]
        );
        assert!(
            squashed(&m).contains(&"q sonnet 1/1 (100.0%) 1/1 (100.0%) +0 -0 ~ -".to_string()),
            "{m}"
        );
        // Only prompt p is in every column: q's hits must not lift X1's pooled rate.
        let pooled: Vec<_> = m.pooled.iter().map(|c| c.as_ref().unwrap()).collect();
        assert_eq!(
            pooled.iter().map(|c| c.tally.hit).collect::<Vec<_>>(),
            [1, 0, 0]
        );
        assert_eq!(
            pooled.iter().map(|c| c.tally.under).collect::<Vec<_>>(),
            [0, 1, 1]
        );
        assert_eq!(pooled[1].delta, delta((0, 1), (0, 1), true));
        assert_eq!(pooled[2].delta, delta((0, 1), (0, 1), true));
        // With no prompt in every column there is nothing to pool.
        let x = run_of("X1", 1, &[("p", "a")], &["c"], &[]);
        let y = run_of("Y1", 1, &[("q", "b")], &["d"], &[]);
        let m = Matrix::from_runs(&[x, y], Axis::Candidate);
        assert!(m.pooled.iter().all(Option::is_none) && m.pooled_row().is_none());
    }

    #[test]
    fn auto_picks_prompts_for_several_prompts_else_candidates_for_several_candidates() {
        let axis = |runs: &[RunFile]| Matrix::from_runs(runs, Axis::Auto).axis;
        let run = |ins: &[(&str, &str)], candidates: &[&str]| run_of("X1", 1, ins, candidates, &[]);
        let (p, q) = (("p", "a"), ("q", "b"));
        assert_eq!(axis(&[run(&[p, q], &["c"])]), Axis::Prompt);
        assert_eq!(axis(&[run(&[p], &["c", "d"])]), Axis::Candidate);
        assert_eq!(axis(&[run(&[p, q], &["c", "d"])]), Axis::Prompt);
        assert_eq!(axis(&[run(&[p], &["c"])]), Axis::Prompt);
        // Prompts and candidates are counted across the runs: the same prompt does not make two.
        let y = run_of("Y1", 1, &[p], &["d"], &[]);
        assert_eq!(axis(&[run(&[p], &["c"]), y]), Axis::Candidate);
        let y = run_of("Y1", 1, &[p], &["c"], &[]);
        assert_eq!(axis(&[run(&[p], &["c"]), y]), Axis::Prompt);
        let y = run_of("Y1", 1, &[q], &["c"], &[]);
        assert_eq!(axis(&[run(&[p], &["c"]), y]), Axis::Prompt);
        // A same-named prompt with other content is another prompt.
        let y = run_of("Y1", 1, &[("p", "z")], &["d"], &[]);
        assert_eq!(axis(&[run(&[p], &["c"]), y]), Axis::Prompt);
        // An explicit axis is kept.
        let m = Matrix::from_runs(&[run(&[p], &["c", "d"])], Axis::Prompt);
        assert_eq!(m.axis, Axis::Prompt);
    }

    #[test]
    fn history_orders_by_created_time_and_keeps_unreadable_files() {
        assert_eq!(history(Vec::new()), "");
        let bad = Err::<RunSummary, String>("line 1: x".into());
        let text = history(vec![("b.jsonl".into(), bad)]);
        assert_eq!(text, "b.jsonl  unreadable: line 1: x\n");
    }
}
