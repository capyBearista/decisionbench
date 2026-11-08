# decisionbench

A terminal-first benchmark for **Decisions models**: specialised models that are asked which reasoning effort (`low`, `medium`, `high`, `xhigh` or `max`) a target assistant should use for the next message. It scores **Claude Haiku, Sonnet, Opus and Fable** separately, against human-reviewed acceptable ranges on 66 bundled Cases. It measures effort selection only, not whether a model finishes the task at that effort.

- A Rust engine shared by a headless CLI and a Ratatui terminal UI.
- Providers: OpenRouter and OpenCode Zen. The same model on two routes is two separate Candidates.
- Accuracy (hit, under, over), invalid and unscored counts, reliability, latency, consistency, history and manually priced cost, reported **separately**. There is no composite score.
- Raw predictions and confidence are kept as returned. The current effort is never sent as a hint.
- Local TOML config and versioned JSON/JSONL evidence. No web UI, server, database or Docker.

**Status: version 1.0.0, the first release.** Linux is the only supported platform. The test suite runs offline; see [Known limitations](#known-limitations).

## Install

You need Linux, `git`, and a Rust toolchain that supports edition 2024. The dependencies declare Rust 1.89 as their highest minimum, and `Cargo.toml` sets `rust-version = "1.89"`; only Rust 1.99.1 has been tested.

Install straight from GitHub (needs only `cargo`; it builds on your machine in about a minute and puts `decisionbench` in `~/.cargo/bin`):

```sh
cargo install --git https://github.com/capyBearista/decisionbench --locked                 # latest main
cargo install --git https://github.com/capyBearista/decisionbench --locked --tag v1.0.0    # a specific release
decisionbench --help
```

To update, run the same command again (with `--tag`, name the newer tag). Nothing checks for updates by itself. `main` can be ahead of the latest release, so use `--tag` to stay on releases; releases are listed at https://github.com/capyBearista/decisionbench/releases.

Or build from a clone:

```sh
git clone https://github.com/capyBearista/decisionbench.git
cd decisionbench
cargo build --release          # binary: target/release/decisionbench
cargo install --path .         # or install it into ~/.cargo/bin
```

The examples below use `cargo run --` from a checkout; replace it with `decisionbench` once installed.

## Quick start

```sh
cargo run -- dataset inspect            # bundled dataset
cargo run -- dataset inspect FILE.json  # another dataset file
cargo run -- grade PREDICTIONS.json     # grade against the bundled dataset (or --dataset FILE.json)
cargo run -- run --candidate jev-openrouter --case c002 --target opus --repetitions 1   # smoke test
cargo run -- run --candidate jev-openrouter --candidate jev-zen-free                     # full run
cargo run -- show ~/.local/share/decisionbench/runs/FILE.jsonl      # full report of one run
cargo run -- compare RUN1.jsonl RUN2.jsonl                          # prompts and runs side by side
cargo run -- history                                                # saved runs
cargo run -- tui                                                    # terminal UI
```

A first session:

1. `dataset inspect` shows the 66 Cases, their split and their label counts without a network or a key.
2. Create `~/.config/decisionbench/config.toml` with a Candidate (see below) and set its key (`OPENROUTER_API_KEY`, or `K` in the TUI). Zen free models need no key.
3. Send one request as a smoke test: `run --candidate NAME --case c002 --target opus --repetitions 1`.
4. Run the full set, then `history` and `show` the file it prints.
5. To compare prompts or models, pass `--instructions` more than once or `--candidate` more than once, then `show` the run; `compare` puts several runs side by side.

## Sending a request

A Candidate pairs a Decision Model with a Provider Route. Define Candidates in `$XDG_CONFIG_HOME/decisionbench/config.toml` (default `~/.config/decisionbench/config.toml`, or pass `--config FILE`):

```toml
# prompts_dir = "prompts"          # optional, top level: a folder of Judge Instructions files for the TUI

[[candidate]]
name = "jev-openrouter"            # your label; must be unique
decision_model = "jev-1.13"        # optional: the underlying model
provider = "openrouter"            # "openrouter" or "zen"
model = "typesafe/jev-1.13"        # the model id sent on the wire

[candidate.price]                  # optional; USD, captured with each run
input_per_million = 0.04           # required in the table
output_per_million = 0.0           # optional, default 0

[[candidate]]
name = "jev-zen-free"
decision_model = "jev-1.13"
provider = "zen"
model = "jev-1.13-free"
```

The only top-level key besides `[[candidate]]` is the optional `prompts_dir` (see [Judge Instructions](#judge-instructions)); any other unknown key is rejected. A config with no `[[candidate]]` tables loads as an empty list.

API keys come from the environment variables `OPENROUTER_API_KEY` (required for `openrouter`) and `OPENCODE_API_KEY` (optional for `zen`; free models work without one, and no `Authorization` header is sent without a key). A non-empty variable wins; otherwise the key saved in `credentials.toml`, next to the config file, is used (an empty value counts as unset). Save a key with `K` (Shift-k) on the TUI Candidates screen, or write the file by hand as `openrouter = "..."` and/or `zen = "..."`. The TUI creates the file with mode 0600; a hand-written one is read as is, so make it private yourself. A key is never written to the config or to evidence. An unreadable or invalid `credentials.toml` stops runs even when an environment key is set; the Candidates row then shows `file error`. Requests always go to the provider's own URL and honour the standard `HTTPS_PROXY`/`HTTP_PROXY`/`ALL_PROXY`/`NO_PROXY` environment variables. The tests redirect every request, API key included, to a local stub with `DECISIONBENCH_TEST_BASE_URL`; never set it outside tests.

### Running a benchmark

`run` sends one independent request per Trial (Candidate × Judge Instructions × Case × repetition). The request asks one question per selected Target Model (`effort_haiku`, `effort_sonnet`, `effort_opus`, `effort_fable`) and carries the Case message and its context, if any. It never sends current effort or current model. Because one request covers all selected targets, the answers within a Trial are not independent. Each target's answer is a Prediction, graded against that target's label.

| Flag | Default | Meaning |
|---|---|---|
| `--candidate NAME` | required | Candidate from the config; repeatable |
| `--instructions PATH` | bundled `neutral` | Judge Instructions file; repeatable, with distinct names. Any given path replaces the bundled set |
| `--case ID` | every Case | repeatable |
| `--split train\|holdout` | both | only the Cases in that part of the dataset; combines with `--case` |
| `--target haiku\|sonnet\|opus\|fable` | all four | which questions to ask; repeatable. The instructions file must have a section for each |
| `--repetitions N` | 3 | independent Trials per combination |
| `--concurrency N` | 1 | Trials in flight at once |
| `--timeout SECS` | 60 | per request |
| `--retries N` | 1 | extra Attempts after a transport error or HTTP 408, 429, 500, 502, 503, 504, 524 or 529, after a pause of 1 s that doubles up to 30 s (a `Retry-After` of up to 60 s can lengthen it) |
| `--include-unscored` | off | also run Cases that none of the selected targets labels (they can only grade as unscored) |
| `--config`, `--dataset`, `--out` | see above | |

A reply that arrives in full (2xx) is never retried, even when its effort is invalid; a 2xx whose body is cut off counts as a transport error and is retried. A Trial's outcome is its last Attempt, and every Attempt is kept. Before sending, `run` prints the planned Trial count and the maximum number of requests, then one progress line per finished Trial on stderr, and a summary on stdout.

**Cancelling:** press Ctrl-C (or send SIGTERM or SIGHUP) once to stop starting new Trials; Trials already in flight finish, retries included, and the run is saved as `partial`. If every planned Trial had already started, nothing is dropped and the run still ends `complete`. Press Ctrl-C again to stop at once; the file then has no end record and reads back as partial (interrupted).

**Exit codes:** `0` complete and every Trial answered (an effort may still grade as invalid); `3` complete, but some Trial ended in a provider or transport failure; `130` cancelled (partial); `1` bad config, input or missing key (nothing sent or saved), or the run failed on an internal error such as an evidence write; `2` usage error.

**Evidence:** one append-only JSONL file per run under `$XDG_DATA_HOME/decisionbench/runs` (default `~/.local/share/decisionbench/runs`, or `--out DIR`), private to your user. The first line is the run manifest (settings, Candidates with their price snapshot, Judge Instructions, dataset revision); then one line per finished Trial with all its Attempts; then an end line with the state `complete`, `partial` or `failed`. The format is schema version 4; older files are not readable. Grades and costs are recomputed when reading, never stored.

### Reading results

- `show FILE [--columns auto|prompt|candidate]` prints the full report of one run. After the run header comes a summary: the table described under `compare`, for this run alone, with one column per prompt (or per candidate, see `--columns` there). The rest is accuracy (hit, under, over, invalid and unscored counts per Candidate × Judge Instructions × target, with the denominator), cross-model metrics, consistency, coverage and validity, reliability, latency (median and p95), cost and warnings. A ratio with nothing to divide by prints as `n/a`, not 0%.
- `compare FILE… [--columns auto|prompt|candidate]` (two or more) first lists everything that makes the runs not directly comparable: dataset, grading revision, schema, targets, provider route, case selection, repetitions, unscored inclusion, candidates, or a run that is not complete. Timeout, concurrency and retries are listed apart as `settings differ`. Different prompts are not a difference on the prompt axis, where they are the columns; likewise candidates present in only some runs are not a difference when candidates are the columns (see `--columns` below). It then prints one table with a row per candidate × target and a column per prompt in each run; the first column is the baseline. A column is named after its prompt, with the first 12 characters of its hash added when two prompts share a name but not content, and ` in <run id>` when the same prompt appears in two runs. Each cell after the first shows its hit count and `+g -l`: the trials (case × repetition) that the baseline missed and this prompt hit, and the reverse. The change is marked `~` (not significant) unless an exact two-sided sign test on the cases that changed gives p < 0.05: each case counts once, up if its repetitions gained more trials than they lost and down if fewer, and cases that stayed the same are ignored. The 5% line is a convention, and with many cells some will look significant by chance. A `pooled` row adds up the rows present in every column (so all columns cover the same candidates and targets) over valid scored predictions, counting a case once per candidate (once per prompt with `--columns candidate`) for the sign test however many targets it covers; it is not a score, and it is left out when no row is present in every column. `--columns candidate` swaps the axes: a column per candidate in each run (named after the candidate, plus ` in <run id>` when two columns share a name; the first is the baseline), and a row per prompt × target (the prompt name, plus its 12-character hash when two prompts share a name but not content). Each delta then compares a candidate with the baseline candidate on the same prompt and target, paired the same way. `--columns prompt` keeps prompts as columns. The default, `auto`, uses prompts when the runs between them hold more than one distinct prompt, otherwise candidates when they hold more than one candidate, otherwise prompts. The table says which it used on its `columns:` line.
- `score FILE [--dataset PATH] [--allow-holdout]` prints JSON for prompt tuning, per Candidate × prompt and target and pooled: hit, under, over, invalid, failed, coverage, hit rate, **floor** (the hit rate of the best single fixed effort on the same rows) and **lift** (hit rate minus floor). Invalid answers count as misses; Trials with no valid reply are left out and lower coverage. Every Case has a `split` (`train` or `holdout`); `score` refuses a run that includes holdout Cases unless `--allow-holdout` is given, a different dataset than the run used, and prompts that quote a Case message ([ADR-0007](docs/adr/0007-split-and-tuning-objective.md)).
- `history [DIR]` lists saved runs by creation time (default: the evidence folder above) with state, recorded and planned Trials, Candidates and dataset hash. Unreadable files are listed as such.

The two cross-model metrics sit next to the per-target numbers and never replace them. **Joint hit** is the share of Trials, among those with at least two labelled targets that all gave a valid effort, where every labelled target hit. **Gap agreement** is the share of eligible target pairs that keep the expected order: when two labels do not overlap, the target with the higher range must get a strictly higher effort.

### Terminal UI

`decisionbench tui` (also takes `--config FILE` and `--out DIR`) opens the same engine and reports in a terminal. It has five screens: Candidates, Dataset, Run setup, Run and Results/History. Switch screens with `Tab` or `1`-`5`, move with the arrow keys or `h`/`j`/`k`/`l`, select with `Enter`, quit with `q`. `Esc` cancels a form, a key prompt or a text entry, and goes back from an opened run in Results; it is not a back key on every screen. On the Candidates screen, `K` asks for the API key of the highlighted Candidate's provider and saves it at once to `credentials.toml`; each row shows `key: env`, `key: saved`, `key: missing` (OpenRouter), `key: none (free ok)` (Zen without a key) or `file error`. On the Run setup screen, prompts are a list: the bundled `neutral` is chosen by default, `Space` toggles a prompt, `a` adds a prompt file by path (checked on entry, relative to the folder you started the TUI from) and `d` removes an added one, so one run can use several prompts. The `prompt folder` row sets `prompts_dir` in the config file; its `*.md` files are listed too, and `r` rescans the folder. A dim `plan:` line above Start shows candidates × prompts × cases × repetitions = trials, and the most requests that could take. On the Run screen, press `c` to cancel (stop starting Trials) and `c` again to stop waiting for Trials in flight; `Enter` opens the finished run in Results. On the Dataset screen, `Space` marks or unmarks the highlighted Case (`[x]`) and `x` clears all marks; Run setup then shows `cases: all eligible` or `cases: N selected (...)`, the same selection as `--case`. The Dataset screen draws each labelled target's acceptable range as a bar across the five effort levels. Results opens a run on a summary table, and `[` and `]` switch between it and the full report, which is the text `show` prints (`compare` for marked runs). `c` swaps the columns between prompts and candidates in both panels (the panel title says which); a newly opened report starts on the automatic choice. The summary also opens with the targets pooled: one row per candidate (or prompt) with `all` in the target column, adding up only the targets every column has, so it hides differences between targets. `t` spreads it into one row per target and back; the full report is not affected. Mark runs with `Space` and press `m` or `Enter` to compare; marks show as `[1]`, `[2]`… in the order you set them, and the first is the baseline. If the evidence file cannot be written during a Run, the Run screen says so and the unsaved Trials are printed to stderr as JSON lines when you quit.

**Cost:** each answered Attempt costs `input_tokens × input rate + output_tokens × output rate`, using the price captured in the run, not the current config. A failed Attempt (error reply or transport failure before a success status) is counted as costing nothing, on the assumption that providers do not bill failed requests; the summary says how many were counted this way. A Candidate without a price, or an answer without the needed usage counters, shows as unknown, never as zero. OpenRouter's own `usage.cost` is kept in the raw reply but not used.

**Evidence files contain the full prompt text, the Case message and context, and the raw provider reply.** Review them before sharing.

### Judge Instructions

The task text, per-model instructions and per-effort descriptions sent with every request live in a Markdown file. Two sets ship with the repository; only `neutral` is built into the binary:

- [`prompts/neutral.md`](prompts/neutral.md), the default, uses the same neutral criteria for every model and has no effortless policy rules.
- [`prompts/effortless.md`](prompts/effortless.md), selected with `--instructions prompts/effortless.md`, mirrors effortless's text for Haiku, Sonnet and Opus, plus a **drafted Fable section** that has not been measured. Its task text quotes examples that match Cases in the dataset (for example c021 "review the entire repo" and c026 "migrate everything"), so its scores are in-sample: a gap between `effortless` and `neutral` is not a clean measure of prompt quality.

Pass another file with `--instructions PATH`; the file name without `.md` is the set's name. Repeat the flag to put several sets in one run (names must be distinct); `compare` then lines the sets up side by side, and also across runs. With no flag the run uses `neutral`; with any flag it uses only the files given, so add `prompts/neutral.md` to include the default. In the TUI, the config key `prompts_dir = "<folder>"` names a folder whose `*.md` files appear in the Run setup list (not subfolders; a relative path is relative to the config file's folder, and `~` is not expanded; the TUI writes the key when you set the folder). The command line does not read it. A file that does not parse, or whose name repeats a listed prompt, is shown but cannot be chosen. The format is an intro (sent as `state.task`), then one `## <target>` section per model in the order `haiku`, `sonnet`, `opus`, `fable`. Each section is an instructions paragraph followed by exactly five lines in the order `low`, `medium`, `high`, `xhigh`, `max`, none empty:

```markdown
Choose the reasoning effort ... for each model asked about.

## haiku
Which effort should Claude Haiku 5.5 use for the next message?

- low: quick answer or trivial action
- medium: ...
- high: ...
- xhigh: ...
- max: ...

## sonnet
...
```

A file may leave a model out, but asking about it with `--target` is then an input error (exit 1) before anything is sent. Evidence records the set's name, SHA-256 and full text.

## Offline grading

A predictions file is a JSON array such as `[{"case":"c001","target":"sonnet","effort":"high"}]`; `target` is `haiku`, `sonnet`, `opus` or `fable`. A Case with no label for that target grades as unscored, whatever the effort. Otherwise only an exact effort name (`low`, `medium`, `high`, `xhigh`, `max`) is valid; anything else, or a missing `effort`, grades as invalid. The raw effort is printed as re-serialized JSON. Accuracy counts hits among valid, scored predictions.

## Evidence format

One run is one append-only JSONL file (schema version 4) in the evidence folder, written with mode 0600:

| Line | Holds |
|---|---|
| first: `run` (the manifest) | schema version, run id (`{unix_ms}-{pid}`, also the file name), dataset name and SHA-256, the full text and hash of each Judge Instructions set, grading revision, each Candidate (name, provider, model, price snapshot, the URL actually used), the Case ids and targets, settings, planned Trial count |
| one per finished Trial | Candidate, instructions name and hash, Case id, repetition, targets asked, the Case's labels at request time, the request body, every Attempt, start time, elapsed time |
| last: `end` | the run state (`complete`, `partial` or `failed`) and the finish time |

An Attempt records its start time, latency, disposition (`answered`, `provider_error` or `transport_error`), HTTP status, the raw reply body and any error text. Trial lines are written as Trials finish, so file order is not plan order (`seq` gives the plan position). A file with no end line reads back as partial (interrupted); a truncated last line is skipped with a warning.

Grades and costs are **derived when a file is read** and never stored, so a later grading revision can regrade old evidence without changing it. Only schema 4 is read: any other `schema_version` is rejected with a message naming it, and there is no migration. The key is never written to evidence; if a provider echoes it in a reply, the copy in the saved reply is replaced with `[redacted]` (exact text only, see [Known limitations](#known-limitations)).

## Troubleshooting

| Symptom | Cause and fix |
|---|---|
| `cannot read …/config.toml: No such file or directory` | `run` needs a config with at least one `[[candidate]]`. Create `~/.config/decisionbench/config.toml` (or pass `--config FILE`) as shown above. `dataset`, `grade`, `show`, `compare` and `history` need none. |
| `run` exits 1 before sending anything | Bad config or input, or a missing key. The message names the file or variable. Set `OPENROUTER_API_KEY`, or press `K` on the TUI Candidates screen. |
| The Candidates row says `key: missing` or `file error` | No key for that provider, or `credentials.toml` does not parse. A broken credentials file stops runs even when an environment key is set. Fix or delete the file. |
| Exit code 3 | The run finished but some Trial ended in a provider or transport failure. `show FILE` has the Reliability section; the raw body is kept in the evidence. |
| Exit code 130, or `partial` in `history` | The run was cancelled or interrupted. The evidence holds the Trials that finished; compare treats partial runs as not comparable. |
| HTTP 429 or a free-quota error from Zen | The free quota is used up. Retries pause 1 s, doubling to 30 s, and honour a `Retry-After` in seconds up to 60 s; if they still fail, wait, lower `--concurrency`, or use a paid model. |
| Timeouts | Raise `--timeout` (default 60 s). Requests honour `HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY` and `NO_PROXY`. |
| `unsupported schema_version` | The file was written by another version of the tool. Only schema 4 is read. |
| `terminal too small` | The TUI needs at least 60x16. |
| `tui` says it cannot open the terminal | It needs an interactive terminal; use the CLI in scripts. |
| A candidate model answers 400 | Some models restrict the `state` shape or the question types they accept (for example, some `:free` routes). The 400 is saved as a provider error. |

Never set `DECISIONBENCH_TEST_BASE_URL` outside tests: it sends every request, key included, to the address you give.

## Known limitations

- **Linux only.** No Windows or macOS builds, CI or support.
- **Prices are manual.** There is no price fetching, and cost is an estimate from the usage counters in the reply. Unknown usage or price shows as unknown, never zero.
- **Dataset.** The 66 Cases were converted once from effortless, but effortless labelled only 75 of its 142 Case–Target slots (67 Sonnet, 8 Opus), so most of the 264 labels now in the file, including every Haiku and Fable label, were drafted case by case and approved by the maintainer ([ADR-0003](docs/adr/0003-owned-per-target-effort-ranges.md), [ADR-0005](docs/adr/0005-one-request-per-case-four-targets.md)). They are reviewed judgements, not measurements and not independently adjudicated. Ranges are acceptable levels, not measured task success; a target left without a label stays unscored. Any change to the file is a new dataset revision.
- **Fable instructions are drafted.** `prompts/effortless.md` has a Fable section that has not been measured against effortless.
- **`effortless` is in-sample.** Its task text quotes examples that match Cases in the dataset (see Judge Instructions), so compare it with `neutral` knowing that.
- **Key redaction is exact-text.** A provider reply that echoes the key in another form (encoded, split) is not caught. Evidence also holds the full prompt, the Case text and the raw reply, so review it before sharing.
- **Retries are simple.** Exponential backoff with a `Retry-After` in seconds (not an HTTP-date); concurrency is global, not per provider; a complete 2xx reply is never retried (a 2xx cut off mid-body is).
- **One request covers all targets**, so the answers within a Trial are not independent; the sign test accounts for this by counting each Case once. The 5% line is a convention, and a table with many cells will show some significant deltas by chance.
- **TUI.** Terminals narrower than 60 columns or shorter than 16 rows show a size message. The Candidates form hint is not wrapped.
- **Deferred.** An export contract for other tools, HTTP API, Docker packaging, a label-editing UI, other providers (SubQ, direct TypeSafe), and price scraping. See [issue #1](https://github.com/capyBearista/decisionbench/issues/1).
- **Live checks.** Both routes have answered four-target requests during development (OpenRouter with `baseten/joey-1` and `perplexity/pplx-decider-flash-v1.2`, Zen with `jev-1.13-free`). The automated tests never call a provider, and other models on these routes may behave differently.

## Documentation

- [Product and engineering specification](docs/SPEC.md): behaviour, evidence, scoring, scope and testing decisions.
- [Domain glossary](GLOSSARY.md): the terms used throughout (Candidate, Trial, Attempt, Target Model, Judge Instructions, Prediction).
- [Architecture decisions](docs/adr/): [0001](docs/adr/0001-standalone-engine.md) standalone engine, [0002](docs/adr/0002-raw-model-relative-evaluation.md) raw model-relative grading, [0003](docs/adr/0003-owned-per-target-effort-ranges.md) per-target ranges, [0004](docs/adr/0004-judge-instructions-axis.md) Judge Instructions, [0005](docs/adr/0005-one-request-per-case-four-targets.md) one request per Case for four targets, [0006](docs/adr/0006-prompts-as-a-comparison-axis.md) prompts as a comparison axis, [0007](docs/adr/0007-split-and-tuning-objective.md) train/holdout split, tuning score and retry backoff.
- [Issue tracker convention](docs/agents/issue-tracker.md) and [domain-documentation map](docs/agents/domain.md).

## Source attribution

The dataset is derived from the MIT-licensed [effortless project](https://github.com/capyBearista/effortless) by HeyCubit. Its license is in [datasets/LICENSE-effortless](datasets/LICENSE-effortless).

## Project license

[MIT](LICENSE).
