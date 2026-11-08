# HANDOFF — feat/prompts-axis-results-ui: prompts and models as comparison axes, collapsed grid, sign test

## Objective

- **Original request:** After PR #18 merged, the user wanted the Results screen and `compare` to show prompt and model efficacy readably: several prompts per run, a prompt folder in Setup, a matrix with paired deltas and a pooled row, a candidate (model) axis with a toggle, a collapsed per-candidate grid (`t`), and a significance marker that is not fooled by small counts.
- **Next-session goal:** Respond to review on the draft PR, then mark it ready. The user tests the TUI themselves and reports problems.

## Current state

- **Status:** Implementation complete and verified. Committed; the PR is a draft (see the PR URL the opening session reported, or `gh pr list --head feat/prompts-axis-results-ui`).
- **Repo / branch:** `/home/arjun/decisionbench`, branch `feat/prompts-axis-results-ui`, based on `main` (`37b906c`, PR #18's merge). Push with the explicit URL: `git push https://github.com/capyBearista/decisionbench.git feat/prompts-axis-results-ui` (SSH fails here).
- **Last two code commits:** `b1dff62` collapsed summary and once-per-case sign test; `d31f613` docs for both.
- **Verification (fresh, this session):** `cargo fmt --check` clean; `cargo clippy --locked --all-targets -- -D warnings` clean; `cargo test --locked`: lib 191, CLI 115, TUI 20, workflow 1, all passed. `show` on the real run `1793857680687-313790.jsonl` is byte-identical with and without the collapsed-grid changes. An independent review found 0 critical, 0 important, 2 minor (doc wording, fixed). The two doc-only fixes came after the last full test run.
- **User checks:** the user looked at both Results views on that run and confirmed the numbers add up, and that `c` works. Not yet checked by the user: `[` `]`, scrolling in a short terminal, marking runs for compare, the hint line at narrow widths.

## Decisions and constraints

- **Matrix axes:** `--columns auto|prompt|candidate` on `show` and `compare`. Auto uses prompts when there are several distinct (name, hash) prompts across the runs, else candidates when there are several candidate names, else prompts. TUI `c` swaps the axis and rebuilds both panels; the Full report panel always equals the CLI text.
- **Collapsed by default:** the TUI Summary opens with targets pooled (one row per candidate or prompt, target column `all`). `t` expands and collapses. TUI-only; `show` and `compare` print the full table.
- **Significance:** `~` = exact two-sided sign test on the cases that changed against the baseline; p < 0.05 is significant. Repetitions collapse per case. A pooled cell (collapsed row or pooled row) adds a case's net change across targets and counts it once per row label (candidate, or prompt on the candidate axis), because one response answers every target. `+g -l` still counts paired trials. The 5% line is a convention.
- **Pooled row in the collapsed view** shows only when more than one row is displayed.
- **Settled, do not reopen:** prompts as columns with paired delta; pooled row over rows present in every column; TUI-only collapsed view defaulting to collapsed; sign test in this branch; no evidence schema change (stays 4).

## Work completed

- Engine (`src/report.rs`): axis-generic `Matrix::from_runs`, `Axis`, paired `Delta`, `sign_test_p` (log space), shared `pool()`, `grouped()`/`grouped_table()`, `Cell.case_nets`.
- CLI (`src/main.rs`): `--columns` on `show` and `compare`; exit 2 on a bad value.
- TUI: Setup prompt list and folder (`a` `d` `r`), plan line; Dataset effort bars; Results Summary and Full panels, mark order, `c` axis, `t` collapse, wrapped prose.
- Config: optional top-level `prompts_dir`.
- Docs: README, SPEC, ADR-0006 (with addenda), ADR-0004 pointer, GLOSSARY.
- Fixed this session: stale test `t_at_the_bottom_of_a_narrow_pane_keeps_the_last_row_in_view` (it asserted a `pooled =` note that a one-candidate collapsed view cannot show; it now checks that `k` scrolls one row up); stale doc passages (old noise rule, double-counting caveat).

## Pitfalls

- `gh pr edit` fails on the deprecated Projects-classic field; use `gh api -X PATCH repos/capyBearista/decisionbench/pulls/<n>`.
- Subagent shells may block `grep`; `GLOSSARY.md` edits are blocked for subagents (edit in the main session).
- The CLI test `transport_failure_exits_3_and_saves_evidence` uses a local server (closed ports hang in this sandbox).
- The user's pasted tables sometimes lose the trailing `│`; that is a paste artefact.
- Pooled `~` in `show`/`compare` text can differ from older commits because of the once-per-case rule; layout is unchanged.

## Remaining work

1. Handle review comments on the PR: verify each against live code, fix, re-verify, push, reply per finding. Reply to Devin threads, never resolve them.
2. Mark the PR ready when the user says so. Do not merge without the user's go-ahead.
3. **Follow-ups, not started:** the Results hint line (about 110 columns) is cut off below that width; a per-group expand key if the global `t` is not enough; `summary()` calls `grouped()` only for its length and `matrix_lines` calls it again (trivial); issues #2 and #12 status.

## Resume instructions

- **Start here:** `cd /home/arjun/decisionbench && git status -sb && gh pr list --head feat/prompts-axis-results-ui`.
- **Read first:** `docs/adr/0006-prompts-as-a-comparison-axis.md`, `docs/SPEC.md` §6–§7, `src/report.rs` (`pool`, `grouped`, `Delta`, `sign_test_p`), `src/tui/results.rs` (`View`, `toggle_targets`, `summary`).
- **Avoid repeating:** the sign-test choice, the candidate-axis design, and weakening tests to pass. Do not commit `.env`, print keys, or resolve Devin threads.
