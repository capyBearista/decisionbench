# decisionbench domain glossary

Canonical terminology for specs, data files, interfaces, tests and implementation issues. This file defines **what** a term means, not how it must be implemented.

## Language

**Decision Model**  
The specialized model being evaluated for its ability to select reasoning effort. It is not the assistant doing the downstream work. Avoid: target model, judge (unless referring to legacy effortless code).

**Target Model**  
The Claude model family whose reasoning effort is being recommended for a particular task. Initial target models are **Sonnet** and **Opus**. Avoid: decision model.

**Provider**  
The upstream organization/service exposing a Decisions API, such as OpenCode Zen or OpenRouter.

**Provider Route**  
The specific provider-facing endpoint and model identifier used to request a Decision Model. Distinguishes differently hosted offerings of an underlying model.

**Candidate**  
The benchmarkable pairing of a Decision Model and a Provider Route, including the identity/configuration necessary to reproduce requests. Identically named models on different Provider Routes are distinct Candidates.

**Case**  
A task message with any relevant conversation context, identified by a stable ID that is never changed or reused. One Case can have separately reviewed labels for one or more Target Models.

**Context**  
Conversation material relevant to interpreting a Case, such as the preceding assistant question. Context is distinct from the Target Model and from historical session effort.

**Dataset**  
A versioned collection of Cases and their ground-truth labels.

**Dataset Revision**  
An immutable snapshot of dataset content and label decisions used by a Run. Changes to labels or Cases produce a new revision.

**Label / Acceptable-Effort Range**  
A human-reviewed contiguous range of effort levels considered acceptable for one Case and one Target Model, written as one level (`max`) or `from-to` (`low-high`). Every effort in the range is equally acceptable.

**Unscored**  
A Case–Target Model combination with no reviewed label. Predictions may still be recorded, but they cannot contribute to accuracy.

**Raw Response**  
The original observable reply from the provider, subject to secrets-safe retention policy, before normalization or interpretation.

**Raw Prediction**  
The effort choice made by a Decision Model, extracted from its Raw Response, with original confidence information if available. Invalid or absent effort is not a prediction.

**Normalized Prediction**  
A Raw Prediction mapped to a canonical effort value for grading, without applying fallback, bias, floor/ceiling or previous-effort retention.

**Effective Decision**  
The action taken after an explicitly specified production-policy interpretation of a prediction (for example, an uncertainty fallback). It is separate from Raw Prediction and is not the initial benchmark's scoring target.

**Run**  
One configured benchmark execution, identifying the selected Candidates, Dataset Revision, evaluation instructions, targets, repetitions and execution settings. Contains Trials and associated evidence.

**Trial**  
One logical evaluation of one Candidate × Case × Target Model × repetition. A Trial can require multiple Attempts when a retry policy is enabled.

**Attempt**  
One actual outbound request to a Provider Route, including timing and success/error disposition. Attempts remain visible even when a later attempt succeeds.

**Grade**  
A derived classification of a valid Normalized Prediction against the Label for the same Case and Target Model: hit (inside the range), under (below it) or over (above it). An invalid response, provider failure and Unscored Case are distinct outcomes.

**Evidence**  
Immutable records of the Run configuration, Attempts, Raw Responses and provenance used to compute Grades and aggregate results.

**Pricing Snapshot**  
Manually entered per-Provider-Route rates captured with a Run so estimated costs can be reproduced without fetching current prices.

## Relationships

- A **Candidate** identifies one **Decision Model** accessed through one **Provider Route**.
- A **Dataset Revision** contains many **Cases**. A **Case** can have separate **Labels** for Sonnet and Opus.
- A **Run** selects a **Dataset Revision** and one or more **Candidates**; each **Trial** evaluates one Case–Target Model–Candidate combination in one repetition.
- A **Trial** contains one or more **Attempts**; a **Grade** derives from the valid **Normalized Prediction** and the matching **Label**.
- A **Run** preserves **Evidence**; summaries and estimated costs are derived interpretations of that evidence.

## Resolved ambiguities

- **Model** alone is ambiguous. Say **Decision Model** or **Target Model**.
- **Provider** is not synonymous with **Decision Model**. Provider Route identity matters for comparisons, latency, reliability and price.
- **Current effort** and **current model** are session state. The dataset does not record them, and current effort is never a judging input; the Target Model is sent with each request instead.
- **Unsure** is a model confidence/response observation; a decision to retain previous effort is a separate policy behavior owned by effortless.
