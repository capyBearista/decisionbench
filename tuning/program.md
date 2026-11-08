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

1. **Branch.** Agree a tag with the user (today's date, for example `nov8`). `git switch -c tune/<tag>` from the branch the user names. The branch must not exist yet.
2. **Binary.** `cargo build --release --locked` once. Use `BIN=target/release/decisionbench` from now on. Do not rebuild: the dataset is compiled in, and a rebuild after a dataset change would change the scored Cases.
3. **Candidates: the gate.** Ask the user for the Candidate names, one for each *distinct Decision Model*. Read the config to check them. Two routes to the same `decision_model` count once; keep one. Only Decisions endpoints are supported. Then smoke-test each: `$BIN run --candidate NAME --case c002 --target opus --repetitions 1`. Drop any Candidate that does not exit 0. If fewer than 5 distinct models pass, **stop and tell the user**. They decide whether to continue with fewer. Fewer than 2 means no loop.
4. **Budget.** Ask the user for the most requests (or dollars) you may spend, and write it down. One experiment sends about *models × train Cases × repetitions* requests (the count `run` prints as "up to N requests" is the ceiling, with retries). Track the running total in `tuning/results.tsv`.
5. **Settings.** Use `--split train --repetitions 1 --concurrency 4 --retries 3` for every run below, so that runs stay comparable. Free tiers rate-limit; backoff is built in, and low coverage is handled by the rules below. If calibration shows a mean noise of 3 points or more (step 6), repeat the calibration once with `--repetitions 3` and use 3 from then on. Note the choice in the first line of `results.tsv`.
6. **Calibrate.** Run the unchanged prompt twice with every passing Candidate, and score both. `run` prints `evidence: PATH` on its last line.

   ```sh
   $BIN run --split train --instructions tuning/prompt.md --repetitions 1 --concurrency 4 --retries 3 \
        --candidate A --candidate B --candidate C --candidate D --candidate E
   $BIN score PATH_1 > /tmp/score_a.json
   # run the same command again, then:
   $BIN score PATH_2 > /tmp/score_b.json
   python3 tuning/decide.py baseline /tmp/score_a.json /tmp/score_b.json
   ```

   `decide.py baseline` writes `tuning/baseline.json` and prints, per model, its neutral lift, its noise (the difference between the two runs) and whether it is **eligible**. Eligible means the lift beats the best fixed answer by more than twice the noise and at least 2 points, with coverage of 90% or better. It also prints `delta`, the smallest objective gain that counts: 1 point or twice the mean noise. It exits 12 when fewer than 2 models are eligible. In that case **stop and report**: there is no signal to tune against. Exit code 3 from `run` only means some Trials had no reply; keep going, and `score` will show the coverage.
7. **Freeze.** Show the user `tuning/baseline.json`: models, eligibility, noise, delta. If they want to change an eligibility call, they edit the file. Then `git add tuning/baseline.json && git commit -m "tuning: baseline"`. Create `tuning/results.tsv` (untracked) with the header `commit	objective	gain	verdict	requests	description`. Log the baseline as the first row (verdict `baseline`).

From here on, only the **eligible** models are run (`--candidate` for each of them, nothing else). The objective is the mean of their pooled lift. You never compute it yourself; `decide.py` does.

## The loop

Repeat until you reach a stop condition.

1. **Look.** Find what the current prompt gets wrong on the train set: `python3 tuning/decide.py misses PATH_OF_THE_LAST_RUN --limit 40`. It lists the missed Trials with counts of under- and over-predictions per target. Use it to form one hypothesis, such as "the models pick `low` for multi-step edits" or "xhigh is chosen for ordinary refactors". Under-predicting is the common failure, but check the counts.
2. **Edit** `tuning/prompt.md` to test that one hypothesis. One idea per experiment, so that a keep or discard tells you something.
3. **Commit** only the prompt: `git add tuning/prompt.md && git commit -m "tune: <what and why in one line>"`.
4. **Run** the same command as in calibration with the eligible Candidates. Redirect output to a file; do not print the whole progress stream into your context. Take the `evidence:` path from the end of it.
   If `run` exits 1, the prompt or the config is broken (for example a section without exactly five effort lines). Nothing was scored: fix the prompt, `git commit --amend`, and run again. Exit 3 is fine, see above. Exit 130 means the run was interrupted; run it again.
5. **Score and judge.**

   ```sh
   $BIN score PATH > /tmp/score.json
   python3 tuning/decide.py judge /tmp/score.json
   ```

   `judge` compares the run with the best state so far (the baseline at first) and exits:
   - `0` **keep**: the objective rose by at least delta, and no model fell by more than twice its noise (at least 1 point). Leave the commit in place. `judge` has recorded the new best in `tuning/best.json`.
   - `10` **discard**: `git reset --hard HEAD~1`.
   - `11` **rerun**: the run is not usable (it is partial, a Trial is missing, or a model's coverage is under 90%). Do not score it and do not count it as an experiment. Run the same commit again. If it comes back unusable three times in a row, stop (see below).
   - `1`: bad input (a different dataset or other selection than the baseline, or a missing model). Stop and report.
6. **Log** one row to `tuning/results.tsv`: short commit hash, objective, gain, verdict (`keep`, `discard`, `rerun`), the requests used, and the description. Never commit `results.tsv`.
7. Go to step 1.

If `score` refuses (a holdout Case in the run, or a prompt that quotes a Case message), do not look for a way around it. The first means you ran without `--split train`; the second means your prompt copies a Case, so rewrite it in general terms.

## Stop conditions

Stop and report to the user when any of these happens:

- The request budget is used up.
- A run was unusable three times in a row (providers are failing; do not burn the budget).
- Ten experiments in a row were discarded. Say what you tried and ask for direction.
- A `score` or `judge` error you do not understand.
- The user interrupts.

Otherwise keep going. The user may be away; do not stop to ask whether to continue.

## Report

When you stop, give the user: the branch, the number of experiments kept and discarded, the baseline and best objective, each eligible model's baseline and best lift, the budget used, and a short list of the changes that worked. Keep the claims honest: these numbers come from the train Cases the prompt was tuned on, so they overstate what to expect. The labels are the maintainer's reviewed judgements, not measurements, so the loop optimises agreement with them.

The holdout step belongs to the user, and you do it only when they ask for it in this session: run `--split holdout` once for `tuning/prompt.md` and once for `prompts/neutral.md` with the same Candidates, then `score --allow-holdout` on each, and report train and holdout side by side per model and target. A tuned prompt is worth promoting to `prompts/` only if it beats `neutral` on the holdout by more than the noise. The 59 holdout Cases give roughly ±6 points at 95% confidence, so small gains cannot be told apart from chance.
