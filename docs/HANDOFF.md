# HANDOFF — effort-selection accuracy: tuning done, tuned prompt shipped

## Objective

- **Original request:** raise how often Decision Models pick an acceptable reasoning effort. There were four parts: grow the dataset to about 200 Cases, audit every label, tune the Judge Instructions with an autoresearch-style loop (https://github.com/karpathy/autoresearch), and test at least 5 distinct Decision Models.
- **Next-session goal:** none is set. All four parts are done and merged. What's left is optional cleanup and a list of possible next tasks (see Remaining work). Ask the user which, if any.

## Current state

- **Status:** completed and merged. PRs #25, #26, #28, #29 and #30 are all in `main`.
- **Repo:** `capyBearista/decisionbench`.
  - `main` is at `7a17472` (#30 merged), and that commit includes #29 (`af7c005`).
  - This handoff is committed on branch `docs/handoff-tuning-done`, pushed, with no PR.
- **What's on disk:**
  - The untracked `.serena/` in the main checkout. Not ours, so leave it.
  - Merged local branches, none deleted: `feat/tuning-noise`, `feat/tuned-prompt`, `docs/smoke-test-retries`. The remote branches are kept.
  - Worktree `/home/arjun/decisionbench-tune-nov8` (`tune/nov8`): the first loop, at 1 repetition.
  - Worktree `/home/arjun/decisionbench-tune-nov8b` (`tune/nov8b`): the second loop, at 3 repetitions. Its untracked files hold the evidence:
    - `tuning/results.tsv` and `tuning/best.json`.
    - `tuning/runs/`: all train evidence. Its `tools/` subfolder has the helper scripts `exp.sh`, `ev.py` and `cmp.py`, and the probe variants are in `tools/var/`.
    - `holdout/`: the holdout evidence and its scores.
- **Verification run:**
  - #29: 29 Python tests pass. With the spread term removed, 2 tests fail. `misses` output is byte-identical to before on real evidence. `baseline` runs end to end on the real nov8b calibration (all 6 models eligible, delta 0.5 points).
  - #30: `cargo fmt --check`, `clippy -D warnings` and `cargo test --locked` (352 tests) pass.
  - CI and Devin's review passed on both PRs.
- **Not verified:** the new `decide.py` rules (noise from the spread, the 0.5-point delta, the probe rules) have not been used in a live loop. A real agent following `tuning/program.md` from Setup was tested only by this session.

## Decisions and constraints

- **Current rules** are in `tuning/program.md` and `tuning/decide.py`:
  - Noise is the larger of two things: the difference between the two calibration totals, and the spread of the hits over every repetition.
  - Delta is the larger of 0.5 points and twice the mean noise.
  - The drop guard is the larger of 1 point and twice the model's own noise.
  - Probes: one repetition on the train split. They're never judged, they need a `spent` check first, and they count toward the stop rule.
- **The holdout has been used once** (for `042e79e`, the nov8b prompt). Under `program.md`, tuning sessions nov8 and nov8b are over, so don't resume them. Any more tuning against the same holdout weakens it. An honest second round needs new holdout Cases.
- **`prompts/tuned.md` is an extra prompt, not the default.** `neutral` is still the default and the only prompt built into the binary (user decision). The README caveats are approved by the user.
- **Candidates:** joey-1, decider-v1.2, elephant-1, decision-fast:free, jev-1.14 and d1-ultra, all on OpenRouter.
  - jev-1.13 (Zen free) was dropped, because it kept returning HTTP 429.
  - jev-1.13 and jev-1.14 are distinct models (user's answer).
  - `decision_model` isn't needed in the config.
- **Standing rules:**
  - Reply to Devin threads, and never resolve them.
  - Discuss review points before editing.
  - Merge with merge commits, and only on request.
  - Keep remote branches.
  - Repeat `closes` before each issue number in PR bodies.

## Work completed

- **nov8:** 1 repetition, 19 experiments, objective 20.15 → 29.35.
- **nov8b:**
  - **Setup:** 3 repetitions, started from the default prompt, 100k budget, 48,186 requests used, plus 2,124 for the holdout.
  - **Results:** 4 keeps out of 12 full runs, plus 23 probes. Objective 20.27 → 28.14.
  - **Kept changes:**
    1. Each section says how strong its model is.
    2. "Ordinary focused work is low" for Opus and Fable.
    3. A relative Haiku rule (one level more for real work).
    4. A Haiku `high` line describing a normal task, and a Fable `low` line covering everyday work.
- **Holdout** (59 Cases): tuned prompt vs `neutral`, gain in pooled lift, averaged over the six models:
  - Mean +9.8 points, against +7.9 on train.
  - Per model, from +5.1 (joey-1) to +12.6 (decider-v1.2).
  - Per target: Haiku +9.9, Sonnet +0.4, Opus +8.7, Fable +20.4.
  - Tables are in the PR #30 description.
- **#29:** a better noise estimate, the delta floor lowered to 0.5 points, probes added to `program.md`, and `--retries 0` for smoke tests.
- **#30:** `prompts/tuned.md`, README and SPEC entries, and the parse test extended to it.

## Investigations, attempted approaches and pitfalls

- **The models read different parts of the prompt.** This is the key finding:
  - **decider-v1.2** applies a rule to every target when the rule names effort levels ("at least high"). A relative rule ("two levels more") stays with its own target.
  - **joey-1** reads only the effort lines, never the sentences. It's also almost blind to the target: it picks the same level for Sonnet and Opus on 126 of 138 Cases.
  - **d1-ultra** reads both, so a sentence and a line pushing the same way stack and overshoot.
- **Determinism:** joey-1 and elephant-1 always give the same answer, so for them one probe is exact. The others vary between calls (d1-ultra changed 61 of 552 predictions across 3 repetitions).
- **Mistakes to avoid repeating:**
  - **I misdiagnosed the nov8 stall.** I blamed the drop guard, but the late discards had negative gains. The real cause was edits too small to clear delta.
  - **Examples taken from misses.** nov8 used examples copied from miss messages ("push, merge, rename"), which `program.md` forbids. nov8b used invented examples.
  - **Ten-discard rule miscounted.** I stopped nov8 at 9 discards in a row, not 10.
- **WSL restarts wipe `/tmp`,** including the session scratchpad. Keep helper scripts in a gitignored folder of the worktree.
- **Stacked PRs on GitHub** need the async merge API (`PUT .../pulls/N/merge-async`). Merging a lower PR rebases and retargets the next one. Its head SHA changes, so check that the file tree is unchanged before merging it.

## Remaining work

None of this is required, and none of it has been requested.
1. **Cleanup (user):**
   - Delete `~/.local/share/decisionbench-private/raw.json`.
   - `git worktree remove` the two tuning worktrees once their evidence isn't needed. The holdout scores are in `nov8b/holdout/`.
   - `git branch -d` the three merged local branches.
2. **Unfiled ideas:**
   - Reject `"sonnet": null` labels.
   - A CI job on Rust 1.89.
   - The order of the `ctrlc` handler.
   - A split selector in the TUI.
   - Maybe a `decide.py compare` command for probes. This session used ad-hoc scripts instead.
3. **A future tuning round** needs new holdout Cases first (dataset work by the user). It also has to deal with joey-1's blindness to the target, which no wording fixed in four probes.

## Resume instructions

- **Start here:** `cd /home/arjun/decisionbench && git switch main && git pull --ff-only && git status -sb`. Then ask the user what to do next.
- **Read first:** `tuning/program.md`, `README.md` (the Judge Instructions section and Known limitations), and the descriptions of PRs #29 and #30.
- **Suggested skills:** `shared-workflow:respond-review` for new Devin comments; `shared-workflow:merge-pr` for merges on request.
- **Avoid repeating:**
  - Resuming nov8 or nov8b. Their tuning is over.
  - Running the holdout again for tuning.
  - Changing the default prompt without the user's say-so.
  - Resolving review threads.
