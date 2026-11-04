# HANDOFF — decisionbench: ticket #4 done, #5 (offline grading) next

## Objective

- **Original request:** Set up the Rust toolchain on this machine (Linux/WSL), then implement ticket [#4](https://github.com/capyBearista/decisionbench/issues/4) of spec [#2](https://github.com/capyBearista/decisionbench/issues/2). The work had to follow TDD and independent code review, and end in a PR. Contradictions were to be flagged, not silently resolved. No dependent tickets were to be started.
- **Next-session goal:** Implement [#5 — Grade offline effort predictions against dataset ranges](https://github.com/capyBearista/decisionbench/issues/5). It is now unblocked.

## Current state

- **Status:**
  - #4 is **completed**. [PR #13](https://github.com/capyBearista/decisionbench/pull/13) was merged with a merge commit, and GitHub closed #4 automatically.
  - #5 to #12 are open and not started.
- **Repo / branch / PR:**
  - `capyBearista/decisionbench`. Local `main` is at `bff8c0d` (the merge of PR #13).
  - The feature branch has been deleted locally and on GitHub. The only other remote branch is `origin/docs/initial-specification`, left over from PR #3; it was not touched.
- **Working tree / artifacts:**
  - Clean, apart from this file (`docs/HANDOFF.md`), which is uncommitted.
  - The one-off conversion script was used in a scratchpad and was deliberately not committed.
- **Verification:**
  - On the final commit, locally on Rust 1.99.1, all of these passed: `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo test --locked` (21 tests) and `cargo build --locked`.
  - GitHub Actions `check` passed on `7d2b5cc` ([run 41754418493](https://github.com/capyBearista/decisionbench/actions/runs/41754418493)).
  - Nothing has been run against `main` after the merge. The merge was conflict-free and has the same tree as `7d2b5cc` on top of `18a9941`.

## Decisions and constraints

All of these are settled by the user. They are recorded in [docs/SPEC.md](SPEC.md), [GLOSSARY.md](../GLOSSARY.md), [ADR-0002](adr/0002-raw-model-relative-evaluation.md) and [ADR-0003](adr/0003-owned-per-target-effort-ranges.md).

- **Dataset format (ADR-0003):** `datasets/cases.json` is a JSON array of `{id, message, context?, ok: {sonnet?, opus?}}`.
  - A label is a contiguous range string, either `"max"` or `"low-high"` (inclusive).
  - The effort scale is `low < medium < high < xhigh < max`, and every target accepts every level.
  - A missing target means **unscored**.
  - Every Case needs at least one label.
- **Targets:** Sonnet and Opus only. Haiku and Fable were explicitly rejected for now.
- **No session state anywhere:** the dataset has no current model and no current effort. The judge request sends the target model, but never the current effort. This matches [effortless #4](https://github.com/capyBearista/effortless/issues/4).
- **Dropped:** `kind`, `holdout`, source IDs and any traceability to the effortless upstream. The user considers the upstream file irrelevant.
- **IDs:** plain `c001`, `c002` and so on. They are stable and never reused, and carry no meaning.
- **Grades:** hit, under or over for valid predictions. Ranges have no gaps, so there is no "in-between miss". Invalid, unscored and provider failure are separate outcomes.
- **Platform:** Linux only, with no Windows or macOS CI or support (SPEC §1, tickets #11 and #12).
- **Licensing:** the effortless MIT license must stay with the data (`datasets/LICENSE-effortless`). This is a legal requirement, not traceability.
- **Merging:** the user chose a **merge commit**, not a squash, for PR #13. Ask about the strategy again for future PRs rather than assuming one.
- **`null` labels will be rejected (decided after the merge):** `"sonnet": null` is currently accepted and treated as absent, which means unscored. The user decided to reject it, so leaving the key out becomes the only way to mark a target unscored. This is the first step of #5 (now its first acceptance criterion), and SPEC §3 is updated in the same PR.

## Work completed

- **Toolchain:** rustup 1.29.3 and Rust stable 1.99.1, with clippy and rustfmt. Rust had already been installed at 1.91.1 and was only updated.
- **Code** (`8026c64`, `a837756` and `7d2b5cc`, merged in `bff8c0d`):
  - `src/dataset.rs`:
    - `Effort`, whose variant order is the effort order and which derives `Ord`.
    - `EffortRange { from, to }`, with a custom string `Deserialize`.
    - `Labels` and `Case`, both with `deny_unknown_fields`.
    - `parse_cases()`, which rejects an empty file, an empty or duplicate ID, a duplicate message+context, and a case with no labels.
    - `summarize()` and `BUNDLED_CASES`, which is `include_bytes!`.
  - `src/main.rs`: a clap command, `decisionbench dataset inspect [PATH]`. It exits 0 when valid, 1 on a read or validation error, and 2 on a usage error.
  - `tests/cli.rs`: 21 integration tests that run the real binary through `env!("CARGO_BIN_EXE_decisionbench")`. They use no network, credentials or extra test crates.
  - `.github/workflows/ci.yml`: ubuntu-latest, running fmt, clippy, test and build with `--locked`.
- **Data:** `datasets/cases.json` has 71 Cases, with 67 Sonnet labels and 8 Opus labels; 4 Cases have both.
  - It was converted from effortless `bench/judge-cases.json` at commit `99c03f2`.
  - The 4 identical message+context pairs were merged.
  - An independent reviewer checked all 75 source records against it by hand.
- **Docs:**
  - SPEC: status Approved, §2 adds `max`, §3 describes the dataset, §6 uses hit/under/over.
  - GLOSSARY: Source Case and Holdout removed; Label is now a range.
  - ADR-0002 is marked superseded in part. ADR-0003 is new.
  - README updated.
- **Tickets edited on GitHub:**
  - #4: new acceptance criteria.
  - #5: trimmed to grading and retitled.
  - #9: in-between misses and holdout views removed.
  - #11 and #12: Linux only, with no partition or provenance wording.

## Investigations, attempted approaches and pitfalls

- **A superseded intermediate approach** (commit `a837756`) kept the effortless fixture and reordered 10 out-of-order `ok` lists in place. `7d2b5cc` replaced it entirely with the owned per-target format. Do not revive the source-fixture, `SOURCE.md` or digest-test approach.
- **The upstream fixture no longer exists on effortless `main`.** It was removed in `7dc8103`, as the user intended when they retired `/effortless bench`.
- **Reviewer subagents cannot run anything.** `shared-workflow:code-reviewer` subagents found their sandbox blocked `cargo`, `gh`, `git diff` and `sha256sum`, so they review by reading the code. The parent must run every check itself and say so in the PR.
- **Use the REST API for GitHub writes.** `gh pr edit`, and `gh issue view` with some fields, fail with "Projects (classic) is being deprecated" (a GraphQL `projectCards` error). Use `gh api -X PATCH repos/capyBearista/decisionbench/{pulls|issues}/N -F body=@file` instead, and `gh api repos/.../issues/N --jq .state` to read state.
- **`cargo fmt` reformats test files after Write/Edit,** so re-read files before editing them again.
- **Known minor issues that were accepted:**
  - range errors report a line and column, not the case ID;
  - `print!` panics on a closed pipe (`| head`);
  - `current.effort` must never re-enter request types (#6).
- **#2's ticket map is a plain markdown checklist** that does not update itself. #4 has been ticked by hand; tick each later ticket the same way when it is done.

## Remaining work

1. **#5 — offline grading** (unblocked). The acceptance criteria are in the issue. **Step 1:** reject `null` target labels. Write a failing test first (for example `"ok":{"sonnet":null}` exits 1 with a clear message), then make the change in `src/dataset.rs`; `Labels` fields are `Option<EffortRange>`, which serde treats `null` as `None`. Add one line to SPEC §3 saying an omitted key is the only way to leave a target unscored. **Then:** The open design point is how an "offline fixture evaluation" supplies predictions. #6 introduces evidence files, so #5 needs a minimal local input. For example, a small JSON or JSONL of `{case id, target, raw effort}`, or a CLI subcommand that takes a prediction. Settle the shape with the user, or choose the smallest one and state it. Grading should be a pure function over `EffortRange` and `Effort` in `src/dataset.rs`, or a new `grade` module:
   - hit when `from <= e <= to`
   - under when `e < from`
   - over when `e > to`
   - invalid when the effort is not parseable
   - unscored when the target has no label
   
   Tests must cover range edges, `max`, single-level ranges, missing labels and denominators.
2. #6 to #12, in the order given by their **Blocked by** sections. Of these, only #6 directly follows #5.
3. Optional housekeeping, only if the user asks: commit or discard this HANDOFF.md.

## Resume instructions

- **Start here:**
  1. Run `cargo test --locked` on `main` to confirm the build is green.
  2. Read #5 (`gh api repos/capyBearista/decisionbench/issues/5 --jq .body`).
  3. Do step 1 (reject `null` labels), then ask the user how predictions should be supplied for offline grading before writing the grading code.
- **Read first:** `docs/SPEC.md` (§3, §4, §6), `GLOSSARY.md`, `docs/adr/0003-owned-per-target-effort-ranges.md`, `src/dataset.rs`, `tests/cli.rs`.
- **Suggested skills:**
  - `shared-workflow:implement`: the user's standard implementation flow.
  - `shared-workflow:test-driven-development`: required; watch each test fail first.
  - `shared-workflow:verification-before-completion`.
  - `shared-workflow:requesting-code-review`, then the `shared-workflow:code-reviewer` agent.
  - `shared-workflow:commit`.
  - `merge-pr`: only when the user asks.
- **Avoid repeating:**
  - Don't re-ask about targets (Sonnet/Opus), `max`, range format, IDs, dropping kind/holdout, Linux-only, or `null` labels (they are rejected).
  - Don't reintroduce `current_effort`, source provenance, or "in-between miss".
  - Don't start tickets beyond the one requested.
  - Don't push or open a PR without the user asking. Confirm the merge strategy each time.
