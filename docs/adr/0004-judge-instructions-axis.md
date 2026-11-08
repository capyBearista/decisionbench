# ADR-0004: Judge Instructions are a selectable, versioned axis

**Status:** Accepted (2026-11-03). Superseded in part by [ADR-0005](0005-one-request-per-case-four-targets.md): a Trial no longer includes a Target Model, and the file format now has one `## <target>` section per model, each with its own instructions and effort lines. Selection by name and hash is unchanged.

## Context

Every Decisions request carries instructions and a description of each effort level. That text shapes every prediction as much as the choice of Decision Model does. Effortless's text also contains policy rules ADR-0002 excludes (a one-step Opus/Sonnet equivalence, keeping the current effort on follow-ups), so decisionbench needs its own. The user wants to try several instruction texts and compare their accuracy side by side, per Decision Model, in the same way as Candidates.

## Decision

- **Judge Instructions** are a separate axis: a Trial is one Candidate × Judge Instructions × Case × Target Model × repetition. Any Candidate can be run with any instruction set.
- Each set is a **Markdown file**, `prompts/<name>.md`, versioned in the repository: the instructions first, then one `## <effort>` section per effort level in scale order. A default set, `neutral`, is bundled into the binary; another is selected by path.
- A set is identified by its **name** (the file stem) and the **SHA-256 of the file**. Evidence stores both and the full text, so an edited file under the same name is a different revision and reports can warn when revisions are mixed.
- Sets are selected, never edited, through the CLI or TUI.

## Alternatives considered

**Instructions as part of the Candidate:** rejected. It mixes the prompt into Provider Route identity and needs a Candidate per prompt to compare prompts.

**Instructions in the TOML configuration:** rejected. Long text is awkward to write and review in TOML, and the configuration is local to one machine rather than versioned with the repository.

**Editing instructions in the CLI or TUI:** rejected. Files under version control are easier to review and reproduce.

**Identify by name only:** rejected. A file edited in place would be silently compared with its earlier version.

## Consequences

The number of Trials in a Run grows by one factor per selected instruction set (#8). Reports break out accuracy by Candidate × Judge Instructions and warn when one name has several hashes (#9). Prompt experiments are reproducible from the repository and from evidence alone. [ADR-0006](0006-prompts-as-a-comparison-axis.md) extends the side-by-side comparison from sets inside one Run to sets across Runs.
