# decisionbench

**Status: early implementation.** The Rust CLI can validate and summarize the bundled dataset, grade local effort predictions offline, and run batches of Decisions requests (Candidate × Judge Instructions × Case × repetition, one question per Target Model) through OpenRouter or OpenCode Zen with retries, a concurrency limit and safe cancellation, saving every Attempt as evidence with a manually priced cost estimate. Saved Runs can be reported, compared and listed offline. A terminal UI (`decisionbench tui`) is under development.

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
| `--target haiku\|sonnet\|opus\|fable` | all four | which questions to ask; repeatable. The instructions file must have a section for each |
| `--repetitions N` | 3 | independent Trials per combination |
| `--concurrency N` | 1 | Trials in flight at once |
| `--timeout SECS` | 60 | per request |
| `--retries N` | 1 | extra Attempts after a transport error or HTTP 408, 429, 500, 502, 503, 504, 524 or 529, after a 1 s pause |
| `--include-unscored` | off | also run Cases that none of the selected targets labels (they can only grade as unscored) |
| `--config`, `--dataset`, `--out` | see above | |

A reply that arrives is never retried, even when its effort is invalid. A Trial's outcome is its last Attempt, and every Attempt is kept. Before sending, `run` prints the planned Trial count and the maximum number of requests, then one progress line per finished Trial on stderr, and a summary on stdout.

**Cancelling:** press Ctrl-C (or send SIGTERM or SIGHUP) once to stop starting new Trials; Trials already in flight finish, retries included, and the run is saved as `partial`. If every planned Trial had already started, nothing is dropped and the run still ends `complete`. Press Ctrl-C again to stop at once; the file then has no end record and reads back as partial (interrupted).

**Exit codes:** `0` complete and every Trial answered (an effort may still grade as invalid); `3` complete, but some Trial ended in a provider or transport failure; `130` cancelled (partial); `1` bad config, input or missing key (nothing sent or saved), or the run failed on an internal error such as an evidence write; `2` usage error.

**Evidence:** one append-only JSONL file per run under `$XDG_DATA_HOME/decisionbench/runs` (default `~/.local/share/decisionbench/runs`, or `--out DIR`), private to your user. The first line is the run manifest (settings, Candidates with their price snapshot, Judge Instructions, dataset revision); then one line per finished Trial with all its Attempts; then an end line with the state `complete`, `partial` or `failed`. The format is schema version 4; older files are not readable. Grades and costs are recomputed when reading, never stored.

### Reading results

- `show FILE [--columns auto|prompt|candidate]` prints the full report of one run. After the run header comes a summary: the table described under `compare`, for this run alone, with one column per prompt (or per candidate, see `--columns` there). The rest is accuracy (hit, under, over, invalid and unscored counts per Candidate × Judge Instructions × target, with the denominator), cross-model metrics, consistency, coverage and validity, reliability, latency (median and p95), cost and warnings. A ratio with nothing to divide by prints as `n/a`, not 0%.
- `compare FILE… [--columns auto|prompt|candidate]` (two or more) first lists everything that makes the runs not directly comparable: dataset, grading revision, schema, targets, provider route, case selection, repetitions, unscored inclusion, candidates, or a run that is not complete. Timeout, concurrency and retries are listed apart as `settings differ`. Different prompts are not a difference on the prompt axis, where they are the columns; likewise candidates present in only some runs are not a difference when candidates are the columns (see `--columns` below). It then prints one table with a row per candidate × target and a column per prompt in each run; the first column is the baseline. A column is named after its prompt, with the first 12 characters of its hash added when two prompts share a name but not content, and ` in <run id>` when the same prompt appears in two runs. Each cell after the first shows its hit count and `+g -l`: the trials (case × repetition) that the baseline missed and this prompt hit, and the reverse. The change is marked `~` (not significant) unless an exact two-sided sign test on the cases that changed gives p < 0.05: each case counts once, up if its repetitions gained more trials than they lost and down if fewer, and cases that stayed the same are ignored. The 5% line is a convention, and with many cells some will look significant by chance. A `pooled` row adds up the rows present in every column (so all columns cover the same candidates and targets) over valid scored predictions; it is not a score, and it is left out when no row is present in every column. `--columns candidate` swaps the axes: a column per candidate in each run (named after the candidate, plus ` in <run id>` when two columns share a name; the first is the baseline), and a row per prompt × target (the prompt name, plus its 12-character hash when two prompts share a name but not content). Each delta then compares a candidate with the baseline candidate on the same prompt and target, paired the same way. `--columns prompt` keeps prompts as columns. The default, `auto`, uses prompts when the runs between them hold more than one distinct prompt, otherwise candidates when they hold more than one candidate, otherwise prompts. The table says which it used on its `columns:` line.
- `history [DIR]` lists saved runs by creation time (default: the evidence folder above) with state, recorded and planned Trials, Candidates and dataset hash. Unreadable files are listed as such.

The two cross-model metrics sit next to the per-target numbers and never replace them. **Joint hit** is the share of Trials, among those with at least two labelled targets that all gave a valid effort, where every labelled target hit. **Gap agreement** is the share of eligible target pairs that keep the expected order: when two labels do not overlap, the target with the higher range must get a strictly higher effort.

### Terminal UI

`decisionbench tui` (also takes `--config FILE` and `--out DIR`) opens the same engine and reports in a terminal. It has five screens: Candidates, Dataset, Run setup, Run and Results/History. Switch screens with `Tab` or `1`-`5`, move with the arrow keys or `h`/`j`/`k`/`l`, select with `Enter`, go back with `Esc` and quit with `q`. On the Candidates screen, `K` asks for the API key of the highlighted Candidate's provider and saves it at once to `credentials.toml`; each row shows `key: env`, `key: saved` or `key: missing`. On the Run setup screen, prompts are a list: the bundled `neutral` is chosen by default, `Space` toggles a prompt, `a` adds a prompt file by path (checked on entry, relative to the folder you started the TUI from) and `d` removes an added one, so one run can use several prompts. The `prompt folder` row sets `prompts_dir` in the config file; its `*.md` files are listed too, and `r` rescans the folder. A dim `plan:` line above Start shows candidates × prompts × cases × repetitions = trials, and the most requests that could take. On the Run screen, press `c` to cancel (stop starting Trials) and `c` again to stop waiting for Trials in flight; `Enter` opens the finished run in Results. On the Dataset screen, `Space` marks or unmarks the highlighted Case (`[x]`) and `x` clears all marks; Run setup then shows `cases: all eligible` or `cases: N selected (...)`, the same selection as `--case`. The Dataset screen draws each labelled target's acceptable range as a bar across the five effort levels. Results opens a run on a summary table, and `[` and `]` switch between it and the full report, which is the text `show` prints (`compare` for marked runs). `c` swaps the columns between prompts and candidates in both panels (the panel title says which); a newly opened report starts on the automatic choice. Mark runs with `Space` and press `m` or `Enter` to compare; marks show as `[1]`, `[2]`… in the order you set them, and the first is the baseline. If the evidence file cannot be written during a Run, the Run screen says so and the unsaved Trials are printed to stderr as JSON lines when you quit. Details may change while the TUI is being built.

**Cost:** each answered Attempt costs `input_tokens × input rate + output_tokens × output rate`, using the price captured in the run, not the current config. A failed Attempt (error reply or transport failure before a success status) is counted as costing nothing, on the assumption that providers do not bill failed requests; the summary says how many were counted this way. A Candidate without a price, or an answer without the needed usage counters, shows as unknown, never as zero. OpenRouter's own `usage.cost` is kept in the raw reply but not used.

**Evidence files contain the full prompt text, the Case message and context, and the raw provider reply.** Review them before sharing.

### Judge Instructions

The task text, per-model instructions and per-effort descriptions sent with every request live in a Markdown file. Two sets ship with the repository; only `neutral` is built into the binary:

- [`prompts/neutral.md`](prompts/neutral.md), the default, uses the same neutral criteria for every model and has no effortless policy rules.
- [`prompts/effortless.md`](prompts/effortless.md), selected with `--instructions prompts/effortless.md`, mirrors effortless's text for Haiku, Sonnet and Opus, plus a **drafted Fable section** that has not been measured.

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

A file may leave a model out, but asking about it with `--target` is then a usage error before anything is sent. Evidence records the set's name, SHA-256 and full text.

## Offline grading

A predictions file is a JSON array such as `[{"case":"c001","target":"sonnet","effort":"high"}]`; `target` is `haiku`, `sonnet`, `opus` or `fable`. A Case with no label for that target grades as unscored, whatever the effort. Otherwise only an exact effort name (`low`, `medium`, `high`, `xhigh`, `max`) is valid; anything else, or a missing `effort`, grades as invalid. The raw effort is printed as re-serialized JSON. Accuracy counts hits among valid, scored predictions.

**decisionbench** is a planned standalone, terminal-first benchmark for **Decisions models**: specialized models asked to choose an appropriate reasoning-effort level for a target assistant. Its first benchmark evaluates effort-selection quality for **Claude Haiku, Sonnet, Opus and Fable** separately, not task-execution success.

## Initial product

- **Rust** benchmark engine shared by a **headless CLI** and **Ratatui TUI**.
- Initial provider routes: **OpenCode Zen** and **OpenRouter Decisions API**; the same underlying model exposed through different routes is compared as a distinct candidate.
- One versioned dataset (`datasets/cases.json`, 66 Cases) converted from the effortless benchmark cases.
- Independent, human-reviewed acceptable effort ranges per Target Model, such as `"low-high"`. Every Case has a label for all four targets today; a missing label would grade as **unscored**. Each Trial is one API request that asks about every selected model, with three logical trials per combination by default.
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
