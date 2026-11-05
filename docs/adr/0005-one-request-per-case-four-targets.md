# ADR-0005: One request per Case, four Target Models

**Status:** Accepted (2026-11-04). Supersedes the "separate request per target" part of [ADR-0002](0002-raw-model-relative-evaluation.md) and the two-target label set of [ADR-0003](0003-owned-per-target-effort-ranges.md). Amends the file format of [ADR-0004](0004-judge-instructions-axis.md).

## Context

Issue #17 asks for acceptable efforts per model and for the request shape effortless uses: one request that asks one question per model. Until now decisionbench sent one request per Case and Target Model, with a single `effort` question and the Target Model named in `state`, and it knew only Sonnet and Opus.

Effortless asks only Haiku, Sonnet and Opus today. Fable is asked here on the assumption that effortless will add it.

## Decision

- The **Target Models** are **Haiku 5.5** (`haiku`), **Sonnet 5.5** (`sonnet`), **Opus 5.5** (`opus`) and **Fable 5.1** (`fable`).
- A Trial is one **Candidate × Judge Instructions × Case × repetition**. It sends one Decisions request that asks one `choice` question per selected target, named `effort_<target>`. Each question has its own instructions and criteria.
- `state` is `{task, recent_conversation?, next_message}`. It has no `target_model`, and still no current effort or current model (ADR-0002).
- A **Prediction** is a Trial × Target Model. Each Prediction is graded against that target's label.
- A Judge Instructions file is an intro (sent as `state.task`), then one `## <target>` section per model in the order `haiku`, `sonnet`, `opus`, `fable`. Each section has an instructions paragraph and exactly five `- <effort>: …` lines in scale order. A file may leave a model out; asking about it is then a usage error. The name and SHA-256 identify a set as before.
- The bundled `neutral` set uses the same neutral criteria for every model. `prompts/effortless.md` mirrors effortless's text, with a **drafted Fable section** that has no measurement behind it.
- Label order (#17, item 4) is already settled by ADR-0003. A label is a range and every effort in it is equally acceptable, so the first effort listed is not the cheapest preferred one.
- Two **cross-model metrics** are reported next to the per-target ones and never replace them:
  - **Joint hit:** among answered Trials with at least two labelled targets that all got a valid effort, the share where every labelled target hit.
  - **Gap agreement:** for each pair of labelled targets in a Trial whose ranges do not overlap, so that one is expected to be strictly below the other, the share of pairs whose predicted efforts keep that order. A tie counts as disagreement.

## Alternatives considered

**Keep one request per target and add Haiku and Fable:** rejected. It does not match the request effortless makes, so results would not transfer to it.

**Ask only the three models effortless asks:** rejected. The user wants Fable measured now, so effortless can adopt it with evidence.

**Derive the new labels from existing ones:** rejected, as in ADR-0002. Drafts are written case by case and only count once approved.

## Consequences

- One request covers every selected target, so the answers for one Trial are not independent of each other. Repetitions remain independent.
- Evidence schema is now **4** and the grading revision is **2**. Older evidence (schemas 2 and 3) is not readable; there is no compatibility layer.
- Labels for all four targets were drafted, reviewed and approved by the user, then merged into `datasets/cases.json` (a new Dataset Revision). The dataset now has 66 English Cases, each labelled for Haiku, Sonnet, Opus and Fable. The five Swedish Cases (c006, c014, c028, c034, c040) were removed, and their IDs were refilled by moving the last five Cases (c071, c070, c069, c068, c067) into the gaps, so the IDs run c001 to c066.
- `--target` chooses which questions are asked. A Case is planned when at least one selected target has a label, unless `--include-unscored` is set.
- Fable's criteria in `effortless.md` are a draft. Results from it are a measurement of that draft, not of effortless.
