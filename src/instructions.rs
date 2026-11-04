//! Judge Instructions: named, hashed Markdown files sent with every request (ADR-0004).

use crate::dataset::Effort;
use crate::sha256_hex;

/// The default set, bundled into the binary.
pub const BUNDLED_NAME: &str = "neutral";
pub const BUNDLED_TEXT: &str = include_str!("../prompts/neutral.md");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JudgeInstructions {
    /// The file stem.
    pub name: String,
    /// SHA-256 of the exact file text.
    pub sha256: String,
    /// The exact file text.
    pub text: String,
    /// The text before the first `## ` heading.
    pub instructions: String,
    /// One description per effort, in scale order.
    pub criteria: Vec<(Effort, String)>,
}

/// Parses a Judge Instructions file: instructions, then one `## <effort>` section per effort in scale order.
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
    let instructions = intro.trim();
    if instructions.is_empty() {
        return Err("no instructions before the first `## `".to_string());
    }
    let mut criteria = Vec::new();
    for (i, (heading, body)) in sections.iter().enumerate() {
        match Effort::ALL.get(i) {
            Some(effort) if effort.name() == *heading => {
                if body.trim().is_empty() {
                    return Err(format!("`## {heading}` is empty"));
                }
                criteria.push((*effort, body.trim().to_string()));
            }
            Some(effort) if Effort::parse(heading).is_some() => {
                return Err(format!(
                    "expected `## {}`, found `## {heading}`",
                    effort.name()
                ));
            }
            _ => return Err(format!("unexpected section `## {heading}`")),
        }
    }
    if let Some(effort) = Effort::ALL.get(criteria.len()) {
        return Err(format!("missing `## {}`", effort.name()));
    }
    Ok(JudgeInstructions {
        name: name.to_string(),
        sha256: sha256_hex(text.as_bytes()),
        text: text.to_string(),
        instructions: instructions.to_string(),
        criteria,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

    fn file(intro: &str, sections: &[(&str, &str)]) -> String {
        let mut text = format!("{intro}\n");
        for (heading, body) in sections {
            text.push_str(&format!("\n## {heading}\n{body}\n"));
        }
        text
    }

    fn all_sections() -> Vec<(&'static str, &'static str)> {
        EFFORTS.iter().map(|e| (*e, "desc")).collect()
    }

    #[test]
    fn bundled_set_parses() {
        let set = parse_instructions(BUNDLED_NAME, BUNDLED_TEXT).unwrap();
        assert_eq!(set.name, "neutral");
        assert_eq!(set.text, BUNDLED_TEXT);
        assert_eq!(set.sha256, sha256_hex(BUNDLED_TEXT.as_bytes()));
        assert!(set.instructions.starts_with("Choose the reasoning effort"));
        assert!(!set.instructions.contains("##"));
        let order: Vec<Effort> = set.criteria.iter().map(|(e, _)| *e).collect();
        assert_eq!(order, Effort::ALL);
        assert_eq!(
            set.criteria[2].1,
            "hard or large multi-step job: many tool calls, mistakes are costly"
        );
    }

    #[test]
    fn keeps_multiline_text_and_trims_edges() {
        let mut sections = all_sections();
        sections[0] = ("low", "line one\nline two");
        let set =
            parse_instructions("x", &file("  Intro one.\n\nIntro two.  ", &sections)).unwrap();
        assert_eq!(set.instructions, "Intro one.\n\nIntro two.");
        assert_eq!(
            set.criteria[0],
            (Effort::Low, "line one\nline two".to_string())
        );
    }

    #[test]
    fn rejects_malformed_files() {
        let mut missing = all_sections();
        missing.remove(3);
        let mut swapped = all_sections();
        swapped.swap(0, 1);
        let mut extra = all_sections();
        extra.push(("notes", "x"));
        let mut empty_section = all_sections();
        empty_section[4] = ("max", "  ");
        for (text, fragment) in [
            (
                file("Intro.", &missing),
                "expected `## xhigh`, found `## max`",
            ),
            (
                file("Intro.", &swapped),
                "expected `## low`, found `## medium`",
            ),
            (file("Intro.", &extra), "unexpected section `## notes`"),
            (file("Intro.", &all_sections()[..4]), "missing `## max`"),
            (file("Intro.", &empty_section), "`## max` is empty"),
            (
                file("   ", &all_sections()),
                "no instructions before the first `## `",
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
