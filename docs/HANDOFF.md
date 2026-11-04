# HANDOFF: decisionbench, #8 and #10 done; #9 next

## Objective

- **Original request (this session):** plan and implement [#8](https://github.com/capyBearista/decisionbench/issues/8) (batch Trials, retries, concurrency, timeout, cancellation, partial evidence) and [#10](https://github.com/capyBearista/decisionbench/issues/10) (manual route prices and costs) in **one PR**, part of spec [#2](https://github.com/capyBearista/decisionbench/issues/2).
  - The work used TDD, Fixer subagents for code edits, and independent review.
  - Contradictions were flagged, not changed silently in the spec.
- **Next-session goal:** not chosen yet. The likely options are:
  - [#9](https://github.com/capyBearista/decisionbench/issues/9) (accuracy, reliability, latency and history reports), now unblocked;
  - the Zen smoke check, once its quota resets.

## Current state

- **Status:** #8 and #10 are **complete and closed**. The user has ticked the #2 checklist themselves. #9, #11 and #12 are open.
- **Repo / branch / PR:**
  - `capyBearista/decisionbench`, `main`.
  - [PR #16](https://github.com/capyBearista/decisionbench/pull/16) was merged as merge commit `5d8672d`.
  - The local feature branch was deleted. The remote branch `feat/8-10-batch-runs-costs` was kept; delete it only if asked.
- **Working tree:** clean except this file.
- **Verification on the PR head `a1298e6`:**
  - `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo build --locked` and `cargo test --locked` all pass (36 unit and 82 CLI tests).
  - CI passed.
  - Devin Review's final pass found no new issues, and all 5 threads are resolved.
  - **Not verified:**
    - no live provider run of `run`; only local stubs were used;
    - the evidence write-failure path has no CLI test;
    - Zen has still never returned a successful reply.

## Decisions and constraints

Durable sources:
- [docs/SPEC.md](SPEC.md) §5 (retry taxonomy, run states, JSONL evidence) and §6 (cost rules);
- the [README](../README.md) (flags, exit codes, cancellation, cost);
- [GLOSSARY.md](../GLOSSARY.md) (Run states, Pricing Snapshot per Candidate);
- the decision note in #10's body.

User decisions made in this session:
- **`run` replaced `evaluate`.** Evidence schema 1 was dropped, with no backward compatibility.
- **Unlabelled Case × Target pairs are skipped** unless `--include-unscored` is given.
- **Prices go on each Candidate** in `[candidate.price]`, in USD:
  - `input_per_million` is required;
  - `output_per_million` and `per_request` default to 0.
- **Cost of failed Attempts:**
  - A failed Attempt (non-2xx reply, or a transport error before any status) counts as **$0, assumed unbilled**, and the summary always says how many.
  - A transport error after a 2xx status is costed like an answer. It is unknown unless its token rates are 0, in which case it costs the per-request fee.
  - An answered Attempt without the needed usage counts is **unknown**, never $0.
  - Known costs are labelled "priced attempts".
- **Defaults:** repetitions 3, concurrency 1, timeout 60 s, retries 1.
  - Retryable: transport errors and HTTP 408/429/500/502/503/504/524/529, after a fixed 1 s pause. `Retry-After` is ignored.
  - Never retried: a 2xx reply.
- **Plan order:** repetition-first.
- **Run states:**
  - `complete`: every planned Trial was recorded.
  - `partial`: the run was cancelled, or the file has no end line.
  - `failed`: an internal error, such as a write failure.
  - **Exit codes:** 0, 3 (some Trial failed), 130 (partial), 1, 2.
- **Cancellation (`ctrlc` with the `termination` feature, so SIGINT, SIGTERM and SIGHUP):**
  - First signal: in-flight Trials finish, retries included.
  - Second signal: exit 130 at once.
- **Process preferences:**
  - Fixer subagents (Sonnet, medium effort) make code edits, and code-reviewer subagents review.
  - Evaluate Devin comments, then discuss before editing.
  - The user approves merges. Use merge commits.
  - Use REST `gh api`, not `gh pr` or `gh issue view`, which fail with the Projects (classic) GraphQL error. GraphQL `resolveReviewThread` works.

## Work completed

- **Modules:**
  - `src/run.rs`: `plan` and `execute`, using thread-scoped workers and a channel to a single writer.
  - `src/cost.rs`: `Price`, `attempt_cost`, `CostTally`, `fmt_usd`.
  - `src/evidence.rs`: JSONL `Manifest`, `TrialRecord` and `EndRecord`; `read_run`; the summary `Display`.
  - `src/provider.rs`: `agent(timeout)`, `retryable`, `Candidate.price`.
  - `src/main.rs`: the `run` command and the signal handler.
- **Tests:** `tests/cli.rs` has a threaded, scripted stub with a peak-concurrency counter and real-signal cancellation tests.
- **Devin findings on #16:**
  - Fixed:
    - the cost of a reply cut off after 2xx when token rates are 0;
    - Candidates with no recorded Trials missing from the summary;
    - C1 control characters in the stderr copy of a line whose write failed;
    - the "answered" cost label.
  - Declined with a reply: the race where cancellation can let one more Trial start per worker. It can't be closed without aborting HTTP requests, and that Trial is still recorded.

## Investigations, attempted approaches and pitfalls

- **Key redaction is a plain substring replace.** A one-character test key garbles reply bodies, for example making the usage unreadable. Use realistic keys in manual stub runs.
- **The in-flight count in the cancel message is approximate**, and that is accepted.
- **Reviewer subagents can't run cargo** (a hook blocks it), so always re-run checks in the parent session.
- **Devin moves its old comment threads onto new commits** after a push. Compare comment IDs to spot genuinely new findings.
- **Zen:** the free quota returned 429 with `Retry-After` of about 20.8 h on 2026-11-03, so it resets around 2026-11-04 23:00 UTC. A successful reply's shape and its usage fields are still unknown, so Zen costs may show as unknown.

## Remaining work

1. **Zen smoke check** (after the quota resets):
   - Run `decisionbench run --candidate <zen free> --case c002 --target opus --repetitions 1`.
   - If it returns 200 with `answers.effort.choice`, update SPEC §4a.
   - If the shape differs, add that path to `provider::normalize`, with the real body as a test fixture.
2. **#9 reports.** Read the issue first. Its pieces:
   - accuracy per Candidate × Judge Instructions × Target, with denominators;
   - a warning when one instruction name appears with several hashes;
   - coverage and reliability;
   - median and p95 latency, using the per-attempt `latency_ms` and the per-Trial `elapsed_ms` already stored;
   - consistency between repetitions;
   - mismatch warnings across runs.

   Reuse `evidence::read_run`.
3. **Later:** #11 (TUI, which needs #9), then #12.
4. **Optional:**
   - a live OpenRouter `run`, with the user's OK because it costs money;
   - delete the merged remote branches, if asked.

## Resume instructions

- **Start here:** ask the user whether to do #9 or the Zen check first (check the time against the quota reset). Read the chosen issue with `gh api repos/capyBearista/decisionbench/issues/N --jq .body`.
- **Read first:**
  - docs/SPEC.md §5–§6
  - GLOSSARY.md
  - docs/adr/0004-judge-instructions-axis.md
  - src/evidence.rs
  - src/run.rs
  - tests/cli.rs (the stub helpers and `Eval`)
- **Suggested skills:**
  - `shared-workflow:implement`
  - `shared-workflow:test-driven-development`
  - `shared-workflow:requesting-code-review`
  - `shared-workflow:verification-before-completion`
  - `merge-pr`, only when asked.
- **Avoid repeating:**
  - Don't re-ask any decision listed above.
  - Don't reintroduce `evaluate`, `current_effort` or a candidate `url`.
  - Don't honour `Retry-After`.
  - Don't treat the Zen 429 as a code failure.
