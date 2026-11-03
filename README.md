# decisionbench

**Status: design/specification phase.** There is no Rust implementation or runnable binary in this repository yet.

**decisionbench** is a planned standalone, terminal-first benchmark for **Decisions models**: specialized models asked to choose an appropriate reasoning-effort level for a target assistant. Its first benchmark evaluates effort-selection quality for **Claude Sonnet** and **Claude Opus** separately, not task-execution success.

## Initial product

- **Rust** benchmark engine shared by a **headless CLI** and **Ratatui TUI**.
- Initial provider routes: **OpenCode Zen** and **OpenRouter Decisions API**; the same underlying model exposed through different routes is compared as a distinct candidate.
- Versioned dataset starting from the original 75 [effortless labelled cases](https://github.com/capyBearista/effortless/blob/main/bench/judge-cases.json), preserving 20 holdouts and source provenance.
- Independent, human-reviewed acceptable effort levels for Sonnet and Opus. Missing counterparts remain **unscored**. Each evaluation is a separate API request, with three logical trials per combination by default.
- Accuracy and under/over-selection, reliability, latency, consistency, history and manually configured per-route cost reported **separately**. No composite leaderboard score.
- Raw predictions and confidence retained separately from any future production fallback/retention policy. **Current effort is not supplied as a judging anchor.**
- Local TOML configuration and versioned JSON/JSONL evidence. No web UI, mandatory service, Docker runtime, or coding-agent harness dependency.

The later [Valbench integration](https://github.com/capyBearista/decisionbench/issues/1) will be planned around the CLI and evidence export. HTTP APIs and packaging are deferred until required.

## Planning documents

- [Product and engineering specification](docs/SPEC.md): behavior, evidence, scoring, scope and testing decisions.
- [Domain glossary](GLOSSARY.md): canonical terminology for models, provider routes, cases, trials and evidence.
- [Architecture decisions](docs/adr/): why the engine is standalone and why the benchmark grades raw, model-relative predictions.
- [Issue tracker convention](docs/agents/issue-tracker.md) and [domain-documentation map](docs/agents/domain.md): how future implementation work should use these documents.

## Source attribution

The initial dataset will be derived from the MIT-licensed [effortless project](https://github.com/capyBearista/effortless) by HeyCubit. Preserve the original fixture and copyright/license notice when importing it. The source dataset has **75 cases, 20 holdouts**, and includes labelled Sonnet/Opus examples.

## Project license

[MIT](LICENSE).
