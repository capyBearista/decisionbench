# HANDOFF — decisionbench 1.0.0 released

## Current state

- **Done:** the first release, 1.0.0. #2 (umbrella spec) and #12 (first-release verification) are closed. The only open issue is #1 (deferred: a versioned evidence export for external tools).
- **Verification record:** the comment on #12 (https://github.com/capyBearista/decisionbench/issues/12#issuecomment-6717563472) says what was checked and what was not.
- **Evidence schema** stays 4; `rust-version` is 1.89 (declared, untested; CI runs `stable` only).

## Known gaps (not blockers)

- Not tried: a truly fresh machine, Rust 1.89, a fresh live provider success (a Zen request on 2026-11-08 hit the free-quota 429).
- Reviewer nits left open: the `ctrlc` handler is installed after the evidence file is created; the duplicate-hash warning in `show` can hardly fire; a redacted `raw_response` is not byte-exact; `DECISIONBENCH_TEST_BASE_URL` is read in release builds (documented).
- Optional ideas: a CI job that builds with Rust 1.89; a per-group expand key in the Results summary; `summary()` and `matrix_lines` both call `grouped()` (trivial).

## Decisions and constraints (settled, do not reopen)

- A private project must not be named in this public repo or its issues (already scrubbed; old commits and edit histories still hold it).
- Matrix, sign-test and evidence decisions: ADR-0006 and SPEC §6–§7. Labels are maintainer-approved drafts, not measurements (README, ADR-0003, ADR-0005). `prompts/effortless.md` is in-sample against the dataset.
- Key hints wrap between hints; the Candidates form hint is not wrapped.

## Pitfalls

- `gh issue edit` fails on the deprecated Projects-classic field; use `gh api -X PATCH repos/capyBearista/decisionbench/issues/<n>`. A quoted heredoc keeps backslashes, so do not escape backticks in a PR body.
- The auto-mode classifier sometimes blocks `git push`; run commit, push and reply as separate commands, and the user can push with `!`.
- Reply to Devin threads; never resolve them. Repeat `closes` before each issue number in PR bodies.
- `GLOSSARY.md` edits are blocked for subagents. The CLI test `transport_failure_exits_3_and_saves_evidence` uses a local server because closed ports hang in this sandbox.
- Do not commit `.env` or print keys.

## Resume instructions

- **Start here:** `cd /home/arjun/decisionbench && git status -sb && gh issue list --state open && gh pr list`.
- **Read first:** `README.md`, `docs/SPEC.md`, the ADRs, then issue #1 if work on the export begins.
