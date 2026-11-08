//! The benchmark dataset: Cases with an acceptable-effort range per Target Model (ADR-0003).
//!
//! Cases carry no session state; current effort is never a judging input (ADR-0002).

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::grade::Target;

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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Labels {
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub haiku: Option<EffortRange>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub sonnet: Option<EffortRange>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub opus: Option<EffortRange>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub fable: Option<EffortRange>,
}

impl Labels {
    pub fn get(&self, target: Target) -> Option<EffortRange> {
        match target {
            Target::Haiku => self.haiku,
            Target::Sonnet => self.sonnet,
            Target::Opus => self.opus,
            Target::Fable => self.fable,
        }
    }
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

/// Which part of the dataset a Case belongs to (ADR-0007). Prompt tuning runs on `Train`; `Holdout`
/// is scored once, at the end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Split {
    Train,
    Holdout,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    pub id: String,
    pub message: String,
    pub context: Option<String>,
    pub split: Split,
    pub ok: Labels,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Summary {
    pub cases: usize,
    /// Cases in `Split::Train` and in `Split::Holdout`.
    pub splits: [usize; 2],
    /// Labelled Cases per Target Model, in `Target::ALL` order.
    pub labels: [usize; Target::ALL.len()],
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
        if Target::ALL.iter().all(|t| case.ok.get(*t).is_none()) {
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
        splits: [Split::Train, Split::Holdout]
            .map(|split| cases.iter().filter(|c| c.split == split).count()),
        labels: Target::ALL.map(|t| cases.iter().filter(|c| c.ok.get(t).is_some()).count()),
    }
}

impl std::fmt::Display for Summary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "cases: {}", self.cases)?;
        writeln!(
            f,
            "split: {} train, {} holdout",
            self.splits[0], self.splits[1]
        )?;
        for (target, labels) in Target::ALL.iter().zip(self.labels) {
            writeln!(
                f,
                "{target} labels: {labels} ({} unscored)",
                self.cases - labels
            )?;
        }
        Ok(())
    }
}
