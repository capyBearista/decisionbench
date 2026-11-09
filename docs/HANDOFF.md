# HANDOFF — effort-selection accuracy: three stacked PRs ready to merge; live autoresearch loop next

## Objective

- **Original request:** raise how often Decision Models pick an acceptable reasoning effort. Four parts: (1) grow the dataset to about 200 Cases, (2) audit every label, (3) tune Judge Instructions with an autoresearch-style loop (https://github.com/karpathy/autoresearch), (4) test at least 5 distinct Decision Models.
- **Spec:** the approved plan `/home/arjun/.claude/plans/check-the-plan-audit-replicated-gadget.md` (outside the repo). Recorded decisions: `docs/adr/0007-split-and-tuning-objective.md`, `tuning/program.md`. The remediation plan for the last review round is `/home/arjun/.claude/plans/review-remediation-25-26-28.md` (outside the repo).
- **Next-session goal:** run the first live autoresearch loop (`tuning/program.md`) with the user, after they compact the session. Merging the stack is a separate decision the user makes; nothing is merged.
- **Not supported:** non-Decisions chat models (for example `liquid/lfm-3-1.7b:free`).

## Current state

- **Status:** all three PRs are implemented, reviewed, fixed, pushed and green. **Nothing is merged.** The user said "no merge yet" and wants merges only on request.
- **Repo / branch / PRs** (GitHub stack, order Base ← #25 ← #26 ← #28):
  - #25 engine: https://github.com/capyBearista/decisionbench/pull/25, branch `feat/split-and-score`, base `main`, head `af57b6a`. CI `check` and Devin Review pass. 5 reviewer threads are unresolved; I replied to each with the fixing commit and did not resolve them.
  - #26 dataset: https://github.com/capyBearista/decisionbench/pull/26, branch `feat/dataset-200`, base `feat/split-and-score`, head `01f82f7`. CI and Devin pass. 1 reviewer thread unresolved (title test, answered in `01f82f7`).
  - #28 tuning kit: https://github.com/capyBearista/decisionbench/pull/28, branch `feat/tuning-kit` (current branch), base `feat/dataset-200`, head `3a6f61b`. CI and Devin pass. GitHub reports 0 unresolved threads. I did not resolve any and did not check who did; treat it as unverified.
- **Working tree:** `docs/HANDOFF.md` (this file) is the only change. `label-review.json` has been removed by the user. `.env` and `target/` are gitignored. Remote and local `feat/tuning-kit` are both `3a6f61b` before this commit.
- **Verification actually run:**
  - PR #25 and #26: `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings` and `cargo test --locked` passed locally after each fix (196 unit, 135 CLI, 20 TUI, 1 workflow). Title test mutation check: dropping a count fails it.
  - PR #28: the 25 Python tests pass (`python3 -m unittest discover -s tuning`). Mutation check: a baseline-relative drop guard fails the new drop-guard test. `spent` run on the real two-model trial gives 605 requests and a next-run ceiling of 1,104 (1,104 = 2 models × 138 Cases × 1 repetition × 4).
  - Rehearsal run 1 (fake provider, scripted stand-in for the agent, eight sessions with crashes, a quota stop, a quota reset and a budget cut): all checks passed. 8,189 stub requests; `spent` counted 8,185; the 4-request gap is the Trials in flight at the one kill.
- **Not verified:**
  - A real Claude Code agent following `tuning/program.md`. The rehearsal used a scripted stand-in only.
  - Calibration with all seven models on real providers. Zen `jev-1.13` returned HTTP 429 (free quota used up) at last attempt.
  - Whether GitHub retargets #26 to `main` and #28 to `feat/split-and-score` when the lower PR merges. Unconfirmed.

## Decisions and constraints

- **Dataset:** 197 Cases, 138 train and 59 holdout (seed 20261108). The 66 original Cases keep their messages, contexts and splits. The audit revised 12 labels on 11 of them (c012, c014, c015, c024, c026, c028, c030, c032, c036, c052, c063). The user approved every revision. Labels are reviewed judgements, not measurements. Fable has only 4 holdout Cases at xhigh or max, and Opus has 8, so holdout results say little about those levels.
- **Tuning objective:** mean pooled lift over the eligible models; one Candidate per distinct Decision Model. Eligible means lift above max(2 × noise, 2 points) with coverage at least 90%. Keep a change when the objective rises by at least delta (max(1 point, 2 × mean noise)) and no eligible model falls by more than max(1 point, 2 × its own noise). Coverage under 90% means rerun, not score.
- **Retries are part of the compared selection.** Every run in a tuning session uses the baseline's retry count; `baseline` refuses calibration runs with different retries and `judge` refuses a run whose retries differ.
- **Budget (decided in review):** the budget is in requests and is never exceeded. `decide.py spent tuning/runs --budget N` counts every Attempt in the session's evidence (retries included). The next experiment is sized at its worst case: eligible models × train Cases × repetitions × (retries + 1). The loop therefore stops with some budget unused. Calibration's worst case is checked with the user during Setup, before it runs.
- **Stop and resume (user's choice):** on a provider quota limit the agent checks each low-coverage model with one request, stops, and leaves the `tune:` commit pending. A later session follows the Resuming section in `tuning/program.md`. Unlogged keeps are recorded first (no requests). The mid-loop model drop was rejected.
- **Holdout:** the agent never reads `datasets/cases.json` and never runs or scores the holdout. `score` refuses holdout Cases without `--allow-holdout`, but that is only a guard; the contract is the real rule. The holdout run is the user's, once, at the end.
- **Worktree:** the agent works in its own git worktree, so untracked files in the user's checkout (`label-review.json` and the like) never reach it.
- **Standing rules:** reply to Devin and reviewer threads, never resolve them unless the user says so; repeat `closes` before each issue number in PR bodies; merge without squashing and delete only the local branch; the user decides when to push, open PRs or merge.
- **Candidates:** live in `~/.config/decisionbench/config.toml` (user's file, not in the repo; it holds no keys). Passed one-request smoke tests: `joey-1`, `elephant-1`, `decision-fast:free`, `jev-1.14`, `d1-ultra`, `decider-v1.2` (OpenRouter). `jev-1.13` (Zen) is out of free quota. **Open:** whether `jev-1.13` and `jev-1.14` are distinct Decision Models. The user must set `decision_model` per Candidate.

## Work completed

- **#25 (engine):** `Case.split`, `run --split`, `score`, retry backoff with `Retry-After`, ADR-0007, SPEC/README. Review fixes in `af57b6a`: `score` checks the run's selected Cases (holdout refusal, unknown Cases), refuses Trials for unselected Cases, candidates, instructions or targets; evidence warnings go to stderr; an empty `--split` says so; README no longer claims `prompts/effortless.md` is refused (it quotes fragments only, so the check misses it).
- **#26 (dataset):** 197 Cases, revised labels recorded in ADR-0007 and the PR. Review fixes in `01f82f7`: the title test checks the exact title at every width 60–100; stale c024 comment; SPEC §3 provenance (first 66 Cases came from effortless, the other 131 were adapted or synthetic).
- **#28 (tuning kit):** `tuning/program.md`, `tuning/decide.py`, `tuning/test_decide.py` (25 tests), `tuning/prompt.md` (copy of `neutral`), README section, `.gitignore` (`tuning/results.tsv`, `tuning/best.json`, `tuning/runs/`), CI now runs the Python tests. Fixes: `judge --commit` records commit, run and evidence in `best.json` (`b75cc7d`); atomic writes; `decide.py` errors without tracebacks; `spent` budget and retry-ceiling sizing (`b75cc7d`, `3d7b18d`); retries in the selection (`3a6f61b`).
- **Real-provider trial (earlier):** calibration of `joey-1` and `decider-flash-v.1.2` on 138 train Cases: coverage 100%, lift about +15 points over the best fixed answer, zero noise. About 611 requests, about $0.0017. This was the old 2-model trial, before the retry-ceiling change.

## Investigations, attempted approaches and pitfalls

- **Devin marked "Resuming can discard the best prompt" outdated, but the bug was still there.** Outdated does not mean fixed. The fix is `judge --commit` plus the Resuming order in `program.md`.
- **Devin's budget findings (3 × 🔴):** a per-run check that counts only initial attempts lets retries overshoot; calibration was not budget-checked; resume blocked recording an unlogged keep. All three were real and are fixed in `3d7b18d` and `3a6f61b`.
- **The retry default in smoke tests:** the one-request gate uses the default retries, so a 429 can cost up to 4 requests. `spent` counts them, so the budget holds. Optional tidy-up: add `--retries 0` to the smoke command in `program.md`.
- **Harness bugs (my scripts, not the kit):** a read-after-truncate bug in the epoch file; a rehearsal check that assumed one request per gate probe. Both fixed; the kit did not need changes.
- **`git push` blocking:** the auto-mode classifier blocked a force push and a chained amend-and-push. The user ran the push themselves with `! git push …`. The user has since allowed plain pushes.
- **Force pushes** are needed after any rebase of the stack. Use `git push --force-with-lease --atomic origin feat/split-and-score feat/dataset-200 feat/tuning-kit`.
- **`pkill -f` can kill your own shell.** Kill by PID or port.
- **A `--locked` build fails after a version bump** until `cargo update --offline -p decisionbench`.
- **Editing eligibility in `baseline.json` after the first experiment** makes `judge` refuse the old `best.json`, by design.

## Remaining work

1. **Live autoresearch loop (next session, after compaction; user starts it).** Prerequisites the user supplies:
   - the Candidate list, one per distinct Decision Model, with `decision_model` set in the config (answer the `jev-1.13` vs `jev-1.14` question);
   - a request budget. Calibration's worst case is about 2 × passing models × 138 × 4 requests (up to about 7,700 with 7 models; about 1,900 expected without retries). Setup step 6 has the agent confirm this with the user first;
   - Zen quota reset if `jev-1.13` is wanted.
   Then follow `tuning/program.md` Setup steps 1–7 in a fresh worktree (`git worktree add ../decisionbench-tune-<tag> -b tune/<tag> feat/tuning-kit`), build `--release --locked` once, and begin the loop.
2. **Real-agent rehearsal (not done; optional but the only untested link).** Run a Claude Code agent against the fake provider to test that it follows `program.md`. The scripted rehearsal and the fake provider are in the scratchpad at `/tmp/claude-1000/-home-arjun-decisionbench/996488af-d00e-47fe-8742-f4b6bbae1589/scratchpad/loop2/` (`agent.py`, `rehearse.py`, `stub.py`). These are temporary and may be gone; `stub.py` needs port 8766 free.
3. **Merging (user decides):** bottom-up, #25 → #26 → #28, no squash, delete only the local branch. Retargeting after each merge is unconfirmed: check the base of the next PR, and `gh pr edit N --base main` if it did not move. `docs/HANDOFF.md` is the only uncommitted change; commit it only if the user asks.
4. **Holdout (user, once, at the end):** `--split holdout` once for `tuning/prompt.md` and once for `prompts/neutral.md`, then `score --allow-holdout` on each, with the same Candidates. Promote to `prompts/` only if the tuned prompt beats `neutral` by more than the noise (about ±6 points at 95% for 59 Cases).
5. **Cleanup:** delete `~/.local/share/decisionbench-private/raw.json` after #26 merges (it holds unredacted real messages).
6. **Unfiled, not blocking:** reject `"sonnet": null` labels (a validation change and one SPEC §3 line); a CI job on Rust 1.89; the `ctrlc` handler order; a TUI split selector; the optional `--retries 0` on the smoke command.

## Resume instructions

- **Start here:** `cd /home/arjun/decisionbench && git status -sb && for n in 25 26 28; do gh pr view $n --json state,baseRefName,headRefOid; done`, then ask the user for the Candidate list, the budget and whether `jev-1.13` is wanted, before any loop.
- **Read first:** `tuning/program.md` (the contract), `tuning/decide.py` (the arithmetic), `docs/adr/0007-split-and-tuning-objective.md`, the review remediation plan above, and PR #28's description.
- **Suggested skills:** `shared-workflow:merge-pr` when the user asks to merge; `shared-workflow:verification-before-completion` before any claim of done; `shared-workflow:respond-review` for new Devin or reviewer comments; `shared-workflow:requesting-code-review` for further changes.
- **Avoid repeating:** the dataset audit and review (done and approved); the stop-and-resume design debate (decided); the budget design debate (decided: worst case, never exceeded); resolving review threads without the user's say-so; running or scoring the holdout during tuning; committing `~/.local/share/decisionbench-private/`, the scratchpad, `label-review.json` or `.env`; pushing, opening PRs or merging without the user asking.
