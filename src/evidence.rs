//! Immutable evidence: one append-only JSONL file per Run, readable offline (SPEC §5).
//!
//! Line 1 is the `run` manifest, then one `trial` line per finished Trial, then an `end` line.
//! Grades and costs are derived when the file is read and never stored. The file never holds
//! headers or keys.

use std::fmt;
use std::fs::{DirBuilder, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cost::{CostTally, attempt_cost, plural};
use crate::dataset::Labels;
use crate::grade::{Grade, Tally, Target, grade};
use crate::provider::{Attempt, Candidate, Disposition, normalize};
use crate::run::Settings;

pub const SCHEMA_VERSION: u32 = 4;
/// Bump when grading semantics change, so old evidence stays interpretable.
pub const GRADING_REVISION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hashed {
    pub name: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstructionsRecord {
    pub name: String,
    pub sha256: String,
    pub text: String,
}

/// A Candidate as it was when the Run started, with the URL its requests actually went to (route
/// identity) and its price snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateRecord {
    #[serde(flatten)]
    pub candidate: Candidate,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub schema_version: u32,
    /// `{unix_ms}-{pid}`; also the file name.
    pub id: String,
    pub created_unix_ms: u64,
    pub dataset: Hashed,
    pub instructions: Vec<InstructionsRecord>,
    pub grading_revision: u32,
    pub candidates: Vec<CandidateRecord>,
    pub cases: Vec<String>,
    pub targets: Vec<Target>,
    pub settings: Settings,
    pub planned_trials: usize,
}

/// One logical Trial with every Attempt made for it. Its outcome is the final Attempt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrialRecord {
    /// Position in the plan; records are written as Trials finish, so file order is not plan order.
    pub seq: usize,
    pub candidate: String,
    pub instructions: Hashed,
    pub case_id: String,
    /// The Target Models the request asked about, in `Target::ALL` order.
    pub targets: Vec<Target>,
    pub repetition: u32,
    /// The Case's labels as they were when the request was made.
    pub labels: Labels,
    pub request: Value,
    pub attempts: Vec<Attempt>,
    pub started_unix_ms: u64,
    /// Covers every Attempt and the pauses between them.
    pub elapsed_ms: u64,
}

impl TrialRecord {
    fn last_attempt(&self) -> Option<&Attempt> {
        self.attempts.last()
    }

    pub fn answered(&self) -> bool {
        self.last_attempt()
            .is_some_and(|a| a.disposition == Disposition::Answered)
    }

    /// One grade per asked target; `None` unless the final Attempt was answered.
    pub fn grades(&self) -> Option<Vec<(Target, Grade)>> {
        let attempt = self
            .last_attempt()
            .filter(|a| a.disposition == Disposition::Answered)?;
        let efforts = normalize(attempt.raw_response.as_deref().unwrap_or_default()).efforts;
        Some(
            self.targets
                .iter()
                .map(|t| (*t, grade(self.labels.get(*t), &efforts[t])))
                .collect(),
        )
    }

    /// The per-trial progress line `run` prints to stderr.
    pub fn progress(&self, done: usize, total: usize) -> String {
        let outcome = match (self.grades(), self.last_attempt()) {
            (Some(grades), _) => grades
                .iter()
                .map(|(target, grade)| format!("{target} {grade}"))
                .collect::<Vec<_>>()
                .join(" · "),
            (None, Some(a)) => {
                let status = a
                    .http_status
                    .map_or_else(String::new, |s| format!(" (HTTP {s})"));
                format!("{}{status}: {}", a.disposition.name(), failure_text(a))
            }
            (None, None) => "no attempts".to_string(),
        };
        format!(
            "[{done}/{total}] {} {} {} #{}: {outcome} ({}, {} ms)",
            self.candidate,
            self.instructions.name,
            self.case_id,
            self.repetition,
            plural(self.attempts.len(), "attempt", "attempts"),
            self.elapsed_ms
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    /// Every planned Trial was recorded.
    Complete,
    /// The Run was cancelled.
    Partial,
    /// The Run stopped on an internal error such as an evidence write failure.
    Failed,
}

impl RunState {
    pub fn name(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Partial => "partial",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndRecord {
    pub state: RunState,
    pub finished_unix_ms: u64,
}

/// One line of the file, tagged by `type`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Line {
    Run(Manifest),
    Trial(TrialRecord),
    End(EndRecord),
}

/// Creates `dir` (mode 0700) and opens `{id}.jsonl` (mode 0600) for writing, never overwriting.
/// Done before any request is sent, so an unwritable destination fails with nothing sent.
pub fn create(dir: &Path, id: &str) -> Result<(File, PathBuf), String> {
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let path = dir.join(format!("{id}.jsonl"));
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok((file, path))
}

/// The line as written: compact JSON and a trailing newline.
pub fn encode(line: &Line) -> String {
    let mut text = serde_json::to_string(line).expect("evidence serializes");
    text.push('\n');
    text
}

/// The line as it is safe to show on a terminal: C1 controls escaped, still valid JSON.
pub fn printable(line: &Line) -> String {
    escape_c1(&encode(line))
}

/// Appends one line with a single `write_all`. If that fails the reply is already paid for; the
/// caller must keep the line (see `printable`).
pub fn append(file: &mut File, path: &Path, line: &Line) -> Result<(), String> {
    write_text(file, path, encode(line))
}

/// Like `append`, for after a failed write: that write may have left a partial line with no
/// newline, so this line starts on a fresh one instead of being glued onto it.
pub fn append_on_new_line(file: &mut File, path: &Path, line: &Line) -> Result<(), String> {
    write_text(file, path, format!("\n{}", encode(line)))
}

fn write_text(file: &mut File, path: &Path, text: String) -> Result<(), String> {
    file.write_all(text.as_bytes())
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// serde_json escapes C0 controls but not C1 (U+0080-U+009F, e.g. CSI), which a terminal acts on.
/// Writes them as JSON `\u00XX` escapes so the printed copy stays valid JSON.
fn escape_c1(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '\u{80}'..='\u{9f}' => format!("\\u{:04x}", c as u32),
            _ => c.to_string(),
        })
        .collect()
}

/// A parsed Run file.
#[derive(Debug)]
pub struct RunFile {
    pub manifest: Manifest,
    /// Sorted by `seq`.
    pub trials: Vec<TrialRecord>,
    /// `None` when the file has no end line: the process stopped before it could write one.
    pub end: Option<EndRecord>,
    pub warnings: Vec<String>,
}

fn unsupported_schema(value: &Value) -> String {
    let found = value
        .get("schema_version")
        .map_or_else(|| "missing".to_string(), Value::to_string);
    format!("unsupported schema_version {found} (this build reads {SCHEMA_VERSION})")
}

/// Reads a Run file. A missing end line and a truncated last line are tolerated (the process may
/// have died mid-write); an unparseable line anywhere else is an error, except one directly
/// followed by an `end` line with state `failed` (a write failed and left a partial line).
pub fn read_run(bytes: &[u8]) -> Result<RunFile, String> {
    parse(bytes, false).map(|(run, _)| run)
}

/// What a list of Runs needs: the manifest, the number of Trials and the end line. Trial lines
/// are only checked for their `type` tag (not read into records), except the last two, which can
/// be cut or partial; so a Trial line with a bad body in the middle counts here but fails
/// `read_run`. Open the file with `read_run` for anything more.
pub struct RunSummary {
    pub manifest: Manifest,
    pub trials: usize,
    pub end: Option<EndRecord>,
}

impl RunSummary {
    pub fn state(&self) -> RunState {
        self.end.as_ref().map_or(RunState::Partial, |e| e.state)
    }
}

pub fn read_summary(bytes: &[u8]) -> Result<RunSummary, String> {
    let (run, skimmed) = parse(bytes, true)?;
    Ok(RunSummary {
        manifest: run.manifest,
        trials: run.trials.len() + skimmed,
        end: run.end,
    })
}

/// `skim` leaves Trial lines other than the last two unread and counts them instead.
fn parse(bytes: &[u8], skim: bool) -> Result<(RunFile, usize), String> {
    #[derive(Deserialize)]
    struct Tag {
        r#type: String,
    }
    // Lossy so a final line cut inside a multibyte character is a warning, not a failure.
    let text = String::from_utf8_lossy(bytes);
    let mut lines = text
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty());
    let (_, first) = lines.next().ok_or("empty file")?;
    // The old single-evaluation file was one pretty-printed object, so line 1 is not JSON.
    let first: Value = serde_json::from_str(first)
        .or_else(|e| serde_json::from_str(&text).map_err(|_| format!("line 1: {e}")))?;
    if first.get("schema_version") != Some(&Value::from(SCHEMA_VERSION)) {
        return Err(unsupported_schema(&first));
    }
    let manifest = match serde_json::from_value(first) {
        Ok(Line::Run(manifest)) => manifest,
        Ok(_) => return Err("line 1: expected the run record".to_string()),
        Err(e) => return Err(format!("line 1: {e}")),
    };
    let rest: Vec<(usize, &str)> = lines.collect();
    let (mut trials, mut end, mut warnings, mut skimmed) = (Vec::new(), None, Vec::new(), 0);
    for (i, (n, line)) in rest.iter().enumerate() {
        if skim
            && i + 2 < rest.len()
            && serde_json::from_str::<Tag>(line).is_ok_and(|t| t.r#type == "trial")
        {
            skimmed += 1;
            continue;
        }
        match serde_json::from_str::<Line>(line) {
            Ok(Line::Trial(trial)) => trials.push(trial),
            Ok(Line::End(record)) => end = Some(record),
            Ok(Line::Run(_)) => {
                return Err(format!("line {}: unexpected second run record", n + 1));
            }
            Err(e) if i + 1 == rest.len() => {
                warnings.push(format!("ignoring unreadable last line {}: {e}", n + 1));
            }
            Err(e) => {
                let failed_end_follows = matches!(
                    rest.get(i + 1)
                        .map(|(_, next)| serde_json::from_str::<Line>(next)),
                    Some(Ok(Line::End(EndRecord {
                        state: RunState::Failed,
                        ..
                    })))
                );
                if !failed_end_follows {
                    return Err(format!("line {}: {e}", n + 1));
                }
                warnings.push(format!(
                    "ignoring partial line {} left by a failed write: {e}",
                    n + 1
                ));
            }
        }
    }
    trials.sort_by_key(|t| t.seq);
    let run = RunFile {
        manifest,
        trials,
        end,
        warnings,
    };
    Ok((run, skimmed))
}

/// What went wrong: the provider's message when the body has one, else a cut of the raw body.
/// Control characters are dropped: the text is provider-controlled and goes to a terminal.
fn failure_text(attempt: &Attempt) -> String {
    let raw = attempt.raw_response.as_deref().unwrap_or_default();
    let message = serde_json::from_str::<Value>(raw)
        .ok()
        .and_then(|body| body.pointer("/error/message")?.as_str().map(String::from));
    let text = message
        .or_else(|| attempt.error.clone())
        .unwrap_or_else(|| match raw.char_indices().nth(200) {
            Some((end, _)) => format!("{}...", &raw[..end]),
            None => raw.to_string(),
        });
    text.chars().filter(|c| !c.is_control()).collect()
}

/// The summary `run` and `show` both print.
impl fmt::Display for RunFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.end {
            Some(end) => writeln!(f, "run: {}", end.state.name())?,
            None => writeln!(f, "run: partial (interrupted: no end record)")?,
        }
        writeln!(
            f,
            "trials: {} recorded of {} planned",
            self.trials.len(),
            self.manifest.planned_trials
        )?;
        for record in &self.manifest.candidates {
            let candidate = &record.candidate;
            let mine: Vec<&TrialRecord> = self
                .trials
                .iter()
                .filter(|t| t.candidate == candidate.name)
                .collect();
            write!(
                f,
                "candidate: {} ({} {})",
                candidate.name,
                candidate.provider.name(),
                candidate.model
            )?;
            if mine.is_empty() {
                writeln!(f, ": no trials recorded")?;
                continue;
            }
            writeln!(f)?;
            for instructions in &self.manifest.instructions {
                let group: Vec<&&TrialRecord> = mine
                    .iter()
                    .filter(|t| t.instructions.name == instructions.name)
                    .collect();
                if group.is_empty() {
                    continue;
                }
                for target in &self.manifest.targets {
                    let mut tally = Tally::default();
                    for (_, grade) in group
                        .iter()
                        .filter_map(|t| t.grades())
                        .flatten()
                        .filter(|(t, _)| t == target)
                    {
                        tally.add(grade);
                    }
                    let short: String = instructions.sha256.chars().take(12).collect();
                    writeln!(f, "  {} ({short}) {target}: {tally}", instructions.name)?;
                }
            }
            let (mut answered, mut provider, mut transport) = (0, 0, 0);
            let (mut attempts, mut retried) = (0, 0);
            let mut cost = CostTally::default();
            for trial in &mine {
                match trial.last_attempt().map(|a| a.disposition) {
                    Some(Disposition::Answered) => answered += 1,
                    Some(Disposition::ProviderError) => provider += 1,
                    Some(Disposition::TransportError) => transport += 1,
                    None => {}
                }
                attempts += trial.attempts.len();
                retried += usize::from(trial.attempts.len() > 1);
                for attempt in &trial.attempts {
                    cost.add(attempt_cost(candidate.price.as_ref(), attempt));
                }
            }
            writeln!(
                f,
                "  outcomes: {answered} answered, {provider} provider_error, {transport} transport_error; attempts: {attempts}, retried trials: {retried}"
            )?;
            writeln!(f, "  {cost}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::Provider;
    use serde_json::json;

    fn manifest(planned: usize) -> Line {
        Line::Run(Manifest {
            schema_version: SCHEMA_VERSION,
            id: "1-2".into(),
            created_unix_ms: 1,
            dataset: Hashed {
                name: "d".into(),
                sha256: "0".repeat(64),
            },
            instructions: vec![InstructionsRecord {
                name: "neutral".into(),
                sha256: "a".repeat(64),
                text: "t".into(),
            }],
            grading_revision: GRADING_REVISION,
            candidates: vec![CandidateRecord {
                candidate: Candidate {
                    name: "c".into(),
                    decision_model: None,
                    provider: Provider::Zen,
                    model: "m".into(),
                    price: None,
                },
                url: "http://x".into(),
            }],
            cases: vec!["c001".into()],
            targets: vec![Target::Sonnet],
            settings: Settings {
                repetitions: 1,
                concurrency: 1,
                timeout_secs: 60,
                retries: 1,
                include_unscored: false,
            },
            planned_trials: planned,
        })
    }

    fn trial(seq: usize) -> Line {
        Line::Trial(TrialRecord {
            seq,
            candidate: "c".into(),
            instructions: Hashed {
                name: "neutral".into(),
                sha256: "a".repeat(64),
            },
            case_id: "c001".into(),
            targets: vec![Target::Sonnet],
            repetition: 1,
            labels: Labels::default(),
            request: json!({}),
            attempts: Vec::new(),
            started_unix_ms: 1,
            elapsed_ms: 2,
        })
    }

    fn file(lines: &[Line]) -> String {
        lines.iter().map(encode).collect()
    }

    #[test]
    fn lines_round_trip_and_sort_trials_by_seq() {
        let end = Line::End(EndRecord {
            state: RunState::Complete,
            finished_unix_ms: 9,
        });
        let text = file(&[manifest(3), trial(2), trial(0), trial(1), end]);
        assert!(text.lines().all(|l| l.contains(r#""type":"#)));
        let run = read_run(text.as_bytes()).unwrap();
        let seqs: Vec<usize> = run.trials.iter().map(|t| t.seq).collect();
        assert_eq!(seqs, [0, 1, 2]);
        assert_eq!(run.end.unwrap().state, RunState::Complete);
        assert_eq!(run.manifest.candidates[0].url, "http://x");
        assert!(run.warnings.is_empty());
    }

    #[test]
    fn a_missing_end_line_and_a_cut_last_line_are_tolerated() {
        let whole = file(&[manifest(2), trial(0)]);
        let run = read_run(whole.as_bytes()).unwrap();
        assert!(run.end.is_none() && run.warnings.is_empty());

        let cut = format!("{whole}{}", &encode(&trial(1))[..30]);
        let run = read_run(cut.as_bytes()).unwrap();
        assert_eq!(run.trials.len(), 1);
        assert_eq!(run.warnings.len(), 1);
        assert!(run.warnings[0].contains("line 3"), "{:?}", run.warnings);
    }

    #[test]
    fn a_summary_counts_trials_and_tolerates_a_partial_file() {
        let end = Line::End(EndRecord {
            state: RunState::Complete,
            finished_unix_ms: 9,
        });
        let summary =
            read_summary(file(&[manifest(4), trial(0), trial(1), trial(2), end]).as_bytes());
        let summary = summary.unwrap();
        assert_eq!(summary.trials, 3);
        assert_eq!(summary.state(), RunState::Complete);
        assert_eq!(summary.manifest.planned_trials, 4);

        // No end line: partial. A cut last line is not a Trial, however many lines precede it.
        let whole = file(&[manifest(9), trial(0), trial(1), trial(2), trial(3)]);
        let summary = read_summary(whole.as_bytes()).unwrap();
        assert_eq!((summary.trials, summary.state()), (4, RunState::Partial));
        let cut = format!("{whole}{}", &encode(&trial(4))[..30]);
        assert_eq!(read_summary(cut.as_bytes()).unwrap().trials, 4);
        // Same answers as the full read.
        assert_eq!(read_run(cut.as_bytes()).unwrap().trials.len(), 4);

        // An unreadable middle line is still an error; a bad manifest is too.
        let middle = format!(
            "{}{{oops\n{}",
            file(&[manifest(2)]),
            file(&[trial(0), trial(1)])
        );
        assert!(read_summary(middle.as_bytes()).is_err());
        assert!(read_summary(b"").is_err());
    }

    #[test]
    fn an_unreadable_middle_line_or_a_second_manifest_is_an_error() {
        let middle = format!("{}{{oops\n{}", file(&[manifest(2)]), file(&[trial(0)]));
        let err = read_run(middle.as_bytes()).unwrap_err();
        assert!(err.starts_with("line 2:"), "{err}");
        let twice = file(&[manifest(1), manifest(1)]);
        assert!(read_run(twice.as_bytes()).is_err());
        assert!(read_run(b"").is_err());
    }

    #[test]
    fn a_partial_line_before_a_failed_end_line_is_a_warning() {
        let end = Line::End(EndRecord {
            state: RunState::Failed,
            finished_unix_ms: 9,
        });
        let partial = &encode(&trial(1))[..30];
        let text = format!(
            "{}{partial}\n{}",
            file(&[manifest(2), trial(0)]),
            encode(&end)
        );
        let run = read_run(text.as_bytes()).unwrap();
        assert_eq!(run.trials.len(), 1);
        assert_eq!(run.end.unwrap().state, RunState::Failed);
        assert_eq!(run.warnings.len(), 1);
        assert!(run.warnings[0].contains("line 3"), "{:?}", run.warnings);

        // A glued end line (the old behavior) is unreadable, and a complete end does not excuse it.
        let complete = Line::End(EndRecord {
            state: RunState::Complete,
            finished_unix_ms: 9,
        });
        let text = format!(
            "{}{partial}\n{}",
            file(&[manifest(2), trial(0)]),
            encode(&complete)
        );
        assert!(read_run(text.as_bytes()).is_err());
    }

    #[test]
    fn a_partial_run_lists_a_candidate_with_no_trials() {
        let run = read_run(file(&[manifest(1)]).as_bytes()).unwrap();
        let text = run.to_string();
        assert!(
            text.lines()
                .any(|l| l.starts_with("candidate: c (") && l.ends_with("): no trials recorded")),
            "{text}"
        );
    }

    fn answered(body: &str) -> Attempt {
        Attempt {
            started_unix_ms: 0,
            latency_ms: 0,
            disposition: Disposition::Answered,
            http_status: Some(200),
            raw_response: Some(body.to_string()),
            error: None,
        }
    }

    fn graded_trial(attempts: Vec<Attempt>) -> TrialRecord {
        let Line::Trial(mut record) = trial(0) else {
            unreachable!()
        };
        record.targets = vec![Target::Haiku, Target::Sonnet, Target::Opus];
        record.labels =
            serde_json::from_value(json!({"sonnet": "low", "opus": "medium-high"})).unwrap();
        record.attempts = attempts;
        record
    }

    #[test]
    fn each_asked_target_is_graded_from_its_own_answer() {
        let body = r#"{"answers":{"effort_haiku":{"choice":"max"},"effort_sonnet":{"choice":"high"},"effort_opus":{"choice":"high"}}}"#;
        let record = graded_trial(vec![answered(body)]);
        assert_eq!(
            record.grades().unwrap(),
            [
                (Target::Haiku, Grade::Unscored),
                (Target::Sonnet, Grade::Over),
                (Target::Opus, Grade::Hit),
            ]
        );
        assert!(
            record
                .progress(1, 1)
                .contains("#1: haiku unscored · sonnet over · opus hit (")
        );
        // A target without an answer is invalid when labelled, unscored otherwise.
        let record = graded_trial(vec![answered(
            r#"{"answers":{"effort_sonnet":{"choice":"low"}}}"#,
        )]);
        assert_eq!(
            record.grades().unwrap(),
            [
                (Target::Haiku, Grade::Unscored),
                (Target::Sonnet, Grade::Hit),
                (Target::Opus, Grade::Invalid),
            ]
        );
    }

    #[test]
    fn a_failed_final_attempt_has_no_grades() {
        let mut failed = answered("busy");
        failed.disposition = Disposition::ProviderError;
        failed.http_status = Some(503);
        let record = graded_trial(vec![failed]);
        assert_eq!(record.grades(), None);
        assert!(
            record
                .progress(1, 1)
                .contains("#1: provider_error (HTTP 503): busy (")
        );
    }

    #[test]
    fn c1_controls_are_escaped_for_stderr_only() {
        let csi = "a\u{9b}31m\u{80}\u{9f}b\u{a0}";
        let escaped = escape_c1(csi);
        assert_eq!(escaped, "a\\u009b31m\\u0080\\u009fb\u{a0}");
        // The escaped text is valid JSON that decodes to the original.
        let decoded: String = serde_json::from_str(&format!("\"{escaped}\"")).unwrap();
        assert_eq!(decoded, csi);
    }

    #[test]
    fn only_schema_4_is_read() {
        for first in [
            r#"{"type":"run","schema_version":1}"#,
            r#"{"type":"run","schema_version":2}"#,
            r#"{"type":"run","schema_version":3}"#,
            r#"{"schema_version":99}"#,
            r#"{"type":"run"}"#,
        ] {
            let err = read_run(first.as_bytes()).unwrap_err();
            assert!(err.contains("unsupported schema_version"), "{err}");
        }
        let pretty = serde_json::to_string_pretty(&json!({"schema_version": 1})).unwrap();
        let err = read_run(pretty.as_bytes()).unwrap_err();
        assert!(err.contains("schema_version 1"), "{err}");
    }
}
