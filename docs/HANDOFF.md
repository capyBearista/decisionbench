# HANDOFF — decisionbench PR #18: four-target labels, credentials file, price cleanup

## Objective

- **Original request:** Finish PR #18 (`feat/17-9-11-four-targets-reports-tui`, closes #17, #9, #11): four Target Models (Haiku 5.5, Sonnet 5.5, Opus 5.5, Fable 5.1), reports and TUI. This session: review the user's exported label drafts, merge them into `datasets/cases.json`, drop `per_request`, add a credentials file with TUI key entry, close the case-ID gaps, then commit and push.
- **Next-session goal:** Watch CI and the Devin review on PR #18 and answer comments. Optionally run the live four-model runs. Merge only when the user says so.

## Current state

- **Status:** Implementation complete and verified locally. Commit and push were requested and done right after this file was written; confirm with `git status -sb` and `git log --oneline -8` (this file cannot record its own commit hash).
- **Repo / branch / PR:** `/home/arjun/decisionbench`, `capyBearista/decisionbench`, branch `feat/17-9-11-four-targets-reports-tui`, PR #18 (was open, CI green, mergeable before this session's commits). Push uses the explicit URL: `git push https://github.com/capyBearista/decisionbench.git feat/17-9-11-four-targets-reports-tui` (SSH fails here).
- **Working tree / artifacts:** An untracked `.env` exists (never commit it, never print it; `.gitignore` now has `*.env`). `datasets/label-drafts.json` is deleted by design.
- **Verification (run this session, fresh):** `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo test --locked`: 101 unit, 107 CLI, 8 TUI, 1 workflow tests pass. `cargo run -- dataset inspect` shows 66 cases, 66 labels for each of the four models, 0 unscored. The user ran the TUI by hand (candidates, dataset, setup, run, cancel, results, resize, quit, colours) and later tested the credentials prompt; both worked.
- **Not verified:** the live four-model runs; the `K` prompt after the very last layout fixes was only tested by the user once; no CI result yet for the new commits.

## Decisions and constraints

- **Labels** (user approved all 66 in the Label Triage artifact; the export was `label-drafts.json`). A label is a contiguous acceptable-effort range per target model. The model profiles come from community sentiment, not a measured effort sweep. Fable's planning/intent edge is an unmeasured assumption. Haiku is never "unscored". Ranges between models may not overlap; gap agreement treats non-overlap as a strict order claim.
- **English only.** The five Swedish cases (c006, c014, c028, c034, c040) were removed. To avoid gaps the last five cases were moved into those IDs: c071→c006, c070→c014, c069→c028, c068→c034, c067→c040. IDs now run `c001`…`c066`. This breaks the old "IDs never change or are reused" rule; the user accepted that and the docs now say IDs are stable once the dataset is released.
- **`per_request` price field removed** (user: unneeded). Config with `per_request` is rejected (`unknown field`). Evidence `SCHEMA_VERSION` is now **4**; schema 2 and 3 files are unreadable by design (no backward compatibility). The user's three candidates in `~/.config/decisionbench/config.toml` had `per_request` and needed it deleted (the user's TUI run shows they did).
- **Credentials** (user chose option C): non-empty env var (`OPENROUTER_API_KEY`, `OPENCODE_API_KEY`) wins, else `credentials.toml` beside the config file (`openrouter = "…"`, `zen = "…"`), else none (OpenRouter errors, Zen is fine). Mode 0600, atomic write, never in config or evidence. A broken credentials file is an error even when an env key is set (deliberate). On the Candidates screen, **`K`** (capital) opens a masked prompt; lowercase `k` stays "move up".
- Process rules from the user's global instructions still apply: root cause not workaround, plain language, state assumptions, ask before pushing to an open PR's branch unless told, no auto-resolve of Devin threads (reply only).

## Work completed

- `datasets/cases.json`: 66 cases, each labelled for all four targets (`ok` key); drafts file removed.
- `src/credentials.rs` (new), `src/provider.rs` (`Provider::key(&path)`, `KeySource`), `src/batch.rs` (`write_atomic`, `resolve_links`, key lookup in `prepare`), `src/tui/candidates.rs` (key status per row, `K` prompt), `src/tui/mod.rs` (`editing()`), tests in `src/tui/tests.rs`, `tests/cli.rs`, `tests/tui.rs`, `tests/tui_workflow.rs` (dataset counts now 66).
- `src/cost.rs`, `src/evidence.rs`: `per_request` removed, schema 4.
- Docs: `README.md`, `docs/SPEC.md` (§3 counts, credentials, schema 4), `docs/adr/0005-one-request-per-case-four-targets.md`, `GLOSSARY.md` (new "Credentials File", Case ID wording).
- An independent `shared-workflow:code-reviewer` found no blockers; six minor findings were fixed (temp file mode before write and cleanup on failure, numeric-value leak in parse errors, `file error` key status, 60-column layout, stale docs, deterministic env-override test). Skipped on purpose: per-redraw credentials read, prompt hint line, `batch`↔`credentials` module cycle.

## Investigations, attempted approaches and pitfalls

- `gh issue view N` fails on a deprecated Projects-classic field; use `--json`.
- `git status` showed the drafts as `MD` (staged edit, deleted in the tree): check `git diff --cached` before committing.
- The subagent shell hook blocks `grep` for some agents and `GLOSSARY.md` edits by the technical-writer agent; edit the glossary in the main session.
- `a_non_utf8_error_body_is_kept_as_a_provider_error` failed once under load in one agent run and passed on every rerun. Not investigated; if it fails in CI, treat it as a flaky test and look for a timing dependency.
- Known accepted limits: saving a key drops comments in `credentials.toml`; pasting a key that contains a newline presses Enter and saves early; an all-digits unquoted key is replaced by a generic error; TUI rename drops that table's comments; trials abandoned by a second cancel are not recorded.
- Third-party benchmark numbers used to shape labels were never verified.

## Remaining work

1. Check CI on the pushed commits (`gh pr checks 18`). Read new Devin/other review comments (`gh api` for PR 18 comments), verify each against the code, fix real issues, and reply per comment. Never resolve Devin's threads.
2. Optional, needs the user: live runs `decisionbench run --candidate joey-1`, with and without `--instructions prompts/effortless.md`, then `compare` (about 213 requests, about $0.002 each run). The `prompts/effortless.md` Fable section is an unmeasured draft.
3. Update the stale issue text in #2, #9 and #11 (they still say two targets and 71 cases) only if the user asks.
4. Merge PR #18 only when the user says so (use the `merge-pr` skill).
5. Follow-ups, not started: a controlled effort sweep per case and model to calibrate labels (the benchmark cannot yet express "no acceptable effort"); issue #12, first-release verification.

## Resume instructions

- **Start here:** run `git status -sb && git log --oneline -8 && gh pr checks 18` to confirm the push landed and see CI.
- **Read first:** `docs/SPEC.md` §3–§7, `docs/adr/0005-one-request-per-case-four-targets.md`, `GLOSSARY.md`, `src/credentials.rs`, PR #18.
- **Suggested skills:** `shared-workflow:respond-review` (answer review comments), `shared-workflow:verification-before-completion` (before saying anything is done), `shared-workflow:systematic-debugging` (any CI failure), `merge-pr` (only on the user's word).
- **Avoid repeating:** the label review (all 66 are approved), the `per_request` decision, the credentials design (option C, env wins, `K` key), the English-only decision, and the case-ID renumbering. Do not edit `.env`, do not print keys, and do not resolve Devin threads.
