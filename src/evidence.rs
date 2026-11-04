//! Immutable evidence: one JSON file per evaluation, readable offline (SPEC §5).

use std::fmt;
use std::fs::{DirBuilder, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::dataset::EffortRange;
use crate::grade::{Grade, Target, grade};
use crate::provider::{Attempt, Candidate, Prediction};

pub const SCHEMA_VERSION: u32 = 1;
/// Bump when grading semantics change, so old evidence stays interpretable.
pub const GRADING_REVISION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
pub struct Hashed {
    pub name: String,
    pub sha256: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct InstructionsRecord {
    pub name: String,
    pub sha256: String,
    pub text: String,
}

/// Everything about one evaluation. The grade is not stored: it is recomputed from `label` and the
/// raw effort, so regrading never changes evidence. The request never holds headers or keys.
#[derive(Debug, Serialize, Deserialize)]
pub struct Evidence {
    pub schema_version: u32,
    /// `{unix_ms}-{pid}`; also the file name.
    pub id: String,
    pub created_unix_ms: u64,
    pub dataset: Hashed,
    pub instructions: InstructionsRecord,
    pub grading_revision: u32,
    pub candidate: Candidate,
    /// The URL the request actually went to (route identity).
    pub url: String,
    pub case_id: String,
    pub target: Target,
    /// The label as it was when the request was made.
    pub label: Option<EffortRange>,
    pub request: Value,
    pub attempt: Attempt,
    /// Present only when the attempt was answered.
    pub prediction: Option<Prediction>,
}

impl Evidence {
    /// `None` when there is no reply to grade.
    pub fn grade(&self) -> Option<Grade> {
        self.prediction
            .as_ref()
            .map(|p| grade(self.label, &p.effort))
    }
}

/// Creates `dir` (mode 0700) and opens `{id}.json` (mode 0600) for writing, never overwriting.
/// Done before the request is sent, so an unwritable destination fails with nothing sent.
pub fn create(dir: &Path, id: &str) -> Result<(File, PathBuf), String> {
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let path = dir.join(format!("{id}.json"));
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok((file, path))
}

/// Writes the evidence into the file `create` opened. If that fails the reply is already paid for,
/// so the full JSON goes to stderr rather than being lost.
pub fn save(mut file: File, path: &Path, evidence: &Evidence) -> Result<(), String> {
    let text = serde_json::to_string_pretty(evidence).map_err(|e| e.to_string())?;
    file.write_all(text.as_bytes()).map_err(|e| {
        eprintln!("{text}");
        format!("cannot write {}: {e}", path.display())
    })
}

pub fn read(bytes: &[u8]) -> Result<Evidence, String> {
    let value: Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if value.get("schema_version") != Some(&Value::from(SCHEMA_VERSION)) {
        let found = value
            .get("schema_version")
            .map_or_else(|| "missing".to_string(), Value::to_string);
        return Err(format!(
            "unsupported schema_version {found} (this build reads {SCHEMA_VERSION})"
        ));
    }
    serde_json::from_value(value).map_err(|e| e.to_string())
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

/// The summary `evaluate` and `show` both print.
impl fmt::Display for Evidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let c = &self.candidate;
        writeln!(
            f,
            "candidate: {} ({} {})",
            c.name,
            c.provider.name(),
            c.model
        )?;
        let short: String = self.instructions.sha256.chars().take(12).collect();
        writeln!(f, "instructions: {} ({short})", self.instructions.name)?;
        match self.label {
            Some(range) => writeln!(f, "case: {} {} (label {range})", self.case_id, self.target)?,
            None => writeln!(f, "case: {} {} (unlabelled)", self.case_id, self.target)?,
        }
        let a = &self.attempt;
        write!(f, "attempt: {}", a.disposition.name())?;
        if let Some(status) = a.http_status {
            write!(f, ", HTTP {status}")?;
        }
        writeln!(f, ", {} ms", a.latency_ms)?;
        match &self.prediction {
            Some(p) => writeln!(f, "effort: {}", p.effort)?,
            None => writeln!(f, "error: {}", failure_text(a))?,
        }
        match self.grade() {
            Some(g) => writeln!(f, "grade: {g}"),
            None => writeln!(f, "grade: none"),
        }
    }
}
