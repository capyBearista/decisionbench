# ADR-0006: Prompts are a comparison axis

**Status:** Accepted (2026-11-05). Extends [ADR-0004](0004-judge-instructions-axis.md), whose side-by-side comparison of instruction sets applied only to sets inside one Run. It also replaces three statements in the SPEC: that `compare` treats different Judge Instructions as making Runs not directly comparable, that no overall micro-average is reported, and that the TUI Results screen uses the same report as the CLI (this now holds for its Full report panel only).

## Context

Judge Instructions could already be varied inside one Run (`--instructions` is repeatable), but the rest of the tooling treated a different prompt as a reason not to compare. `compare` listed instructions content among the things that make Runs not directly comparable, so Runs that differed only in prompt could not be lined up. The TUI Setup screen took a single instructions file, and the Results screen showed the full report, about 100 lines of text, as its only view.

The user wants to try several prompts in one Run and across Runs, and to see at a glance which prompt gains or loses cases against another.

## Decision

- **Variants are columns.** A variant is one prompt (a Judge Instructions set) in one Run. `compare FILE…` prints one table: a row per Candidate × Target Model, a column per variant. The first column is the baseline. A column is labelled with the prompt name; when two labels collide, ` (<12-character SHA-256>)` is added if their contents differ, and ` in <run id>` if the same content appears in two Runs.
- **Prompts no longer make Runs "not comparable".** The `Not directly comparable:` list still covers Run state, dataset, grading revision, schema version, targets, Case selection, repetitions, unscored inclusion, Candidates missing from either Run and Provider Route differences. Timeout, concurrency and retries are listed apart as `settings differ`.
- **Paired delta.** Each non-baseline cell shows its hit count and `+g -l`: the paired Trials (Case × repetition) where the baseline missed and this variant hit (gained), and where the baseline hit and this variant missed (lost). A pair counts only when both sides have a valid scored prediction. No delta is shown when nothing pairs.
- **Noise rule.** A delta is marked `~` when the net change is at most one Case's worth of repetitions (the larger `repetitions` of the two Runs), or when gained equals lost.
- **Pooled row.** Under the table, a `pooled | all` row adds the valid scored predictions of the rows present in every column (every Candidate and Target Model that all variants cover, so each variant is pooled over the same rows) with the summed delta, followed by a note that it is not a score. With no such row there is no pooled row.
- **`show` gains a `== Summary ==` section** after `== Run ==`: the same matrix for one Run, one column per prompt. Its pooled row appears only when the Run has more than one row.
- **TUI Results** opens on a Summary panel (the matrix; gained in the OK colour, lost in the fail colour, dim when within noise; the numbers stay in the text). `[` and `]` switch to the Full report panel, which is the `show` or `compare` text. Marked Runs keep their mark order, shown as `[1]`, `[2]`…; the first marked is the baseline.
- **TUI Setup** lists prompts: the bundled `neutral` (chosen by default), files added by path, and the `*.md` files of the **prompt folder**. The folder is the optional top-level config key `prompts_dir`, which the TUI writes when it is set. A dim `plan:` line shows candidates × prompts × Cases × repetitions = Trials, up to N requests.

## Consequences

- The SPEC's "no overall micro-average" rule now applies to the per-Candidate and per-Target reports, which stay separate and never form a composite. The pooled row exists only in the comparison matrix, is labelled, and is never a grade.
- Pooled numbers mix Target Models of different difficulty, so a pooled hit rate summarizes the table and is not a measure of the prompt.
- The noise rule is a pragmatic heuristic, not a significance test. It assumes the compared Runs used the same Cases and repetitions; `compare` lists it when they did not. Trials are counted, not Cases, so one Case can contribute up to its repetitions to a delta.
- A cell is `-` where a Run did not ask that Candidate, Target Model or prompt. A row whose baseline cell is `-` shows no deltas.
- More prompts multiply the requests in a Run: one more factor in the Trial count, as ADR-0004 already noted. The Setup `plan:` line shows the total before Start.
- The CLI does not read `prompts_dir`; it takes `--instructions` as before. A config with no `[[candidate]]` tables now loads as an empty list.
