//! Offline grading of raw effort predictions against per-target ranges (SPEC §6).

use std::collections::HashMap;
use std::fmt;

use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::dataset::{Case, Effort, EffortRange};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Target {
    Haiku,
    Sonnet,
    Opus,
    Fable,
}

impl Target {
    /// Every Target Model, in the order requests list their questions.
    pub const ALL: [Self; 4] = [Self::Haiku, Self::Sonnet, Self::Opus, Self::Fable];
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Haiku => "haiku",
            Self::Sonnet => "sonnet",
            Self::Opus => "opus",
            Self::Fable => "fable",
        })
    }
}

/// One raw effort prediction for a Case and Target Model.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prediction {
    pub case: String,
    pub target: Target,
    /// The raw effort exactly as observed; a missing key reads as `null` (no effort given).
    #[serde(default)]
    pub effort: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grade {
    Hit,
    Under,
    Over,
    Invalid,
    Unscored,
}

/// Grades a raw effort. Only an exact effort name is valid; there is no normalization.
/// Unscored comes first: an unlabelled target is outside every scored count, so `invalid` means
/// "labelled but no valid effort".
pub fn grade(label: Option<EffortRange>, raw: &Value) -> Grade {
    let Some(range) = label else {
        return Grade::Unscored;
    };
    match raw.as_str().and_then(Effort::parse) {
        None => Grade::Invalid,
        Some(effort) if effort < range.from => Grade::Under,
        Some(effort) if effort > range.to => Grade::Over,
        Some(_) => Grade::Hit,
    }
}

impl fmt::Display for Grade {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Hit => "hit",
            Self::Under => "under",
            Self::Over => "over",
            Self::Invalid => "invalid",
            Self::Unscored => "unscored",
        })
    }
}

/// Parses a predictions file: a non-empty JSON array of `{case, target, effort}`.
pub fn parse_predictions(bytes: &[u8]) -> Result<Vec<Prediction>, String> {
    let predictions: Vec<Prediction> = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if predictions.is_empty() {
        return Err("no predictions".to_string());
    }
    Ok(predictions)
}

/// Grades each prediction in order. An unknown Case is an error, not unscored.
pub fn grade_all(cases: &[Case], predictions: &[Prediction]) -> Result<Vec<Grade>, String> {
    let by_id: HashMap<&str, &Case> = cases.iter().map(|c| (c.id.as_str(), c)).collect();
    predictions
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let case = by_id
                .get(p.case.as_str())
                .ok_or_else(|| format!("prediction {} names unknown case {}", i + 1, p.case))?;
            Ok(grade(case.ok.get(p.target), &p.effort))
        })
        .collect()
}

/// Grade counts for one Target Model.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub hit: usize,
    pub under: usize,
    pub over: usize,
    pub invalid: usize,
    pub unscored: usize,
}

impl Tally {
    pub fn add(&mut self, grade: Grade) {
        *match grade {
            Grade::Hit => &mut self.hit,
            Grade::Under => &mut self.under,
            Grade::Over => &mut self.over,
            Grade::Invalid => &mut self.invalid,
            Grade::Unscored => &mut self.unscored,
        } += 1;
    }
}

impl fmt::Display for Tally {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let valid_scored = self.hit + self.under + self.over;
        write!(
            f,
            "{} predictions: {} hit, {} under, {} over, {} invalid, {} unscored; accuracy {}/{valid_scored} valid scored",
            valid_scored + self.invalid + self.unscored,
            self.hit,
            self.under,
            self.over,
            self.invalid,
            self.unscored,
            self.hit,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Effort::*;
    use Grade::*;
    use serde_json::json;

    fn range(from: Effort, to: Effort) -> Option<EffortRange> {
        Some(EffortRange { from, to })
    }

    #[test]
    fn grades_every_effort_against_a_middle_range() {
        let label = range(Medium, High);
        for (raw, expected) in [
            ("low", Under),
            ("medium", Hit),
            ("high", Hit),
            ("xhigh", Over),
            ("max", Over),
        ] {
            assert_eq!(grade(label, &json!(raw)), expected, "{raw}");
        }
    }

    #[test]
    fn grades_single_level_ranges_at_both_ends_of_the_scale() {
        for (label, raw, expected) in [
            (range(Max, Max), "max", Hit),
            (range(Max, Max), "xhigh", Under),
            (range(Low, Low), "low", Hit),
            (range(Low, Low), "medium", Over),
            (range(High, High), "high", Hit),
            (range(Low, Max), "low", Hit),
            (range(Low, Max), "max", Hit),
        ] {
            assert_eq!(grade(label, &json!(raw)), expected, "{label:?} {raw}");
        }
    }

    #[test]
    fn missing_label_is_unscored() {
        assert_eq!(grade(None, &json!("high")), Unscored);
    }

    #[test]
    fn anything_but_an_exact_effort_name_is_invalid_when_labelled() {
        for raw in [
            json!("High"),
            json!(" high"),
            json!(""),
            json!("none"),
            json!("low-high"),
            json!(null),
            json!(3),
            json!(["high"]),
        ] {
            assert_eq!(grade(range(Low, Max), &raw), Invalid, "{raw}");
            assert_eq!(grade(None, &raw), Unscored, "{raw}");
        }
    }
}
