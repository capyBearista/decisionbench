//! The benchmark dataset: Cases with an acceptable-effort range per Target Model (ADR-0003).
//!
//! Cases carry no session state; current effort is never a judging input (ADR-0002).

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

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
    /// Every effort, in scale order.
    pub const ALL: [Self; 5] = [Self::Low, Self::Medium, Self::High, Self::Xhigh, Self::Max];

    pub fn name(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }

    pub(crate) fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|e| e.name() == name)
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

impl std::fmt::Display for EffortRange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.from == self.to {
            f.write_str(self.from.name())
        } else {
            write!(f, "{}-{}", self.from.name(), self.to.name())
        }
    }
}

impl Serialize for EffortRange {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// Labels per Target Model; an absent target is unscored. `null` is rejected, so omitting the key is
/// the only way to leave a target unscored.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Labels {
    #[serde(default, deserialize_with = "present")]
    pub sonnet: Option<EffortRange>,
    #[serde(default, deserialize_with = "present")]
    pub opus: Option<EffortRange>,
}

/// Only called for keys that are present, so `None` here means an explicit `null`.
fn present<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<EffortRange>, D::Error> {
    match Option::<EffortRange>::deserialize(deserializer)? {
        Some(range) => Ok(Some(range)),
        None => Err(de::Error::custom(
            "`null` label; omit the key to leave the target unscored",
        )),
    }
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
