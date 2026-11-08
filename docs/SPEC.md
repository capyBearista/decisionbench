# decisionbench: initial product and engineering specification

**Status:** Approved (2026-11-03)  
**Scope:** Initial standalone release  
**Sources:** Agreed design interview; [effortless source benchmark](https://github.com/capyBearista/effortless/blob/99c03f222d9a71dd1e0d2fa5824eb1cb59bbb20c/bench/judge-cases.json); [effortless judge implementation](https://github.com/capyBearista/effortless/blob/main/hooks/register.tsx); [deferred evidence export](https://github.com/capyBearista/decisionbench/issues/1)  
**Domain terminology:** [GLOSSARY.md](../GLOSSARY.md)  
**Architecture decisions:** [ADR-0001](adr/0001-standalone-engine.md), [ADR-0002](adr/0002-raw-model-relative-evaluation.md), [ADR-0003](adr/0003-owned-per-target-effort-ranges.md), [ADR-0004](adr/0004-judge-instructions-axis.md), [ADR-0005](adr/0005-one-request-per-case-four-targets.md), [ADR-0006](adr/0006-prompts-as-a-comparison-axis.md)

## Problem statement

Decision models can recommend the reasoning effort for an agentic assistant, but existing embedded benchmarking in effortless is limited to fixed judges, mixes model predictions with production fallbacks/shortcuts, and does not provide a general-purpose, reproducible way to compare model/provider routes.

The initial question is: **given the same task and relevant conversation context, which effort level should an independent Decisions model recommend for each of Claude Haiku, Sonnet, Opus and Fable, and how accurately, consistently, reliably, and quickly does it make that recommendation?**

This is **not** a test of whether Sonnet or Opus successfully completes the underlying task. Ground truth is a human-reviewed range of acceptable effort levels per target model, not measured task completion.

## Solution

Create an open-source, standalone Rust application named **decisionbench**, with a reusable evaluation engine and two interfaces: an interactive Ratatui terminal UI and a noninteractive CLI. It makes direct requests to specialized Decisions APIs, grades raw predictions against independently reviewed labels, preserves versioned execution evidence, and presents separate metrics for quality, reliability, latency, and manually configured cost.

Start with **OpenCode Zen** and **OpenRouter Decisions API**. Support multiple models and distinct provider routes for the same underlying model. The CLI and TUI use the same engine and data format. No agent harness runtime, HTTP service, Docker container, database server, or web UI is required.

## User stories

1. As a user, I can register a Decisions-model candidate with its exact provider route so similarly named models from different providers remain separately comparable.
2. As a user, I can select a subset of candidates to benchmark without editing source code.
3. As a user, I can configure provider credentials outside result files and receive actionable errors for missing credentials.
4. As a user, I can load, inspect, and select a versioned benchmark dataset.
5. As a user, I can maintain one dataset whose Cases each carry a separate acceptable-effort range per target model.
6. As a user, I can evaluate the same prompt against Haiku, Sonnet, Opus and Fable in one request that asks one question per model.
7. As a user, I can supply independently reviewed acceptable effort ranges for each target model, not a derived one-step equivalence.
8. As a user, I can see that an unlabelled target/prompt is **unscored**, not silently graded right or wrong.
9. As a user, I can run one model against one case before committing to a larger batch.
10. As a user, I can choose repetitions per case/target/candidate, with three independent trials as the default.
11. As a user, I can set concurrency, timeout, and bounded retry policy and understand the resulting request volume.
12. As a user, I can cancel a run and retain all completed and attempted evidence with an explicit incomplete status.
13. As a user, I can inspect the raw provider answer, normalized effort prediction, confidence (when provided), provider errors, and parse failures separately.
14. As a user, I can grade a raw effort prediction without having previous effort or production policy alter it.
15. As a user, I can compare accuracy, under-selection, over-selection, reliability, and latency separately, including by target model.
16. As a user, I can manually enter per-route input/output token rates without live price fetching.
17. As a user, I can see an observed-cost estimate only where sufficient usage and manual prices are present, with missing cost shown as unknown rather than zero.
18. As a user, I can review earlier runs and compare them with prompts side by side and with any dataset, model-route, or evaluation difference stated.
19. As a user, I can use the TUI to manage candidates, browse data, run benchmarks, review results, configure rates, and compare history.
20. As an automation caller, I can perform equivalent benchmark execution and read structured results via the headless CLI and versioned local files.
21. As a future integrator of another tool, I can consume an export with verifiable provenance without importing Rust internals.

## Implementation decisions

### 1. Technology and interfaces

- Use the **latest stable Rust compiler** supporting the **2024 language edition**; the compiler release and language edition are different version concepts.
- **Linux is the only supported platform.** No Windows or macOS builds, CI or support.
- Begin with **one Cargo package** containing a binary and a reusable library. Do not make a multi-crate workspace without a demonstrated requirement.
- Use **Ratatui** for the TUI, with an appropriate terminal backend (Crossterm is the initial candidate); use a conventional CLI argument parser.
- Both interfaces invoke the same evaluation engine. The TUI is not the source of truth for evaluation rules.
- Provide a typed in-process/library interface and a machine-readable CLI/evidence contract first. **No HTTP API** in this release.
- Store user configuration as local **TOML** and results as versioned **JSON/JSONL** files. No required SQLite, remote database, daemon, or cloud synchronization.
- The TUI adapts the neutral/charcoal, restrained orange, semantic status, legible hierarchy, and meaningful-interaction principles of [homelab-design](https://github.com/capyBearista/homelab-design), rather than its SvelteKit/browser implementation.
- Introduce libraries and modules as features require them; do not scaffold a generic plugin framework preemptively.

### 2. Benchmark scope and provider identity

- Initially score only **effort selection** for four target models: **Claude Haiku 5.5**, **Sonnet 5.5**, **Opus 5.5** and **Fable 5.1** (`haiku`, `sonnet`, `opus`, `fable`; [ADR-0005](adr/0005-one-request-per-case-four-targets.md)). Valid effort names are the explicit ordered set supported by the benchmark revision (initially `low` < `medium` < `high` < `xhigh` < `max`; every target model accepts every level); verify route-specific supported options during protocol research.
- A **Decision Model** selects effort; a **Target Model** is Haiku, Sonnet, Opus or Fable. These are different roles.
- A **Candidate** includes the underlying Decision Model identity when known and its **Provider Route**. Jev via Zen, Jev via OpenRouter, and Jev via TypeSafe direct are different candidates, even if the model name matches.
- Initial provider transports: OpenCode Zen Decisions endpoint and OpenRouter Decisions API endpoint. Future transports such as SubQ and TypeSafe direct must be possible without changing the domain-level evaluation data; neither is implemented now.
- Preserve any model recommendation returned by a provider as **unscored evidence**; the initial grade covers effort only.
- Verify actual provider schemas, authentication, output shapes, usage availability, and null/omitted field behavior against primary documentation and representative responses. Do not infer compatibility from similarly named APIs.

### 3. Dataset and ground truth

- The benchmark owns **one dataset file**, `datasets/cases.json` (see [ADR-0003](adr/0003-owned-per-target-effort-ranges.md)). Its first 66 Cases were converted once from the MIT-licensed effortless benchmark cases, and the effortless copyright notice and license accompany it; the other 131 (80 adapted from real maintainer messages, 51 synthetic) were added later. effortless labelled only 75 of its 142 Case–Target slots (67 Sonnet, 8 Opus); the other labels, including every Haiku and Fable label, were drafted case by case and approved by the maintainer ([ADR-0005](adr/0005-one-request-per-case-four-targets.md)), so the ranges are reviewed judgements, not measurements. The upstream file is not kept or tracked, and Cases carry no source identifiers.
- Each Case has a plain, stable **ID** (`c001`…`c197`, contiguous) that does not change once the dataset is released, a task **message**, optional conversation **context**, and **labels** keyed by Target Model (`haiku`, `sonnet`, `opus`, `fable`); every Case has at least one label.
- A label is a **contiguous range of acceptable efforts**, written as one level (`"max"`) or as `"from-to"` (`"low-high"` means `low`, `medium` and `high`). `from` may not be above `to`. Every effort in the range is equally acceptable.
- A Target Model may be absent from a Case; that combination is **unscored**. Omitting the target's key is the only way to leave it unscored; a `null` label is rejected. Do not synthesize missing labels.
- Cases carry **no session state** (no current model, no current effort) and no category markers. Each Case has a required **split**, `train` or `holdout` ([ADR-0007](adr/0007-split-and-tuning-objective.md)); `run --split` runs one part, and `score` refuses holdout results unless asked.
- The dataset has **197 Cases** (138 train and 59 holdout; the first 66 are the original set, in which four identical message/context pairs were merged; five Swedish Cases were removed and their IDs refilled by moving the last five Cases into the gaps, so the IDs have no gaps), each with a label for all four targets: **197 Haiku**, **197 Sonnet**, **197 Opus** and **197 Fable**.
- Any change to Cases or labels produces a new **Dataset Revision**, identified by the file's content hash.

### 4. Evaluation requests and independence

- Each **Trial** is one **Candidate × Judge Instructions × Case × repetition**; send an **independent Decisions API request** for each trial, never the full dataset as one request. The request asks one question per selected Target Model, so one Trial yields one **Prediction** per target (Trial × Target Model), and the answers within a Trial are not independent of each other ([ADR-0005](adr/0005-one-request-per-case-four-targets.md)).
- Every target is asked with the same task, conversation context and evaluation settings; only the per-target question text differs, and it comes from the Judge Instructions. Preserve any known provider-envelope differences as evidence.
- `--target` selects which questions are asked (default: all four). A Case is planned when at least one selected target has a label, unless unscored Cases are requested.
- **Do not send the previous/current reasoning effort as a judging input**. Before implementing, establish whether each provider accepts omission or requires an explicit null; never substitute a meaningful effort that can anchor the judge.
- Relevant recent conversation context **does** matter, especially for follow-up/answer cases. Preserve and send it as defined by the dataset revision.
- No effortless-style short-follow-up shortcut, confidence fallback, bias, floor/ceiling, save mode, or automatic keep-current rule is applied in the **raw model-quality** benchmark.
- Parse and preserve the **raw effort prediction** separately from the normalized effort and any future **effective decision** produced by a selected policy. The initial accuracy score uses the raw normalized model prediction, not a post-policy decision.
- Mark an unsupported choice or missing effort as **invalid/unanswered**, not as correct or as a fallback effort.

### 4a. Provider routes and Judge Instructions (checked 2026-11-03)

- Both routes document the same body: `{"model", "state", "questions"}`. decisionbench sends `state` as `{"task", "recent_conversation" (only when the Case has context), "next_message"}` and one `choice` question per selected target, named `effort_<target>` (`effort_haiku`, `effort_sonnet`, `effort_opus`, `effort_fable`). Each question is `{"type": "choice", "instructions", "criteria"}`, where `criteria` maps each effort name to its description; `task`, the instructions and the criteria come from the selected Judge Instructions. `state` is free-form, so `target_model`, `current_effort` and `current_model` are **omitted**, not nulled. OpenRouter accepted a single-question request of this kind live, and both routes have since answered the four-question body for the models listed in the README; other models may restrict it.
- **OpenRouter** (`POST https://openrouter.ai/api/alpha/decisions`, `Authorization: Bearer` with `OPENROUTER_API_KEY` or the saved `openrouter` key, required). A 200 reply is `{"id", "model", "provider", "answers": {"effort_<target>": {"choice", "confidence", "probabilities"}, …}, "usage": {"input_tokens", "output_tokens", "cost"}}`. The reply's `model` can be a dated revision of the requested one (`typesafe/jev-1.13` answered as `typesafe/jev-1.13-20260917`) and `provider` names the upstream host; both are kept as evidence. Errors are `{"error": {"code", "message"}}`.
- **OpenCode Zen** (`POST https://opencode.ai/inference/systemone/v1/systemone`; the older `/zen/v1/systemone` shares its quota). Free models (`jev-1.13-free`) accept requests without a key; paid models answer `401 {"type":"error","error":{"type":"AuthError","message":"Missing API key."}}` without `OPENCODE_API_KEY` or a saved `zen` key. Errors are `{"type": "error", "error": {"type", "message"}}`; a used-up free quota is `429 FreeUsageLimitError` with `Retry-After`. **Observed (2026-11-04 to 2026-11-05):** `jev-1.13-free` answered four-question requests without `current_effort`, with the OpenRouter `answers` shape (`choice` per `effort_<target>`) and `usage` carrying `input_tokens`, `output_tokens`, `cost` and an `opencode` object. Any other shape grades as invalid for a labelled target, and its raw reply is kept.
- Individual models on a route can restrict the `state` shape and the question types they accept. `respan/span-01-lite:free` on OpenRouter (checked 2026-11-04) accepts `state` only as a string or an `input`/`output` message object and answers only yes/no questions, so it cannot run this benchmark; its 400 replies are recorded as provider errors like any other.
- **Judge Instructions** are Markdown files (`prompts/<name>.md`): an intro (sent as `state.task`), then one `## <target>` section per Target Model in the order `haiku`, `sonnet`, `opus`, `fable`. Each section is an instructions paragraph followed by exactly five lines, `- low: …`, `- medium: …`, `- high: …`, `- xhigh: …`, `- max: …`, in that order. The parser is strict: a missing, extra, duplicated, misordered or empty part is rejected. A file may leave a target out, but asking about a target without a section is an input error (exit 1) before anything is sent. Sets are selected per evaluation, recorded by name, SHA-256 and full text, and never edited through the CLI or TUI ([ADR-0004](adr/0004-judge-instructions-axis.md), format amended by [ADR-0005](adr/0005-one-request-per-case-four-targets.md)). A Run may use several sets, with distinct names: `--instructions` is repeatable, no flag means the bundled `neutral`, and any given path replaces it. The TUI also lists the `*.md` files of the optional config key `prompts_dir` ([ADR-0006](adr/0006-prompts-as-a-comparison-axis.md)).
- Two sets ship with the repository; only `neutral` is bundled into the binary. `neutral` (the default) uses the same neutral criteria for every model and carries no effortless policy rules. `effortless` is the file `prompts/effortless.md`, selected with `--instructions prompts/effortless.md`; it mirrors effortless's task text, effort rules and per-model levels, plus a **drafted Fable section** that has not been measured. Its task text quotes examples that match Cases in the dataset (for example c021 and c026), so its scores are in-sample.

### 5. Execution, failures, and evidence

- Default to **three independent logical trials scheduled per combination**, with configurable repetitions, timeouts and concurrency. Retries are bounded and configurable, not unlimited. Retry policy should identify retryable transport/status errors and distinguish them from valid but wrong predictions. Settled in #8: only a transport error (connect, timeout, read) or HTTP 408, 429, 500, 502, 503, 504, 524 or 529 is retried, after a pause that starts at 1 s, doubles up to 30 s, and is lengthened by a `Retry-After` given in seconds, up to 60 s ([ADR-0007](adr/0007-split-and-tuning-objective.md)); an HTTP-date `Retry-After` is ignored. An Attempt that received a `Retry-After` in seconds saves it as `retry_after_secs`, an optional field, so the schema stays 4. A complete 2xx reply is never retried, even when its effort is invalid; a 2xx whose body is cut off is a transport error and is retried. A Trial's outcome is its final Attempt.
- Every logical trial can contain one or more **physical Attempts**; record each attempt and its disposition. Do not discard timeouts/errors that precede a successful retry.
- A run may finish **complete**, **partial/cancelled**, or **failed**. Cancellation preserves completed evidence and failure details. Avoid presenting partial results as equivalent to complete runs. Settled in #8: **complete** means every planned Trial was recorded; **partial** means the run was cancelled, or its evidence has no end record (the process stopped); **failed** means the run stopped on an internal error such as an evidence write failure. Provider failures inside Trials do not fail a run; they are reliability observations. On cancellation, Trials already in flight finish, including their retries and the pauses between them (up to 30 s, or 60 s with `Retry-After`), so every recorded Trial is whole.
- Evidence for a Run is one append-only JSONL file (schema version 4): a manifest line (settings, Candidates with their price snapshot, Judge Instructions, dataset revision, selected targets, planned Trial count), one line per finished Trial with all its Attempts, and an end line with the Run state. A Trial line records the targets asked and a snapshot of the Case's labels at request time. Grades and costs are derived when the file is read, never stored. Schema 2 and 3 files are not readable (no backward compatibility), and the grading revision is 2.
- Provider errors, missing/invalid predictions, and wrong valid predictions have different meanings. Report **response correctness**, **coverage**, and **transport reliability** separately with honest denominators.
- Persist an immutable **Run** manifest and attempt-level evidence with schema version, dataset revision and hash, Judge Instructions name, content hash and full text, selected target models and labels, provider route + wire model identifier, repetition/attempt identity, relevant request and response metadata, wall-clock timing, error categories, usage (if provided), and grading revision.
- Keep credentials, auth headers and tokens **out of result files and logs**. API keys come from `OPENROUTER_API_KEY`/`OPENCODE_API_KEY` (a non-empty value wins) or from `credentials.toml` beside the config file (`openrouter = "…"`, `zen = "…"`; the TUI writes it with mode 0600); they are never stored in the config. Raw response retention is local by default; warn that benchmark prompts/context may be sensitive. Explicitly document safe redaction and export behavior.
- Reading an old run must not require a live provider or the TUI. Derived reports can be regenerated against an explicit new grading revision without mutating original evidence.

### 6. Scoring and reporting

- A valid predicted effort is a **hit** if it falls within that target's acceptable range.
- Otherwise **under** means the predicted effort is below the range, and **over** means above it. Ranges are contiguous, so there is no other kind of miss.
- **Unscored** means there is no reviewed label. **Invalid** means a response did not yield a valid choice. **Provider failure** means the request failed. All are distinct from valid scored misses. A target with no label is unscored whatever the response; invalid applies only to labelled targets.
- Report accuracy among **valid, scored predictions**, alongside eligible-case counts, invalid/unanswered counts, coverage, failure counts and run completion. Do not hide failures through denominator selection.
- Always break out performance by **Target Model** (with each target's sample size) and by **candidate/provider route** and **Judge Instructions** (name and hash), so instruction sets can be compared side by side. These per-Candidate and per-Target reports stay separate and never form a composite. The one exception is the labelled `pooled` row of the comparison matrix (§7, [ADR-0006](adr/0006-prompts-as-a-comparison-axis.md)); it is never a grade.
- The one number meant to be optimised is printed only by `score` (ADR-0007), never by `show` or `compare`: per Candidate × Judge Instructions and Target Model, hit rate over labelled predictions (invalid counts as a miss, a Trial with no valid reply is excluded and lowers coverage), the hit rate of the best single fixed effort on the same rows (floor), and their difference (lift). It is for tuning, not for ranking Candidates.
- Report **median and p95 latency** with clear definitions, include reliability, under/over counts, and repeat-to-repeat consistency. No arbitrary quality/latency/cost composite score. As implemented:
  - **Latency** uses nearest-rank percentiles (the value at rank ceil(p × n / 100)) and is reported twice: over the answered Attempts' `latency_ms`, and over the `elapsed_ms` of answered Trials (all Attempts plus pauses). It shows `n/a (n=0)` with no samples.
  - **Consistency**, per Candidate × Judge Instructions × Target Model: a Case counts when it has at least two valid efforts across repetitions, and is consistent when all of them are equal. Reported as consistent Cases out of counted Cases.
- **Cross-model metrics** are reported per Candidate × Judge Instructions, with denominators, as separate lines. They never replace the per-target splits. Both use only answered Trials and the label snapshot stored in the Trial.
  - **Joint hit:** among Trials with at least two labelled asked targets that all gave a valid effort, the share where every one of them hit.
  - **Gap agreement:** for each pair of labelled asked targets with valid efforts whose ranges do not overlap, the expected order is that the target with the higher range gets a strictly higher effort. It is reported as pairs that keep the expected order out of eligible pairs, counted across Trials. Equal efforts do not agree.
- A ratio with a zero denominator is shown as `n/a (0 …)`, never as 0%.
- A full report has these sections: Run, Summary (the comparison matrix of §7 for the one Run), Accuracy (with counts of hit, under, over, invalid and unscored per prediction), Cross-model, Consistency, Coverage and validity, Reliability (provider and transport outcomes, attempts, retried Trials), Latency, Cost, and Warnings (including one Judge Instructions name that appears with more than one hash).
- Configure **manual per-route** input and output token prices per million tokens. Snapshot the entered rates for a run. Compute observed costs only when all required usage/rate fields are present; otherwise mark missing/unknown. Do not fetch prices dynamically. Cost is informational, never an implicit grade. Settled in #10: prices are entered per Candidate in USD; the input rate is required, while the output rate is optional and defaults to zero (Decision models often charge nothing for output). A token count is required only when its rate is nonzero. A failed Attempt (non-2xx reply, or a transport error before any success status) is counted as costing nothing, on the user's assumption that providers do not bill failed requests; a transport error after a 2xx status (the body was lost) is costed like an answer, because the provider did the work, so it is unknown unless its token rates are zero; reports always state how many failed Attempts were counted this way. OpenRouter's own `usage.cost` is kept in the raw reply but not used.

### 7. CLI and TUI

- CLI must support selecting candidates, dataset and targets, running a single case or full selection, repetitions, execution controls, structured export, result inspection, and clear exit status; syntax may be finalized in an implementation ticket.
- Settled in #9, amended by [ADR-0006](adr/0006-prompts-as-a-comparison-axis.md): `show FILE` prints the full report of one saved Run. `compare FILE…` (two or more) lists everything that makes the Runs not directly comparable (non-complete Runs, dataset, grading revision, schema version, target set, Case selection, repetitions, unscored inclusion, Candidates present in only some Runs, Provider Route per Candidate) and then prints one matrix; it never states that unlike Runs are comparable, only which fields differ. Timeout, concurrency and retries affect reliability and latency, not accuracy, and are listed apart as `settings differ`. Judge Instructions are not a comparability item: on the prompt axis they are the columns. Likewise, when Candidates are the columns, Candidates present in only some Runs are not a difference (route differences still are), and the "no differences" line then omits "candidates".
  - **Matrix:** a column per **variant** and a row per combination of the other two of Candidate, prompt and Target Model, on one of two **axes**. On the *prompt axis* a variant is a prompt in a Run and the rows are Candidate × Target Model; on the *candidate axis* a variant is a Candidate in a Run and the rows are prompt × Target Model. The first column is the baseline. On the prompt axis a column is labelled with the prompt name, plus ` (<12-character SHA-256>)` when two columns share a name with different content, plus ` in <run id>` when the same content appears in two Runs. On the candidate axis a column is the Candidate name, plus ` in <run id>` when two columns share a name, and a row is the prompt name, plus ` (<12-character SHA-256>)` when two rows share a name with different content; rows are in the order the prompts are first seen across the Runs. A cell is `hit/valid scored (pct)`, or `-` where that Run did not ask the Candidate, Target Model or prompt. The text says which axis it uses (`columns: … (prompts)` or `(candidates)`), and the header row starts `candidate target` or `prompt target`.
  - **Axis choice:** `show` and `compare` take `--columns auto|prompt|candidate`, default `auto`. `auto` picks the prompt axis when the Runs between them contain more than one distinct prompt (name and content), otherwise the candidate axis when they contain more than one distinct Candidate name, otherwise the prompt axis. The TUI Results screen starts a report on `auto` and `c` swaps the two axes in both panels, whose texts stay those `show` and `compare` print with the same `--columns`.
  - **Paired delta:** a non-baseline cell adds `+g -l`, the paired Trials (Case × repetition) the baseline missed and this variant hit (gained), and the baseline hit and this variant missed (lost), for the same row (the same Candidate and Target Model on the prompt axis, the same prompt and Target Model on the candidate axis). Only pairs where both sides have a valid scored prediction count; with no pairs no delta is shown. The change is marked `~` (not significant) unless an exact two-sided sign test gives p < 0.05. The test looks only at Cases whose result changed: repetitions are collapsed per Case, so a Case counts once, as up if its repetitions gained more Trials than they lost, as down if fewer, and is ignored if equal; with `a` Cases up, `b` down and `n = a + b`, p is `min(1, 2 × P(X ≤ min(a, b)))` for X binomial(n, 0.5), and 1 when `n = 0`. A pooled cell (the pooled row, or a collapsed row) counts a Case once per Candidate (or per prompt, on the Candidate axis): the Case's net change is added across Target Models, because one response answers all of them, and `a` and `b` are counted on those nets. The 5% line is a convention, a matrix of many cells will show some significant deltas by chance, and Cases that stayed the same are not counted. It assumes the Runs used the same Cases and repetitions.
  - **Pooled row:** under the table, `pooled | all` adds the valid scored predictions over the rows present in every column (the same rows for every variant), with the summed delta, and a note that it is not a score. With no such row it is left out. It mixes Targets of different difficulty and is not a grade.
  - **`show`** opens its report after `== Run ==` with a `== Summary ==` section: the same matrix for the one Run (on the chosen axis). Its pooled row appears only when the Run has more than one row.
- `score FILE [--dataset PATH] [--allow-holdout]` prints the tuning score as JSON (§6). It needs the dataset the Run used (its SHA-256 must match the manifest), refuses a Run that includes a holdout Case unless `--allow-holdout` is given, and refuses a Run whose Judge Instructions contain the message of a Case of 20 or more characters (whitespace and case ignored). It also refuses evidence with a Trial for a Case, Candidate, Judge Instructions or Target the Run did not select, and prints the evidence file's warnings to stderr. `run --split train|holdout` selects Cases by split.
- `history [DIR]` lists saved Runs by creation time with id, state, recorded/planned Trials, Candidates and dataset hash, and names unreadable files.
- Settled for the TUI (`decisionbench tui`, with `--config` and `--out`): screens for Candidates, Dataset (read-only), Run setup, Run (progress, cancel) and Results/History.
  - **Results** opens a Run (or, with two or more marked, a comparison) on a Summary panel: the matrix above, with gained in the OK colour, lost in the fail colour and not-significant deltas dim; the numbers stay in the text, so colour is never the only sign. `[` and `]` switch to the Full report panel, which is the `show` or `compare` text. Both panels use the same cell text. The Summary opens *collapsed*: one row per Candidate (or prompt) with `all` in the target column, each cell adding up the targets that every column has, as the pooled row does, with the sign test run on the Cases, each counted once per label (a label with no such target shows `-`); the pooled row stays under them when they are more than one row. This hides differences between Targets, so `t` swaps it for one row per Target and back (the title says `targets pooled` when collapsed). `t` is TUI-only: the Full report panel and the CLI text are unchanged. A single Run that is not complete opens with a warning line giving its state and how many planned Trials were recorded, so a rate over the recorded Trials is not read as the whole Run's (a comparison lists such Runs among what differs). Marked Runs keep their mark order, shown `[1]`, `[2]`…; the first marked is the baseline. Enter on the Run screen after a run opens its Summary.
  - **Run setup** lists prompts instead of a single instructions field: the bundled `neutral` (chosen by default), prompt files added by path with `a` (validated on entry, relative to the folder the TUI was launched from) and removed with `d`, and a `prompt folder` row. The folder is the optional top-level config key `prompts_dir`, (a relative path resolves against the config file's folder; `~` is not expanded), which the TUI writes at once when it is set, keeping comments and Candidates and independent of unsaved Candidate edits. Its `*.md` files (not recursive, sorted by name) are listed and rescanned when Setup is shown and on `r`; a file that does not parse, or whose name repeats a listed prompt, is shown dim and cannot be chosen. There is no default folder. A dim `plan:` line above Start shows candidates × prompts × Cases × repetitions = Trials, up to N requests.
  - **Dataset** draws each labelled target's acceptable range as a bar across the five effort levels (a heavy line inside the range, a light dim line outside), with level names above it and the range text beside it when the pane is wide enough.
- TUI must allow selecting/managing candidates, browsing datasets and per-target labels, configuring and starting runs, observing per-trial status, cancelling, inspecting quality/reliability/latency, editing manual prices, and comparing run history.
- Both interfaces must present the same result semantics. Neither should silently mutate the dataset file. Editing/reviewing new ground-truth labels can initially be done in external versioned files; a label-editor UI is not required for the first release.
- An interactive network check is optional; offline fixtures and deterministic grading must work without provider keys.

## Testing decisions

**Principal test seam:** a complete benchmark invocation, using a known fixture provider and dataset selection, emits read-back-able versioned evidence and correct grades. Test observable behavior through the library/CLI rather than internal call counts.

**Secondary external seam:** provider adapters marshal requests and normalize representative responses from recorded fixtures. Use mocked/stubbed **external HTTP** only; do not mock internal business logic. Confirm at least one real per-provider smoke test separately when credentials/endpoints permit.

Tests should cover:

- Dataset validation: range syntax and order, unknown targets, efforts and fields, duplicate IDs, duplicate message/context, and missing/unscored labels.
- Identical test context with target Sonnet versus Opus; absence of current-effort anchoring.
- Range grading (hit, under, over), invalid outputs, unscored observations, and independent target-specific totals.
- Repeat counts, retry histories, transport timeouts, malformed responses, cancellation, partial-run files, resume/read behavior where supported, and deterministic output using controlled fixtures.
- Pricing math with manual rate snapshots, missing usage, missing rates, and zero prices explicitly entered by the user.
- Round-trip versioned data validation, unknown schema-version handling, credential/header redaction, and CLI/TUI agreement for the same saved run.
- Formatting, lint, unit/integration tests, and builds in CI. No live API calls required for the standard test suite.

## Out of scope for initial release

- Executing coding tasks or grading actual Claude task success.
- Embedding into effortless, Claude Code, OpenCode V2, or another agent harness.
- Reproducing effortless production fallback/retention logic inside the primary accuracy benchmark.
- Automatic ground-truth generation or one-step label conversion between target models.
- Direct TypeSafe and SubQ providers at launch; generic chat-completions provider adapters.
- HTTP server, REST API, permanent background service, Docker packaging, and an export contract or adapter for other tools (see [Issue #1](https://github.com/capyBearista/decisionbench/issues/1)).
- Cloud-hosted dashboard, browser frontend, user accounts, cloud synchronization, automatic price scraping, and a generalized benchmark marketplace.
- A full ground-truth editing/reviewing TUI or a plugin loading subsystem before a concrete need emerges.

## Verification gates and recommended implementation order

1. **Documentation and contract review:** approved domain vocabulary, architecture, and this spec.
2. **Rust foundation and offline case inspection:** a clean CLI build and deterministic dataset import/validation without network.
3. **First tracer bullet:** choose one candidate and one case, issue one real Decisions request, preserve raw evidence, grade and read it back.
4. **Second provider and paired targets:** validate distinct transport routes and target switching.
5. **Repetitions and resilience:** configurable batches, attempts/retries, cancellation and durable partial evidence.
6. **Reporting and manual cost:** trustworthy per-target metrics, history, missing-data semantics.
7. **Ratatui workflow and release verification:** full user path, CLI parity, offline tests and documented installation.

Each implementation slice should be **independently demonstrable end to end**, not a horizontal "all schema / all provider / all UI" handoff. Review the final ticket decomposition and blocking relationships **after** accepting this spec.

## Outstanding verification tasks (not permission to guess)

- Verify per-provider Decisions request/response schemas and how to omit or null the old `current_effort` state field. **Settled for OpenRouter** in #6/#7 and **for Zen** by live runs (see §4a).
- Verify effort choice options and response confidence/usage fields per route. **Settled for both routes** (see §4a); other models on a route may differ.
- Settle exact CLI flags, TUI keybindings, transport error taxonomy, and evidence schema field names during the relevant small implementation slices, while preserving the above domain-level invariants. **Settled for runs** in #8 (see §5 and the README); TUI keybindings are settled and documented in the README.

## Related work

- [effortless Issue #4: model-relative effort policy](https://github.com/capyBearista/effortless/issues/4) is a separate production-plugin concern.
- [decisionbench Issue #1: versioned evidence export for external tools](https://github.com/capyBearista/decisionbench/issues/1) is intentionally deferred.
