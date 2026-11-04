# decisionbench: initial product and engineering specification

**Status:** Approved (2026-11-03)  
**Scope:** Initial standalone release  
**Sources:** Agreed design interview; [effortless source benchmark](https://github.com/capyBearista/effortless/blob/99c03f222d9a71dd1e0d2fa5824eb1cb59bbb20c/bench/judge-cases.json); [effortless judge implementation](https://github.com/capyBearista/effortless/blob/main/hooks/register.tsx); [deferred Valbench integration](https://github.com/capyBearista/decisionbench/issues/1)  
**Domain terminology:** [GLOSSARY.md](../GLOSSARY.md)  
**Architecture decisions:** [ADR-0001](adr/0001-standalone-engine.md), [ADR-0002](adr/0002-raw-model-relative-evaluation.md), [ADR-0003](adr/0003-owned-per-target-effort-ranges.md)

## Problem statement

Decision models can recommend the reasoning effort for an agentic assistant, but existing embedded benchmarking in effortless is limited to fixed judges, mixes model predictions with production fallbacks/shortcuts, and does not provide a general-purpose, reproducible way to compare model/provider routes.

The initial question is: **given the same task and relevant conversation context, which effort level should an independent Decisions model recommend for Claude Sonnet versus Claude Opus, and how accurately, consistently, reliably, and quickly does it make that recommendation?**

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
6. As a user, I can evaluate the same prompt against both Sonnet and Opus in independent requests.
7. As a user, I can supply independently reviewed acceptable effort ranges for each target model, not a derived one-step equivalence.
8. As a user, I can see that an unlabelled target/prompt is **unscored**, not silently graded right or wrong.
9. As a user, I can run one model against one case before committing to a larger batch.
10. As a user, I can choose repetitions per case/target/candidate, with three independent trials as the default.
11. As a user, I can set concurrency, timeout, and bounded retry policy and understand the resulting request volume.
12. As a user, I can cancel a run and retain all completed and attempted evidence with an explicit incomplete status.
13. As a user, I can inspect the raw provider answer, normalized effort prediction, confidence (when provided), provider errors, and parse failures separately.
14. As a user, I can grade a raw effort prediction without having previous effort or production policy alter it.
15. As a user, I can compare accuracy, under-selection, over-selection, reliability, and latency separately, including by target model.
16. As a user, I can manually enter per-route input/output token rates and optional per-request fees, without live price fetching.
17. As a user, I can see an observed-cost estimate only where sufficient usage and manual prices are present, with missing cost shown as unknown rather than zero.
18. As a user, I can review earlier runs and compare them only with transparent dataset, prompt, model-route, and evaluation revisions.
19. As a user, I can use the TUI to manage candidates, browse data, run benchmarks, review results, configure rates, and compare history.
20. As an automation caller, I can perform equivalent benchmark execution and read structured results via the headless CLI and versioned local files.
21. As a future Valbench integrator, I can consume an export with verifiable provenance without importing Rust internals or fabricating Harbor trials.

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

- Initially score only **effort selection** for the target model families **Claude Sonnet** and **Claude Opus**. Valid effort names are the explicit ordered set supported by the benchmark revision (initially `low` < `medium` < `high` < `xhigh` < `max`; every target model accepts every level); verify route-specific supported options during protocol research.
- A **Decision Model** selects effort; a **Target Model** is Sonnet or Opus. These are different roles.
- A **Candidate** includes the underlying Decision Model identity when known and its **Provider Route**. Jev via Zen, Jev via OpenRouter, and Jev via TypeSafe direct are different candidates, even if the model name matches.
- Initial provider transports: OpenCode Zen Decisions endpoint and OpenRouter Decisions API endpoint. Future transports such as SubQ and TypeSafe direct must be possible without changing the domain-level evaluation data; neither is implemented now.
- Preserve any model recommendation returned by a provider as **unscored evidence**; the initial grade covers effort only.
- Verify actual provider schemas, authentication, output shapes, usage availability, and null/omitted field behavior against primary documentation and representative responses. Do not infer compatibility from similarly named APIs.

### 3. Dataset and ground truth

- The benchmark owns **one dataset file**, `datasets/cases.json` (see [ADR-0003](adr/0003-owned-per-target-effort-ranges.md)). It was converted once from the MIT-licensed effortless benchmark cases, and the effortless copyright notice and license accompany it. The upstream file is not kept or tracked, and Cases carry no source identifiers.
- Each Case has a plain, stable **ID** (`c001`, `c002`, …) that never changes and is never reused, a task **message**, optional conversation **context**, and **labels** keyed by Target Model (`sonnet`, `opus`); every Case has at least one label.
- A label is a **contiguous range of acceptable efforts**, written as one level (`"max"`) or as `"from-to"` (`"low-high"` means `low`, `medium` and `high`). `from` may not be above `to`. Every effort in the range is equally acceptable.
- A Target Model may be absent from a Case; that combination is **unscored**. Omitting the target's key is the only way to leave it unscored; a `null` label is rejected. Do not synthesize missing labels.
- Cases carry **no session state** (no current model, no current effort) and no category or holdout markers.
- The initial conversion has **71 Cases** (four identical message/context pairs were merged) with **67 Sonnet** and **8 Opus** labels.
- Any change to Cases or labels produces a new **Dataset Revision**, identified by the file's content hash.

### 4. Evaluation requests and independence

- Each **Trial** is one **Candidate × Case × Target Model × repetition**; send an **independent Decisions API request** for each trial, never the full dataset as one request.
- For an exact Sonnet-versus-Opus paired comparison, use the same task, relevant conversation context, question wording, and evaluation settings; change the target Claude model identifier. Preserve any known provider-envelope differences as evidence.
- **Do not send the previous/current reasoning effort as a judging input**. Before implementing, establish whether each provider accepts omission or requires an explicit null; never substitute a meaningful effort that can anchor the judge.
- Relevant recent conversation context **does** matter, especially for follow-up/answer cases. Preserve and send it as defined by the dataset revision.
- No effortless-style short-follow-up shortcut, confidence fallback, bias, floor/ceiling, save mode, or automatic keep-current rule is applied in the **raw model-quality** benchmark.
- Parse and preserve the **raw effort prediction** separately from the normalized effort and any future **effective decision** produced by a selected policy. The initial accuracy score uses the raw normalized model prediction, not a post-policy decision.
- Mark an unsupported choice or missing effort as **invalid/unanswered**, not as correct or as a fallback effort.

### 5. Execution, failures, and evidence

- Default to **three independent logical trials scheduled per combination**, with configurable repetitions, timeouts and concurrency. Retries are bounded and configurable, not unlimited. Retry policy should identify retryable transport/status errors and distinguish them from valid but wrong predictions.
- Every logical trial can contain one or more **physical Attempts**; record each attempt and its disposition. Do not discard timeouts/errors that precede a successful retry.
- A run may finish **complete**, **partial/cancelled**, or **failed**. Cancellation preserves completed evidence and failure details. Avoid presenting partial results as equivalent to complete runs.
- Provider errors, missing/invalid predictions, and wrong valid predictions have different meanings. Report **response correctness**, **coverage**, and **transport reliability** separately with honest denominators.
- Persist an immutable **Run** manifest and attempt-level evidence with schema version, dataset revision and hash, judge task/prompt revision or hash, target model, provider route + wire model identifier, repetition/attempt identity, relevant request and response metadata, wall-clock timing, error categories, usage (if provided), and grading revision.
- Keep credentials, auth headers and tokens **out of result files and logs**. Raw response retention is local by default; warn that benchmark prompts/context may be sensitive. Explicitly document safe redaction and export behavior.
- Reading an old run must not require a live provider or the TUI. Derived reports can be regenerated against an explicit new grading revision without mutating original evidence.

### 6. Scoring and reporting

- A valid predicted effort is a **hit** if it falls within that target's acceptable range.
- Otherwise **under** means the predicted effort is below the range, and **over** means above it. Ranges are contiguous, so there is no other kind of miss.
- **Unscored** means there is no reviewed label. **Invalid** means a response did not yield a valid choice. **Provider failure** means the request failed. All are distinct from valid scored misses. A target with no label is unscored whatever the response; invalid applies only to labelled targets.
- Report accuracy among **valid, scored predictions**, alongside eligible-case counts, invalid/unanswered counts, coverage, failure counts and run completion. Do not hide failures through denominator selection.
- Always break out performance by **Sonnet vs Opus** (with each target's sample size; the initial labels are heavily Sonnet-weighted) and by **candidate/provider route**. Overall micro-average is optional and must not replace those splits.
- Report **median and p95 latency** with clear definitions (per successful attempt vs full logical-trial elapsed time); include reliability, under/over counts, and repeat-to-repeat consistency. No arbitrary quality/latency/cost composite score.
- Configure **manual per-route** input and output token prices per million tokens plus an optional fixed per-request fee. Snapshot the entered rates for a run. Compute observed costs only when all required usage/rate fields are present; otherwise mark missing/unknown. Do not fetch prices dynamically. Cost is informational, never an implicit grade.

### 7. CLI and TUI

- CLI must support selecting candidates, dataset and targets, running a single case or full selection, repetitions, execution controls, structured export, result inspection, and clear exit status; syntax may be finalized in an implementation ticket.
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
- Pricing math with manual rate snapshots, missing usage, missing rates, optional fees, and zero prices explicitly entered by the user.
- Round-trip versioned data validation, unknown schema-version handling, credential/header redaction, and CLI/TUI agreement for the same saved run.
- Formatting, lint, unit/integration tests, and builds in CI. No live API calls required for the standard test suite.

## Out of scope for initial release

- Executing coding tasks or grading actual Claude task success.
- Embedding into effortless, Claude Code, OpenCode V2, or another agent harness.
- Reproducing effortless production fallback/retention logic inside the primary accuracy benchmark.
- Automatic ground-truth generation or one-step Sonnet/Opus label conversion.
- Direct TypeSafe and SubQ providers at launch; generic chat-completions provider adapters.
- HTTP server, REST API, permanent background service, Docker packaging, and Valbench adapter/import implementation (see [Issue #1](https://github.com/capyBearista/decisionbench/issues/1)).
- Cloud-hosted dashboard, browser frontend, user accounts, cloud synchronization, automatic price scraping, and a generalized benchmark marketplace.
- A full ground-truth editing/reviewing TUI or a plugin loading subsystem before a concrete need emerges.

## Verification gates and recommended implementation order

1. **Documentation and contract review:** approved domain vocabulary, architecture, and this spec.
2. **Rust foundation and offline case inspection:** a clean CLI build and deterministic dataset import/validation without network.
3. **First tracer bullet:** choose one candidate and one case, issue one real Decisions request, preserve raw evidence, grade and read it back.
4. **Second provider and paired targets:** validate distinct transport routes and Sonnet/Opus target switching.
5. **Repetitions and resilience:** configurable batches, attempts/retries, cancellation and durable partial evidence.
6. **Reporting and manual cost:** trustworthy per-target metrics, history, missing-data semantics.
7. **Ratatui workflow and release verification:** full user path, CLI parity, offline tests and documented installation.

Each implementation slice should be **independently demonstrable end to end**, not a horizontal "all schema / all provider / all UI" handoff. Review the final ticket decomposition and blocking relationships **after** accepting this spec.

## Outstanding verification tasks (not permission to guess)

- Verify per-provider Decisions request/response schemas and how to omit or null the old `current_effort` state field.
- Verify effort choice options and response confidence/usage fields per route.
- Settle exact CLI flags, TUI keybindings, transport error taxonomy, and evidence schema field names during the relevant small implementation slices, while preserving the above domain-level invariants.

## Related work

- [effortless Issue #4: model-relative effort policy](https://github.com/capyBearista/effortless/issues/4) is a separate production-plugin concern.
- [decisionbench Issue #1: eventual Valbench integration](https://github.com/capyBearista/decisionbench/issues/1) is intentionally deferred.
