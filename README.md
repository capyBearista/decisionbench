# decisionbench

**Status: early implementation.** The Rust CLI can validate and summarize the bundled dataset, grade local effort predictions offline, and run batches of Decisions requests (Candidate × Judge Instructions × Case × Target Model × repetition) through OpenRouter or OpenCode Zen with retries, a concurrency limit and safe cancellation, saving every Attempt as evidence with a manually priced cost estimate. Detailed reports (latency, consistency, history) are not implemented yet.

```sh
cargo run -- dataset inspect            # bundled dataset
cargo run -- dataset inspect FILE.json  # another dataset file
cargo run -- grade PREDICTIONS.json     # grade against the bundled dataset (or --dataset FILE.json)
cargo run -- run --candidate jev-openrouter --case c002 --target opus --repetitions 1   # smoke test
cargo run -- run --candidate jev-openrouter --candidate jev-zen-free                     # full run
cargo run -- show ~/.local/share/decisionbench/runs/FILE.jsonl
```

## Sending a request

A Candidate pairs a Decision Model with a Provider Route. Define Candidates in `$XDG_CONFIG_HOME/decisionbench/config.toml` (default `~/.config/decisionbench/config.toml`, or pass `--config FILE`):

```toml
[[candidate]]
name = "jev-openrouter"            # your label; must be unique
decision_model = "jev-1.13"        # optional: the underlying model
provider = "openrouter"            # "openrouter" or "zen"
model = "typesafe/jev-1.13"        # the model id sent on the wire

[candidate.price]                  # optional; USD, captured with each run
input_per_million = 0.04           # required in the table
output_per_million = 0.0           # optional, default 0
per_request = 0.0                  # optional fixed fee, default 0

[[candidate]]
name = "jev-zen-free"
decision_model = "jev-1.13"
provider = "zen"
model = "jev-1.13-free"
```

Credentials come from environment variables only and are never written to evidence: `OPENROUTER_API_KEY` (required for `openrouter`) and `OPENCODE_API_KEY` (optional for `zen`; free models work without one, and no `Authorization` header is sent when it is unset or empty). Requests always go to the provider's own URL and honour the standard `HTTPS_PROXY`/`HTTP_PROXY`/`ALL_PROXY`/`NO_PROXY` environment variables. The tests redirect every request, API key included, to a local stub with `DECISIONBENCH_TEST_BASE_URL`; never set it outside tests.

### Running a benchmark

`run` sends one independent request per Trial. Each request carries the Case message, its context if any, and the Target Model. It never sends current effort or current model.

| Flag | Default | Meaning |
|---|---|---|
| `--candidate NAME` | required | Candidate from the config; repeatable |
| `--instructions PATH` | bundled `neutral` | Judge Instructions file; repeatable |
| `--case ID` | every Case | repeatable |
| `--target sonnet\|opus` | both | repeatable |
| `--repetitions N` | 3 | independent Trials per combination |
| `--concurrency N` | 1 | Trials in flight at once |
| `--timeout SECS` | 60 | per request |
| `--retries N` | 1 | extra Attempts after a transport error or HTTP 408, 429, 500, 502, 503, 504, 524 or 529, after a 1 s pause |
| `--include-unscored` | off | also run Case × Target pairs with no label (they can only grade as unscored) |
| `--config`, `--dataset`, `--out` | see above | |

A reply that arrives is never retried, even when its effort is invalid. A Trial's outcome is its last Attempt, and every Attempt is kept. Before sending, `run` prints the planned Trial count and the maximum number of requests, then one progress line per finished Trial on stderr, and a summary on stdout.

**Cancelling:** press Ctrl-C (or send SIGTERM) once to stop starting new Trials; Trials already in flight finish, retries included, and the run is saved as `partial`. Press it again to stop at once; the file then has no end record and reads back as partial (interrupted).

**Exit codes:** `0` complete and every Trial answered (an effort may still grade as invalid); `3` complete, but some Trial ended in a provider or transport failure; `130` cancelled (partial); `1` bad config, input or missing key (nothing sent or saved), or the run failed on an internal error such as an evidence write; `2` usage error.

**Evidence:** one append-only JSONL file per run under `$XDG_DATA_HOME/decisionbench/runs` (default `~/.local/share/decisionbench/runs`, or `--out DIR`), private to your user. The first line is the run manifest (settings, Candidates with their price snapshot, Judge Instructions, dataset revision); then one line per finished Trial with all its Attempts; then an end line with the state `complete`, `partial` or `failed`. `show FILE` prints the same summary offline. Grades and costs are recomputed when reading, never stored.

**Cost:** each answered Attempt costs `input_tokens × input rate + output_tokens × output rate + per-request fee`, using the price captured in the run, not the current config. A failed Attempt (error reply or transport failure) is counted as costing nothing, on the assumption that providers do not bill failed requests; the summary says how many were counted this way. A Candidate without a price, or an answer without the needed usage counters, shows as unknown, never as zero. OpenRouter's own `usage.cost` is kept in the raw reply but not used.

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
