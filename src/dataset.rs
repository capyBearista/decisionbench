//! The benchmark dataset: Cases with an acceptable-effort range per Target Model (ADR-0003).
//!
//! Cases carry no session state; current effort is never a judging input (ADR-0002).

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Deserializer, de};

/// The bundled dataset (`datasets/cases.json`).
pub const BUNDLED_CASES: &[u8] = include_bytes!("../datasets/cases.json");

/// Variant order is the effort order; range validation relies on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Effort {
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl Effort {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "low" => Self::Low,
            "medium" => Self::Medium,
            "high" => Self::High,
            "xhigh" => Self::Xhigh,
            "max" => Self::Max,
            _ => return None,
        })
    }
}

/// Contiguous acceptable efforts, written `"max"` or `"low-high"` (inclusive).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffortRange {
    pub from: Effort,
    pub to: Effort,
}

impl<'de> Deserialize<'de> for EffortRange {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        let (from, to) = text.split_once('-').unwrap_or((&text, &text));
        let effort = |name: &str| {
            Effort::parse(name).ok_or_else(|| de::Error::custom(format!("unknown effort `{name}`")))
        };
        let (from, to) = (effort(from)?, effort(to)?);
        if from > to {
            return Err(de::Error::custom(format!(
                "range `{text}` starts above its end"
            )));
        }
        Ok(Self { from, to })
    }
}

/// Labels per Target Model; an absent target is unscored.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Labels {
    pub sonnet: Option<EffortRange>,
    pub opus: Option<EffortRange>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    pub id: String,
    pub message: String,
    pub context: Option<String>,
    pub ok: Labels,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Summary {
    pub cases: usize,
    pub sonnet_labels: usize,
    pub opus_labels: usize,
}

/// Parses and validates a dataset file.
pub fn parse_cases(bytes: &[u8]) -> Result<Vec<Case>, String> {
    let cases: Vec<Case> = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if cases.is_empty() {
        return Err("no cases".to_string());
    }
    let mut ids = HashSet::new();
    let mut prompts = HashMap::new();
    for case in &cases {
        if case.id.is_empty() {
            return Err("case with empty id".to_string());
        }
        if case.ok.sonnet.is_none() && case.ok.opus.is_none() {
            return Err(format!("case {} has no labels", case.id));
        }
        if !ids.insert(case.id.as_str()) {
            return Err(format!("duplicate case id {}", case.id));
        }
        let prompt = (case.message.as_str(), case.context.as_deref());
        if let Some(first) = prompts.insert(prompt, case.id.as_str()) {
            return Err(format!(
                "case {} repeats the message and context of {first}",
                case.id
            ));
        }
    }
    Ok(cases)
}

pub fn summarize(cases: &[Case]) -> Summary {
    Summary {
        cases: cases.len(),
        sonnet_labels: cases.iter().filter(|c| c.ok.sonnet.is_some()).count(),
        opus_labels: cases.iter().filter(|c| c.ok.opus.is_some()).count(),
    }
}

impl std::fmt::Display for Summary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "cases: {}", self.cases)?;
        for (target, labels) in [("sonnet", self.sonnet_labels), ("opus", self.opus_labels)] {
            writeln!(
                f,
                "{target} labels: {labels} ({} unscored)",
                self.cases - labels
            )?;
        }
        Ok(())
    }
}
