# Tuning program

You are an autonomous researcher improving one file, `tuning/prompt.md`: the Judge Instructions sent to Decision Models. The goal is to raise how often the models pick an acceptable reasoning effort. The method follows [autoresearch](https://github.com/karpathy/autoresearch): edit, run, measure, keep or discard, repeat. Read this whole file before you start. It is the contract.

## What is fixed, what you may change

| You may | You may not |
|---|---|
| Edit `tuning/prompt.md` (and commit only that file during the loop) | Edit anything else: the Rust code, `tuning/decide.py`, `tuning/baseline.json`, `prompts/`, the config, the dataset |
| Read `tuning/`, `docs/`, `README.md`, the config file (it holds no keys), and evidence files of **train** runs you made | **Open `datasets/cases.json`**, in any way (`cat`, `grep`, an editor, a script). Do not pass `--dataset`. |
| Run `run`, `score`, `show`, `history` and `tuning/decide.py` | Run `--split holdout`, a run with no `--split`, or `score --allow-holdout`. The holdout is scored once, by the user, at the very end. |

Why: the Cases in `train` are for finding what to change. The `holdout` Cases are the only honest measure of whether a change generalises. Reading them, or tuning against them, makes that measure worthless. The binary refuses some of this (`score` rejects holdout runs and prompts that quote a Case message of 20 or more characters), but you are the real guard. If you notice that you saw holdout data, say so and stop.

Rules for the prompt text:

- Keep the format: an intro paragraph, then one `## haiku`, `## sonnet`, `## opus`, `## fable` section in that order. Each section is an instructions paragraph followed by exactly five non-empty lines, `- low: …`, `- medium: …`, `- high: …`, `- xhigh: …`, `- max: …`, in that order. A broken file makes `run` exit 1.
- Write general criteria. Never paste or paraphrase a specific Case message, and never mention a Case id. An example you invent yourself is fine if it is not lifted from a Case you saw in the misses output.
- Prefer a simpler prompt. A change that gains almost nothing but adds many lines is not worth keeping. A change that gains the same with fewer lines is a win.

## Setup (do this once, with the user)

1. **Worktree.** Agree a tag with the user (today's date, for example `nov8`) and the committed branch to start from. Work in a fresh worktree, so that untracked files and uncommitted edits in the user's checkout (a review export, notes) are not in your tree and `git reset --hard` cannot touch the user's work: `git worktree add ../decisionbench-tune-<tag> -b tune/<tag> <branch>`, then `cd` there. The branch must not exist yet. Read nothing outside this worktree and the evidence folder, and do not read git history for the dataset (`git show`, `git log -p`) or inspect the binary for its embedded data.
2. **Binary.** `cargo build --release --locked` once. Use `BIN=target/release/decisionbench` from now on. Do not rebuild: the dataset is compiled in, and a rebuild after a dataset change would change the scored Cases.
3. **Candidates: the gate.** Ask the user for the Candidate names, one for each *distinct Decision Model*. Read the config to check them. Two routes to the same `decision_model` count once; keep one. Only Decisions endpoints are supported. Then smoke-test each: `$BIN run --out tuning/runs --candidate NAME --split train --case c001 --target opus --repetitions 1 --retries 0` (exactly one request; a train Case, and always with `--split train`). Drop any Candidate that does not exit 0. Never use these smoke-test files for scoring. If fewer than 5 distinct models pass, **stop and tell the user**. They decide whether to continue with fewer. Fewer than 2 means no loop.
4. **Budget.** Ask the user for the most requests you may spend, and write it down. Every run of this session (smoke tests, calibration, experiments, reruns, the one-request checks) writes its evidence to `tuning/runs` (`--out tuning/runs`, gitignored), so the evidence counts what was spent, not you: `python3 tuning/decide.py spent tuning/runs --budget N` prints the requests sent (every Attempt, retries included), what is left, and the most the next experiment can send (eligible models × train Cases × repetitions × (retries + 1), the same ceiling `run` prints as "up to N requests"), and exits 13 when what is left cannot cover it. It needs `tuning/baseline.json`, so it applies from the loop on. One experiment usually sends about *models × train Cases × repetitions* requests; the ceiling is up to four times that with `--retries 3`, so the loop stops while some budget is still unused. That is the price of never going over. The count can be low by the Trials in flight when a run was interrupted (at most concurrency × (retries + 1) requests).
5. **Settings.** Use `--split train --repetitions 1 --concurrency 4 --retries 3` for every run below, so that runs stay comparable. Free tiers rate-limit; backoff is built in, and low coverage is handled by the rules below. If calibration shows a mean noise of 3 points or more (step 6), repeat the calibration once with `--repetitions 3` and use 3 from then on. Put the flags in a shell variable and use it for every run: `FLAGS="--out tuning/runs --split train --instructions tuning/prompt.md --repetitions 1 --concurrency 4 --retries 3"`. Record the flags in the `results.tsv` preamble (step 7), so that a later session can rebuild them.
6. **Calibrate.** First check the cost with the user. Calibration is the largest spend: two runs, each up to *passing Candidates × train Cases × repetitions × (retries + 1)* requests (`$BIN dataset inspect` prints the train count). Add the smoke tests already sent (`decide.py spent` cannot run before the baseline exists; count one request per smoke test). If the budget cannot cover both runs at that ceiling, tell the user and stop: they raise the budget or name fewer Candidates. Check again, with the requests now spent, before a repeat with `--repetitions 3` (step 5). Then run the unchanged prompt twice with every passing Candidate, and score both. `run` prints `evidence: PATH` on its last line.

   ```sh
   $BIN run $FLAGS \
        --candidate A --candidate B --candidate C --candidate D --candidate E
   $BIN score PATH_1 > /tmp/score_a.json
   # run the same command again, then:
   $BIN score PATH_2 > /tmp/score_b.json
   python3 tuning/decide.py baseline /tmp/score_a.json /tmp/score_b.json --runs PATH_1 PATH_2
   ```

   `decide.py baseline` writes `tuning/baseline.json` and prints, per model, its neutral lift, its noise (how far its lift moves between two runs of the same prompt: the difference between the two calibration runs, or, when larger, how often each prediction turns between hit and miss over every repetition, since two totals can match while hits change underneath) and whether it is **eligible**. Eligible means the lift over the best fixed answer is more than twice the noise and more than 2 points, with coverage of 90% or better. It also prints `delta`, the smallest objective gain that counts: half a point or twice the mean noise. A model whose hits never change has noise 0. It exits 12 when fewer than 2 models are eligible. In that case **stop and report**: there is no signal to tune against. Exit code 3 from `run` only means some Trials had no reply; keep going, and `score` will show the coverage.
7. **Freeze.** Show the user `tuning/baseline.json`: models, eligibility, noise, delta. If they want to change an eligibility call, they edit the file now (`decide.py` recomputes the objective and delta from the per-model fields). The file is fixed once the first experiment has run; changing it later makes `judge` refuse the old `tuning/best.json`, and you start over. Then `git add tuning/baseline.json && git commit -m "tuning: baseline"`. Create `tuning/results.tsv` (untracked) that starts with a preamble of `#` lines, the state a later session needs to resume: `# worktree=<absolute path>`, `# branch=tune/<tag>`, `# bin=<absolute path of the binary>`, `# flags=<the FLAGS string>`, `# budget=<requests>`. Then the header `commit	objective	gain	verdict	evidence	description`. Log the baseline as the first row (verdict `baseline`).

From here on, only the **eligible** models are run (`--candidate` for each of them, nothing else). The objective is the mean of their pooled lift. You never compute it yourself; `decide.py` does.

## The loop

Repeat until you reach a stop condition.

1. **Look.** Find what the current prompt gets wrong on the train set: `python3 tuning/decide.py misses PATH --limit 40`, where PATH is the run of the *current* prompt: the latest kept run (the `evidence` column of `results.tsv`), or calibration run 1 at the start. After a discard, the latest run belongs to a prompt that no longer exists; do not use it. It lists a sample of the missed Trials (with the Candidate) and counts of under- and over-predictions per target. Use it to form one hypothesis, such as "the models pick `low` for multi-step edits" or "xhigh is chosen for ordinary refactors". Under-predicting is the common failure, but check the counts.
2. **Edit** `tuning/prompt.md` to test that one hypothesis. One idea per experiment, so that a keep or discard tells you something.
3. **Commit** only the prompt: `git add tuning/prompt.md && git commit -m "tune: <what and why in one line>"`.
4. **Run.** First check the budget: `python3 tuning/decide.py spent tuning/runs --budget N`. Exit 13 means stop (the budget is used up). Then run the same command as in calibration with the eligible Candidates, in this form, so that the progress lines stay out of your context and the exit status stays visible:

   ```sh
   $BIN run $FLAGS --candidate A --candidate B > /tmp/run.log 2>&1; echo "exit $?"
   grep '^evidence:' /tmp/run.log
   ```

   The path after `evidence:` is this experiment's run. If `run` exits 1, read `/tmp/run.log` first: a malformed prompt (for example a section without exactly five effort lines) means fix the prompt, `git commit --amend` and run again; a config or key problem, or an evidence write error, means stop and report.
   Exit 3 is fine, see above. Exit 130 means the run was interrupted; run it again.
5. **Score and judge.**

   ```sh
   $BIN score PATH > /tmp/score.json
   python3 tuning/decide.py judge /tmp/score.json --run PATH --commit $(git rev-parse HEAD)
   ```

   `--run` is the evidence file that was scored; `decide.py` reads its manifest to check that it covers the same Cases, targets, repetitions and retries as the baseline, which `score` alone cannot show. `judge` compares the run with the best state so far (the baseline at first) and exits:
   - `0` **keep**: the objective rose by at least delta, and no model fell by more than twice its noise (at least 1 point). Leave the commit in place. `judge` has recorded the new best in `tuning/best.json`, with the commit, the run id and the evidence path, so a resume can tell that this commit was kept even if the session stops before step 6.
   - `10` **discard**: `git reset --hard HEAD~1`.
   - `11` **rerun**: the run is not usable (it is partial, a Trial is missing, or a model's coverage is under 90%). Do not score it and do not count it as an experiment. `judge` names the low-coverage models in its reason. Check the budget, then send one request to each of them, as in the smoke test, before rerunning. If any fails (a 429 usually means the free quota is used up), **stop now**: leave the experiment's commit in place, unjudged, and report (see Resuming). Otherwise run the same commit again. If it comes back unusable three times in a row, stop in the same way.
   - `1`: bad input (a different dataset, other Cases, targets, repetitions or retries than the baseline, an evidence file that is not the scored run, or a missing model). Stop and report.
6. **Log** one row to `tuning/results.tsv`: short commit hash, objective, gain, verdict (`keep`, `discard`, `rerun`), the evidence path, and the description. Never commit `results.tsv`.
7. Go to step 1.

**Probes.** A full run is the only way to keep a change, but a probe is a cheaper way to test a guess first, for example which part of the prompt one model reads. Save the variant under `tuning/runs/` (gitignored; `spent` reads only the `.jsonl` files there), not in `tuning/prompt.md`, First check the budget with `decide.py spent`, as before a full run; exit 13 means stop. A probe sends at most what a full run does, so that check covers it. Then run it with `--out tuning/runs --split train --repetitions 1 --retries 3 --instructions VARIANT` and any of the eligible Candidates, so `spent` counts it. Then `score` it and the run of the current prompt, and compare the lifts per model and target. A probe is never judged, logged or committed. A model with noise 0 is measured exactly by one probe; the others only roughly, so confirm with the full run. One request answers every target, so check all of them: a line meant for one target can move the others. Probes choose on the same train Cases as experiments, so they make the train gain more optimistic: they count toward the stop rule below, and the report says how many you ran.

If `score` refuses (a holdout Case in the run, or a prompt that quotes a Case message), do not look for a way around it. The first means you ran without `--split train`; the second means your prompt copies a Case, so rewrite it in general terms.

## Stop conditions

Stop and report to the user when any of these happens:

- The request budget is used up: `decide.py spent` exited 13.
- A run was unusable three times in a row (providers are failing; do not burn the budget), or the check of a low-coverage model failed.
- Ten attempts in a row (experiments and probes) brought no keep. Say what you tried and ask for direction.
- A `score` or `judge` error you do not understand.
- The user interrupts.

When you stop for a provider problem, leave the experiment's `tune:` commit in place and log no `keep` or `discard` for it: it is **pending**. Otherwise keep going. The user may be away; do not stop to ask whether to continue.

## Resuming

The user may ask you to continue a stopped session later, for example after a free quota has reset. Everything needed is in the worktree: the commits, `tuning/baseline.json`, and the untracked `tuning/best.json`, `tuning/results.tsv` and `tuning/runs/`. Never recalibrate, never start a new branch or worktree, and never delete any of the untracked files.

1. `cd` to the `worktree=` path in the preamble of `results.tsv`. Check that `git status` shows no uncommitted tracked changes and that the branch is the one in the preamble. If not, stop and tell the user.
2. Rebuild the settings from the preamble: the binary, the flags and the budget. The models to run are `eligible_models` in `tuning/baseline.json`.
3. **Unlogged keep.** This sends no requests, so do it before any budget or provider check. If HEAD is a `tune:` commit that has no `keep` or `discard` row in `results.tsv` (a `rerun` row does not count) and the `commit` in `tuning/best.json` is HEAD (`git rev-parse HEAD`), `judge` kept this experiment and the session stopped before the row was logged. Do not run it again. Score its `evidence` file and judge that file again (`--run` that file, `--commit` HEAD): `judge` recognises the kept run and reports the same keep. Log that row.
4. **Budget.** Check it with `decide.py spent`; exit 13 means stop and report.
5. **Gate.** Send one request to each eligible model, as in the smoke test. If any fails, stop and report which, without running anything else. The quota has not come back.
6. **Pending experiment.** If HEAD is still a `tune:` commit with no `keep` or `discard` row, it is pending: it was never judged, or judged a discard and stopped before the reset. Run it again as in the loop (step 4), then score, judge and log it. Do not edit the prompt first.
7. Then continue the loop.

## Report

When you stop, give the user: the worktree path (which must be kept, because it holds `best.json`, `results.tsv` and `runs/`), the branch, whether an experiment is pending, which models failed and with what error, how to resume (tell the agent to continue per the Resuming section), the number of experiments kept and discarded, the baseline and best objective, each eligible model's baseline and best lift, the budget used (from `decide.py spent`), and a short list of the changes that worked. Keep the claims honest: these numbers come from the train Cases the prompt was tuned on, so they overstate what to expect, the more so the more experiments and probes you ran (give both counts). The labels are the maintainer's reviewed judgements, not measurements, so the loop optimises agreement with them.

The holdout step belongs to the user, and you do it only when they ask for it in this session. Tuning ends once holdout evidence exists; do not resume it, and do not inspect holdout evidence (no `show`, no `misses`), only score it: run `--split holdout` once for `tuning/prompt.md` and once for `prompts/neutral.md` with the same Candidates (without `--out tuning/runs`, so the holdout evidence stays out of the tuning folder), then `score --allow-holdout` on each, and report train and holdout side by side per model and target. A tuned prompt is worth promoting to `prompts/` only if it beats `neutral` on the holdout by more than the noise. The holdout is small (see `dataset inspect`), roughly ±6 points at 95% confidence for 59 Cases, so small gains cannot be told apart from chance.
