# decisionbench

**Status: early implementation.** The Rust CLI can validate and summarize the bundled dataset, grade local effort predictions offline, and send one Decisions request per Case and Target Model to OpenRouter or OpenCode Zen, saving the raw reply as evidence. Batch runs, retries and reports are not implemented yet.

```sh
cargo run -- dataset inspect            # bundled dataset
cargo run -- dataset inspect FILE.json  # another dataset file
cargo run -- grade PREDICTIONS.json     # grade against the bundled dataset (or --dataset FILE.json)
cargo run -- evaluate --candidate jev-openrouter --case c002 --target opus
cargo run -- show ~/.local/share/decisionbench/runs/FILE.json
```

## Sending a request

A Candidate pairs a Decision Model with a Provider Route. Define Candidates in `$XDG_CONFIG_HOME/decisionbench/config.toml` (default `~/.config/decisionbench/config.toml`, or pass `--config FILE`):

```toml
[[candidate]]
name = "jev-openrouter"            # your label; must be unique
decision_model = "jev-1.13"        # optional: the underlying model
provider = "openrouter"            # "openrouter" or "zen"
model = "typesafe/jev-1.13"        # the model id sent on the wire

[[candidate]]
name = "jev-zen-free"
decision_model = "jev-1.13"
provider = "zen"
model = "jev-1.13-free"
# url = "..."                      # optional: override the provider's default URL
```

Credentials come from environment variables only and are never written to evidence: `OPENROUTER_API_KEY` (required for `openrouter`) and `OPENCODE_API_KEY` (optional for `zen`; free models work without one, and no `Authorization` header is sent when it is unset or empty). Requests honour the standard `HTTPS_PROXY`/`HTTP_PROXY`/`ALL_PROXY` environment variables; a candidate `url` override that uses plain `http://` sends the key unencrypted.

`evaluate` sends the Case message, its context if any, and the Target Model (`--target sonnet` or `opus`). It never sends current effort or current model. It saves one JSON file per request under `$XDG_DATA_HOME/decisionbench/runs` (default `~/.local/share/decisionbench/runs`, or `--out DIR`) and prints a summary. Exit codes: `0` answered (the effort may still grade as invalid), `3` provider or transport failure (evidence still saved), `1` bad config, input or missing key (nothing sent or saved), `2` usage error.

`show FILE` prints the same summary from a saved file, offline. The grade is recomputed from the stored label and raw effort, never stored.

**Evidence files contain the full prompt text, the Case message and context, and the raw provider reply.** Review them before sharing.

### Judge Instructions

The instructions and per-effort descriptions sent with every request live in a Markdown file. The bundled default is [`prompts/neutral.md`](prompts/neutral.md); pass another with `--instructions PATH`. The file name (without `.md`) is the set's name. The format is the instructions first, then one `## <effort>` section per effort in the order `low`, `medium`, `high`, `xhigh`, `max`, none empty:

```markdown
Choose the reasoning effort ... for the target model.

## low
quick answer or trivial action
## medium
...
## max
...
```

Evidence records the set's name, SHA-256 and full text.

## Offline grading

A predictions file is a JSON array such as `[{"case":"c001","target":"sonnet","effort":"high"}]`. A Case with no label for that target grades as unscored, whatever the effort. Otherwise only an exact effort name (`low`, `medium`, `high`, `xhigh`, `max`) is valid; anything else, or a missing `effort`, grades as invalid. The raw effort is printed as re-serialized JSON. Accuracy counts hits among valid, scored predictions.

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
