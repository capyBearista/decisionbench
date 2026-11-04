# decisionbench

**Status: early implementation.** The Rust CLI can validate and summarize the bundled dataset offline; benchmarking is not implemented yet.

```sh
cargo run -- dataset inspect            # bundled dataset
cargo run -- dataset inspect FILE.json  # another dataset file
```

**decisionbench** is a planned standalone, terminal-first benchmark for **Decisions models**: specialized models asked to choose an appropriate reasoning-effort level for a target assistant. Its first benchmark evaluates effort-selection quality for **Claude Sonnet** and **Claude Opus** separately, not task-execution success.

## Initial product

- **Rust** benchmark engine shared by a **headless CLI** and **Ratatui TUI**.
- Initial provider routes: **OpenCode Zen** and **OpenRouter Decisions API**; the same underlying model exposed through different routes is compared as a distinct candidate.
- One versioned dataset (`datasets/cases.json`, 71 Cases) converted from the effortless benchmark cases.
- Independent, human-reviewed acceptable effort ranges for Sonnet and Opus, such as `"low-high"`. Missing counterparts remain **unscored**. Each evaluation is a separate API request, with three logical trials per combination by default.
- Accuracy and under/over-selection, reliability, latency, consistency, history and manually configured per-route cost reported **separately**. No composite leaderboard score.
- Raw predictions and confidence retained separately from any future production fallback/retention policy. **Current effort is not supplied as a judging anchor.**
- Local TOML configuration and versioned JSON/JSONL evidence. No web UI, mandatory service, Docker runtime, or coding-agent harness dependency.

The later [Valbench integration](https://github.com/capyBearista/decisionbench/issues/1) will be planned around the CLI and evidence export. HTTP APIs and packaging are deferred until required.

## Planning documents

- [Product and engineering specification](docs/SPEC.md): behavior, evidence, scoring, scope and testing decisions.
- [Domain glossary](GLOSSARY.md): canonical terminology for models, provider routes, cases, trials and evidence.
- [Architecture decisions](docs/adr/): why the engine is standalone, why the benchmark grades raw, model-relative predictions, and why labels are per-target effort ranges.
- [Issue tracker convention](docs/agents/issue-tracker.md) and [domain-documentation map](docs/agents/domain.md): how future implementation work should use these documents.

## Source attribution

The dataset is derived from the MIT-licensed [effortless project](https://github.com/capyBearista/effortless) by HeyCubit. Its license is in [datasets/LICENSE-effortless](datasets/LICENSE-effortless).

## Project license

[MIT](LICENSE).
