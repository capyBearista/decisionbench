# decisionbench domain glossary

Canonical terminology for specs, data files, interfaces, tests and implementation issues. This file defines **what** a term means, not how it must be implemented.

## Language

**Decision Model**  
The specialized model being evaluated for its ability to select reasoning effort. It is not the assistant doing the downstream work. Avoid: target model, judge (unless referring to legacy effortless code).

**Target Model**  
The Claude model family whose reasoning effort is being recommended for a particular task. The Target Models are **Haiku 5.5**, **Sonnet 5.5**, **Opus 5.5** and **Fable 5.1** ([ADR-0005](docs/adr/0005-one-request-per-case-four-targets.md)). Avoid: decision model.

**Provider**  
The upstream organization/service exposing a Decisions API, such as OpenCode Zen or OpenRouter.

**Provider Route**  
The specific provider-facing endpoint and model identifier used to request a Decision Model. Distinguishes differently hosted offerings of an underlying model.

**Candidate**  
The benchmarkable pairing of a Decision Model and a Provider Route, including the identity/configuration necessary to reproduce requests. Identically named models on different Provider Routes are distinct Candidates.

**Judge Instructions**  
A named instruction text, with a description for each effort level, sent with every Decisions request. Each set is a Markdown file identified by its name and the hash of its content, so an edited file is a new revision. Judge Instructions are selected, not part of the Candidate, so the same Candidate can be compared across instruction sets. Avoid: prompt (alone), task.

**Case**  
A task message with any relevant conversation context, identified by a stable ID that does not change once the dataset is released. One Case can have separately reviewed labels for one or more Target Models.

**Context**  
Conversation material relevant to interpreting a Case, such as the preceding assistant question. Context is distinct from the Target Model and from historical session effort.

**Dataset**  
A versioned collection of Cases and their ground-truth labels.

**Dataset Revision**  
An immutable snapshot of dataset content and label decisions used by a Run. Changes to labels or Cases produce a new revision.

**Split**  
The part of the dataset a Case belongs to: `train` (used while tuning Judge Instructions) or `holdout` (scored once, at the end, to check for overfitting). Fixed when a Case is added and never changed ([ADR-0007](docs/adr/0007-split-and-tuning-objective.md)).

**Floor**  
The hit rate of the best single fixed effort on the same labelled predictions, chosen separately for each Target Model. A Decision Model at the floor is not choosing efforts better than a constant. Pooled, it is the sum of the per-target best hits over all rows.

**Lift**  
Hit rate minus Floor. Printed only by `score`, for tuning; it is not a ranking of Candidates.

**Label / Acceptable-Effort Range**  
A human-reviewed contiguous range of effort levels considered acceptable for one Case and one Target Model, written as one level (`max`) or `from-to` (`low-high`). Every effort in the range is equally acceptable.

**Unscored**  
A Case–Target Model combination with no reviewed label. Predictions may still be recorded, but they cannot contribute to accuracy. A response for an unlabelled target is unscored even if it is invalid.

**Raw Response**  
The original observable reply from the provider, subject to secrets-safe retention policy, before normalization or interpretation.

**Raw Prediction**  
The effort choice made by a Decision Model, extracted from its Raw Response, with original confidence information if available. Invalid or absent effort is not a prediction.

**Normalized Prediction**  
A Raw Prediction mapped to a canonical effort value for grading, without applying fallback, bias, floor/ceiling or previous-effort retention.

**Effective Decision**  
The action taken after an explicitly specified production-policy interpretation of a prediction (for example, an uncertainty fallback). It is separate from Raw Prediction and is not the initial benchmark's scoring target.

**Run**  
One configured benchmark execution, identifying the selected Candidates, Dataset Revision, Judge Instructions, targets, repetitions and execution settings. Contains Trials and associated evidence. A Run ends **complete** (every planned Trial recorded), **partial** (cancelled or interrupted) or **failed** (stopped by an internal error).

**Trial**  
One logical evaluation of one Candidate × Judge Instructions × Case × repetition. Its single request asks one question per selected Target Model, so the answers for different targets in one Trial are not independent. A Trial can require multiple Attempts when a retry policy is enabled.

**Prediction**  
One Trial's answer for one Target Model (Trial × Target). Grades, accuracy and consistency are counted per Prediction.

**Attempt**  
One actual outbound request to a Provider Route, including timing and success/error disposition. Attempts remain visible even when a later attempt succeeds.

**Grade**  
A derived classification of a valid Normalized Prediction against the Label for the same Case and Target Model: hit (inside the range), under (below it) or over (above it). An invalid response, provider failure and Unscored Case are distinct outcomes.

**Joint Hit**  
Among answered Trials where at least two selected Target Models are labelled and each of them got a valid effort, a Trial whose labelled targets are all hits.

**Gap Agreement**  
For a pair of selected Target Models whose label ranges do not overlap (so one is expected to need more effort), the two valid predictions keep that strict order. Counted over eligible pairs.

**Variant**  
One column of a comparison: a Judge Instructions set as run in one Run. A Run with several prompts gives several Variants; the same prompt in two Runs gives two. Variants are labelled by prompt name, with a content hash or Run id added only when names collide.

**Baseline**  
The first Variant of a comparison. Every other Variant is measured against it with a paired delta (`+g -l`): the paired Trials (Case × repetition) the Variant gained and lost relative to the Baseline. A delta is marked `~` (not significant) unless an exact two-sided sign test on the Cases that changed gives p < 0.05.

**Pooled Row**  
The row of a comparison matrix that sums valid scored predictions, for each Variant, over the rows that are present in every column (so all columns cover the same rows: the same Candidates and Targets on the prompt axis, the same prompts and Targets on the candidate axis). It exists only to compare Variants at a glance, is always labelled, and is not a score.

**Consistency**  
For one Candidate × Judge Instructions × Case × Target Model with at least two valid efforts across repetitions: all of them are the same effort.

**Evidence**  
Immutable records of the Run configuration, Attempts, Raw Responses and provenance used to compute Grades and aggregate results.

**Pricing Snapshot**  
Manually entered rates for each Candidate's Provider Route, captured with a Run so estimated costs can be reproduced without fetching current prices.

**Credentials File**  
The per-user `credentials.toml` beside the config file that holds one API key per Provider (`openrouter`, `zen`). An environment variable for the same Provider takes precedence. It is private to the user (mode 0600), never part of the config, and never written to Evidence.

## Relationships

- A **Candidate** identifies one **Decision Model** accessed through one **Provider Route**.
- A **Dataset Revision** contains many **Cases**. A **Case** can have a separate **Label** for each Target Model.
- A **Run** selects a **Dataset Revision** and one or more **Candidates**; each **Trial** evaluates one Case–Candidate–Judge Instructions combination in one repetition and yields one **Prediction** per selected Target Model.
- A **Trial** contains one or more **Attempts**; a **Grade** derives from the valid **Normalized Prediction** and the matching **Label**.
- A **Run** preserves **Evidence**; summaries and estimated costs are derived interpretations of that evidence.

## Resolved ambiguities

- **Model** alone is ambiguous. Say **Decision Model** or **Target Model**.
- **Provider** is not synonymous with **Decision Model**. Provider Route identity matters for comparisons, latency, reliability and price.
- **Current effort** and **current model** are session state. The dataset does not record them, and current effort is never a judging input; each Target Model is asked as its own question (`effort_<target>`) instead.
- **Unsure** is a model confidence/response observation; a decision to retain previous effort is a separate policy behavior owned by effortless.
