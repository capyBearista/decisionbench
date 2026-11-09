# ADR-0007: A train/holdout split, a tuning score, and retry backoff

**Status:** Accepted (2026-11-08). Amends [ADR-0003](0003-owned-per-target-effort-ranges.md) (no holdout flag), and SPEC §3 (no holdout markers), §5 (fixed retry pause) and §6 (no composite number).

## Context

We now want to tune Judge Instructions against the dataset with an automated loop. ADR-0003 dropped the holdout flag because "no such tuning loop exists" and said a held-out set "must be reintroduced deliberately" if one did. Three things get in the way of a loop that is honest:

- Tuning on the Cases that are scored overfits. `prompts/effortless.md` already quotes Cases (c021, c026), so its scores are in-sample.
- A raw hit rate is a weak target. Label ranges are wide (1.8 levels on average), so always answering `low` hits 53% pooled and 71% on Fable.
- A loop sends many requests to free-tier routes, whose 429 replies carry `Retry-After`; a fixed 1 s pause makes them fail more often than they need to.

## Decision

- Every Case has a required `split`: `train` or `holdout`. The 66 existing Cases were assigned with a seeded shuffle (seed 20261108) inside strata of highest labelled effort × has-context, 30% of each stratum to holdout; c064 follows its near-duplicate c010. Result: 45 train, 21 holdout. The 131 later Cases (c067–c197) were assigned the same way, giving 138 train and 59 holdout in total. The audit also revised 12 labels on 11 of the original Cases (c012, c014, c015, c024, c026, c028, c030, c032, c036, c052, c063); their messages, contexts and splits are unchanged. A split never changes once assigned.
- `run --split train|holdout` runs only that part. It combines with `--case`.
- `score FILE` prints JSON, per Candidate × Judge Instructions, per Target Model and pooled: hit, under, over, invalid, failed, coverage, hit rate, **floor** and **lift**.
  - Rows are labelled predictions of answered Trials. Invalid counts as a miss; a Trial with no valid reply is `failed` and outside the rows, lowering coverage.
  - The floor is the hit rate of the best single fixed effort on the same rows, chosen separately per target. Pooled, it is the sum of those hits over all rows. Lift is hit rate minus floor.
  - It refuses a Run whose dataset revision differs from the file given, a Run that includes a holdout Case unless `--allow-holdout` is given, and a Run whose Judge Instructions contain the message of any Case (20 or more characters, whitespace and case ignored).
  - `show` and `compare` are unchanged. The score is for tuning, not a ranking; averaging models and keeping or discarding a change is the loop's job, not the binary's.
- Retries pause 1 s, doubling to 30 s. A `Retry-After` in seconds, capped at 60 s, can lengthen the pause. The Attempt that received it saves it as `retry_after_secs` (omitted when absent), an additive field: evidence schema stays 4.

## Alternatives considered

**A separate holdout file:** rejected. It makes two dataset revisions to keep in step, and a Run's manifest already names one.

**Recording the split in the Run's settings:** rejected. It would change the manifest and force a new schema version. `score` looks the split up in the dataset instead, after checking its hash.

**A composite score in `show`:** rejected, as in SPEC §6. The number exists only in `score`.

## Consequences

- Every dataset file, including test fixtures, needs a `split`.
- The holdout protects nothing against an agent that opens the dataset file. The tuning instructions must forbid that; the `score` refusals only catch a Run, not a read.
- The floor is computed over the rows that got a valid reply, so two runs with the same coverage have the same floor and maximising lift is the same as maximising hit rate. A run that lost more Trials to provider failures has a different row set; compare lift only between runs of similar coverage (`score` also prints the run state and Trial counts, because a cancelled run reports full coverage over fewer rows). Lift is what makes a model's skill visible: a model at the floor is not choosing efforts better than a constant.
- Dropping `Retry-After` handling for HTTP dates keeps the parser trivial; a provider that sends dates gets plain backoff.
