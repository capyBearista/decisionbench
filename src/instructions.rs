//! Judge Instructions: named, hashed Markdown files sent with every request (ADR-0004).

use crate::dataset::Effort;
use crate::grade::Target;
use crate::sha256_hex;

/// The default set, bundled into the binary.
pub const BUNDLED_NAME: &str = "neutral";
pub const BUNDLED_TEXT: &str = include_str!("../prompts/neutral.md");

/// The question asked about one Target Model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetQuestion {
    pub instructions: String,
    /// One description per effort, in scale order.
    pub criteria: Vec<(Effort, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JudgeInstructions {
    /// The file stem.
    pub name: String,
    /// SHA-256 of the exact file text.
    pub sha256: String,
    /// The exact file text.
    pub text: String,
    /// The text before the first `## ` heading; sent as `state.task`.
    pub task: String,
    /// The target sections present, in `Target::ALL` order.
    pub targets: Vec<(Target, TargetQuestion)>,
}

impl JudgeInstructions {
    pub fn question(&self, target: Target) -> Option<&TargetQuestion> {
        self.targets
            .iter()
            .find(|(t, _)| *t == target)
            .map(|(_, question)| question)
    }
}

/// Parses one target section: an instructions paragraph, then exactly one `- <effort>: <text>`
/// line per effort, in scale order.
fn parse_question(body: &str) -> Result<TargetQuestion, String> {
    let mut lines = body.trim().lines();
    let instructions: Vec<&str> = lines
        .by_ref()
        .take_while(|line| !line.trim().is_empty())
        .collect();
    // A first line that is already a criterion means the paragraph is missing.
    if instructions
        .first()
        .is_none_or(|line| line.starts_with("- "))
    {
        return Err("no instructions".to_string());
    }
    let rest: Vec<&str> = lines.skip_while(|line| line.trim().is_empty()).collect();
    let mut criteria = Vec::new();
    for (i, effort) in Effort::ALL.iter().enumerate() {
        let prefix = format!("- {}:", effort.name());
        let Some(line) = rest.get(i) else {
            return Err(format!("missing `{prefix}`"));
        };
        let Some(text) = line.strip_prefix(&prefix) else {
            return Err(format!("expected `{prefix}`, found `{line}`"));
        };
        if text.trim().is_empty() {
            return Err(format!("`{prefix}` is empty"));
        }
        criteria.push((*effort, text.trim().to_string()));
    }
    if let Some(extra) = rest.get(Effort::ALL.len()) {
        return Err(format!("unexpected line after `- max:`: `{extra}`"));
    }
    Ok(TargetQuestion {
        instructions: instructions.join("\n"),
        criteria,
    })
}

/// Parses a Judge Instructions file: the task text, then one `## <target>` section per Target Model
/// asked about, in `Target::ALL` order (a target may be left out).
pub fn parse_instructions(name: &str, text: &str) -> Result<JudgeInstructions, String> {
    let mut intro = String::new();
    let mut sections: Vec<(&str, String)> = Vec::new();
    for line in text.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            sections.push((heading.trim(), String::new()));
        } else {
            let body = sections.last_mut().map_or(&mut intro, |(_, body)| body);
            body.push_str(line);
            body.push('\n');
        }
    }
    let task = intro.trim();
    if task.is_empty() {
        return Err("no task text before the first `## `".to_string());
    }
    let mut targets: Vec<(Target, TargetQuestion)> = Vec::new();
    for (heading, body) in &sections {
        let target = Target::ALL
            .into_iter()
            .find(|t| t.to_string() == *heading)
            .ok_or_else(|| format!("unknown section `## {heading}`"))?;
        match targets.last() {
            Some((last, _)) if *last == target => {
                return Err(format!("duplicate section `## {heading}`"));
            }
            Some((last, _)) if *last > target => {
                return Err(format!("`## {heading}` comes after `## {last}`"));
            }
            _ => {}
        }
        let question = parse_question(body).map_err(|e| format!("`## {heading}`: {e}"))?;
        targets.push((target, question));
    }
    if targets.is_empty() {
        return Err("no target sections".to_string());
    }
    Ok(JudgeInstructions {
        name: name.to_string(),
        sha256: sha256_hex(text.as_bytes()),
        text: text.to_string(),
        task: task.to_string(),
        targets,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

    fn criteria() -> String {
        EFFORTS
            .iter()
            .map(|e| format!("- {e}: {e} desc\n"))
            .collect()
    }

    fn section(target: &str) -> String {
        format!("\n## {target}\nAsk about {target}?\n\n{}", criteria())
    }

    fn file(task: &str, targets: &[&str]) -> String {
        let sections: String = targets.iter().map(|t| section(t)).collect();
        format!("{task}\n{sections}")
    }

    #[test]
    fn the_shipped_files_parse_with_all_four_targets() {
        for (name, text) in [
            ("effortless", include_str!("../prompts/effortless.md")),
            ("tuned", include_str!("../prompts/tuned.md")),
        ] {
            let set = parse_instructions(name, text).unwrap();
            let order: Vec<Target> = set.targets.iter().map(|(t, _)| *t).collect();
            assert_eq!(order, Target::ALL, "{name}");
        }
    }

    #[test]
    fn bundled_set_parses() {
        let set = parse_instructions(BUNDLED_NAME, BUNDLED_TEXT).unwrap();
        assert_eq!(set.name, "neutral");
        assert_eq!(set.text, BUNDLED_TEXT);
        assert_eq!(set.sha256, sha256_hex(BUNDLED_TEXT.as_bytes()));
        assert!(set.task.starts_with("Choose the reasoning effort"));
        assert!(!set.task.contains("##"));
        let order: Vec<Target> = set.targets.iter().map(|(t, _)| *t).collect();
        assert_eq!(order, Target::ALL);
        let names = ["Haiku 5.5", "Sonnet 5.5", "Opus 5.5", "Fable 5.1"];
        for ((_, question), name) in set.targets.iter().zip(names) {
            assert_eq!(
                question.instructions,
                format!("Which effort should Claude {name} use for the next message?")
            );
            let efforts: Vec<Effort> = question.criteria.iter().map(|(e, _)| *e).collect();
            assert_eq!(efforts, Effort::ALL);
            assert_eq!(
                question.criteria[2].1,
                "hard or large multi-step job: many tool calls, mistakes are costly"
            );
        }
    }

    #[test]
    fn a_file_may_omit_targets_but_keeps_the_order() {
        let set = parse_instructions("x", &file("Task.", &["sonnet", "fable"])).unwrap();
        assert!(set.question(Target::Sonnet).is_some());
        assert!(set.question(Target::Fable).is_some());
        assert!(set.question(Target::Haiku).is_none());
        assert!(set.question(Target::Opus).is_none());
    }

    #[test]
    fn keeps_multiline_text_and_trims_edges() {
        let text = format!(
            "  Task one.\n\nTask two.  \n\n## opus\nline one\nline two\n\n{}",
            criteria().replace("- low: low desc", "- low:   padded  ")
        );
        let set = parse_instructions("x", &text).unwrap();
        assert_eq!(set.task, "Task one.\n\nTask two.");
        let question = set.question(Target::Opus).unwrap();
        assert_eq!(question.instructions, "line one\nline two");
        assert_eq!(question.criteria[0], (Effort::Low, "padded".to_string()));
    }

    #[test]
    fn rejects_malformed_files() {
        let no_criteria = "T.\n\n## opus\nAsk?\n";
        let no_instructions = format!("T.\n\n## opus\n{}", criteria());
        let missing_max = criteria().replace("- max: max desc\n", "");
        let swapped = criteria().replace(
            "- low: low desc\n- medium: medium desc",
            "- medium: medium desc\n- low: low desc",
        );
        for (text, fragment) in [
            (
                file("   ", &["opus"]),
                "no task text before the first `## `",
            ),
            ("Task.\n".to_string(), "no target sections"),
            (file("T.", &["gemini"]), "unknown section `## gemini`"),
            (
                file("T.", &["opus", "sonnet"]),
                "`## sonnet` comes after `## opus`",
            ),
            (file("T.", &["opus", "opus"]), "duplicate section `## opus`"),
            (file("T.", &["low"]), "unknown section `## low`"),
            (no_criteria.to_string(), "`## opus`: missing `- low:`"),
            (no_instructions, "`## opus`: no instructions"),
            (
                format!("T.\n\n## opus\nAsk?\n\n{missing_max}"),
                "`## opus`: missing `- max:`",
            ),
            (
                format!("T.\n\n## opus\nAsk?\n\n{swapped}"),
                "`## opus`: expected `- low:`, found `- medium: medium desc`",
            ),
            (
                format!(
                    "T.\n\n## opus\nAsk?\n\n{}",
                    criteria().replace("- max: max desc", "- max:  ")
                ),
                "`## opus`: `- max:` is empty",
            ),
            (
                format!("T.\n\n## opus\nAsk?\n\n{}- extra: x\n", criteria()),
                "`## opus`: unexpected line after `- max:`: `- extra: x`",
            ),
        ] {
            let err = parse_instructions("bad", &text).unwrap_err();
            assert!(
                err.contains(fragment),
                "{fragment:?} not in {err:?}\n{text}"
            );
        }
    }
}
