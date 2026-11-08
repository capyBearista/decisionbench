# HANDOFF — first-release verification (#12)

## Objective

- **Goal:** finish #12 (verify and document the first standalone release), under the umbrella spec #2.
- **Plan:** `/home/arjun/.claude/plans/plan-this-sequence-hint-nifty-reef.md` (local to the planning machine; its content is summarised below).

## Current state

- **Done:** PR #20 (wrap the TUI key hints between hints) merged into `main` as `913f15f`, CI and Devin Review green. The remote branch `fix/tui-hint-wrap` was kept.
- **In progress:** branch `docs/first-release` (README rewrite, SPEC and ADR-0001 text, `rust-version = "1.89"`, this file). Not yet pushed or reviewed when this was written.
- **Not started:** gap tests (step 3), issue edits, release record, clean-system run, spec review (step 4).
- **Open issues:** #12 (this work), #2 (umbrella spec; its body is stale), #1 (deferred evidence export for external tools; its body still names a private project and must be rewritten generically).

## Decisions and constraints (settled, do not reopen)

- A private project must not be named in this public repo or its issues. Remove it from #1, #2 and #12 and keep #1 as a generic "versioned evidence export for external tools" issue. Past commits and merged PR text still hold the name and are not rewritten.
- Edit the GitHub issues directly (`gh api -X PATCH repos/capyBearista/decisionbench/issues/<n>`; `gh issue edit` fails on the deprecated Projects-classic field), then show the user the final text.
- Hint wrapping is by whole hints (` · ` separators). The Candidates form hint is drawn inside its popup and is not wrapped.
- Matrix, sign-test and evidence decisions from PR #19 stand (ADR-0006, SPEC §6–§7). Evidence schema stays 4.

## Evidence found while auditing #12

- `cargo fmt --check`, `clippy --locked --all-targets -D warnings` and `cargo test --locked` pass (194 lib, 115 CLI, 20 TUI, 1 workflow after #20).
- Nine saved runs in `~/.local/share/decisionbench/runs` (all `complete` for schema 4). Four-target requests were answered over OpenRouter (`baseten/joey-1`, `perplexity/pplx-decider-flash-v1.2`) and Zen (`jev-1.13-free`); 120 of 120 Zen answers in the latest Zen run had a valid effort. The saved OpenRouter key appears in none of the files (spot check).
- Dependencies declare Rust 1.89 as the highest minimum; only 1.99.1 was tested.

## Remaining work

1. Review and merge `docs/first-release`; reply to Devin threads, never resolve them.
2. Gap tests, only where the existing tests prove a gap: a Sonnet-only and an Opus-only run end to end, cost unknown with a priced Candidate but no usage counters (CLI level), and that stdout and stderr of a keyed run never contain the key. First re-read `tests/cli.rs`; names alone were matched in the audit.
3. Edit #1, #2 and #12 (see above); tick #12's boxes only with evidence.
4. Post the release record as a comment on #12: date, commit SHA, Rust version, per-provider smoke result (ask before spending a fresh request), test counts, and what was not done.
5. Clean-system journey in a throwaway `HOME` and `XDG_*` (clone, `cargo build --locked --release`, `--help`, `dataset inspect`, `grade`, a stub-based run, `tui` start and quit); say it is weaker evidence than a fresh machine.
6. Spec review of the merged result with the code-reviewer agent; fix confirmed findings. Do not publish a release without approval.
7. Optional follow-ups (ask first): `grouped()` is called twice in `summary()` and `matrix_lines` (trivial); a per-group expand key.

## Pitfalls

- SSH works: `git push -u origin <branch>` over SSH succeeded on 2026-11-08. The old HTTPS workaround is no longer needed.
- Run commit, push and reply as separate commands; the auto-mode classifier has blocked combined ones.
- Reply to Devin threads; never resolve them (standing rule). Repeat `closes` before each issue number in PR bodies.
- Subagent shells may block `grep`; `GLOSSARY.md` edits are blocked for subagents (edit in the main session).
- The CLI test `transport_failure_exits_3_and_saves_evidence` uses a local server (closed ports hang in this sandbox).
- A heredoc with `<<'E'` keeps backslashes: do not escape backticks in a `gh pr create --body`.
- Do not commit `.env` or print keys.

## Resume instructions

- **Start here:** `cd /home/arjun/decisionbench && git status -sb && gh issue list --state open && gh pr list`.
- **Read first:** `docs/SPEC.md` (§5 evidence, §6–§7 reporting and UI), `README.md`, `docs/adr/0006-prompts-as-a-comparison-axis.md`, then issue #12.
