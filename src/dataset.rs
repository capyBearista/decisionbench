//! Source Cases from the original effortless benchmark fixture.
//!
//! Parsing only reads the source; the bytes are never rewritten. `current.effort`
//! is historical session metadata kept as provenance, never a judging input (ADR-0002).

use std::collections::{BTreeMap, HashSet};

use serde::Deserialize;

/// The original effortless fixture, unmodified (see `datasets/effortless/SOURCE.md`).
pub const EFFORTLESS_SOURCE: &[u8] = include_bytes!("../datasets/effortless/judge-cases.json");

/// Human-readable identity of [`EFFORTLESS_SOURCE`].
pub const EFFORTLESS_SOURCE_NAME: &str =
    "effortless judge-cases (capyBearista/effortless@99c03f222d9a71dd1e0d2fa5824eb1cb59bbb20c)";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TargetModel {
    Sonnet,
    Opus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    Low,
    Medium,
    High,
    Xhigh,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceDataset {
    pub about: String,
    pub cases: Vec<SourceCase>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceCase {
    pub id: String,
    pub kind: String,
    pub current: SessionState,
    pub message: String,
    #[serde(default)]
    pub context: Option<String>,
    /// Acceptable-effort set for `current.model`, as labelled in the source.
    pub ok: Vec<Effort>,
    #[serde(default)]
    pub holdout: bool,
}

/// Session state recorded with a Source Case: the labelled Target Model and its historical effort.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionState {
    pub model: TargetModel,
    pub effort: Effort,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Summary {
    pub cases: usize,
    pub sonnet_labels: usize,
    pub opus_labels: usize,
    pub sonnet_holdout: usize,
    pub opus_holdout: usize,
    pub categories: BTreeMap<String, usize>,
}

/// Parses and validates a source fixture.
pub fn parse_source(bytes: &[u8]) -> Result<SourceDataset, String> {
    let dataset: SourceDataset = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if dataset.cases.is_empty() {
        return Err("no cases".to_string());
    }
    let mut ids = HashSet::new();
    for case in &dataset.cases {
        if !ids.insert(case.id.as_str()) {
            return Err(format!("duplicate case id {}", case.id));
        }
        if case.ok.is_empty() {
            return Err(format!("case {}: empty acceptable-effort set", case.id));
        }
    }
    Ok(dataset)
}

impl SourceDataset {
    pub fn summary(&self) -> Summary {
        let mut s = Summary {
            cases: self.cases.len(),
            sonnet_labels: 0,
            opus_labels: 0,
            sonnet_holdout: 0,
            opus_holdout: 0,
            categories: BTreeMap::new(),
        };
        for case in &self.cases {
            let (labels, holdout) = match case.current.model {
                TargetModel::Sonnet => (&mut s.sonnet_labels, &mut s.sonnet_holdout),
                TargetModel::Opus => (&mut s.opus_labels, &mut s.opus_holdout),
            };
            *labels += 1;
            *holdout += usize::from(case.holdout);
            *s.categories.entry(case.kind.clone()).or_default() += 1;
        }
        s
    }
}

impl std::fmt::Display for Summary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "cases: {}", self.cases)?;
        writeln!(f, "sonnet labels: {}", self.sonnet_labels)?;
        writeln!(f, "opus labels: {}", self.opus_labels)?;
        writeln!(
            f,
            "holdout: {} (sonnet {}, opus {})",
            self.sonnet_holdout + self.opus_holdout,
            self.sonnet_holdout,
            self.opus_holdout
        )?;
        writeln!(f, "categories:")?;
        for (kind, count) in &self.categories {
            writeln!(f, "  {kind}: {count}")?;
        }
        Ok(())
    }
}
