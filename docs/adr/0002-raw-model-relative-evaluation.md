# ADR-0002: Grade raw, model-relative effort predictions independently of prior effort

**Status:** Accepted as benchmark methodology (2026-11-03); superseded in part by [ADR-0003](0003-owned-per-target-effort-ranges.md) for dataset provenance, holdouts and label format

## Context

Effortless already knows the active Claude model, but its production policy can retain the current effort on short follow-ups, substitute a previous effort when confidence is low, and fall back to another judge. Its instructions also contain an unvalidated one-step Sonnet/Opus effort equivalence. Measuring those effective decisions as if they were the original Decision Model's independent recommendation would conflate model ability with the mod's policy.

The source fixture has 75 human-labelled cases (20 holdout), originally labelled for Sonnet or Opus rather than both. Four identical prompt/context pairs already have independently specified labels for both.

## Decision

- For each Case, request a **separate raw effort recommendation for Sonnet and Opus**, changing the target model while keeping the task, conversation context and evaluation instructions equivalent.
- **Exclude current/previous reasoning effort from the judging input**. Whether a transport requires an omitted or explicit-null field is a provider-compatibility verification task, not a license to substitute a non-null effort.
- Grade **raw normalized effort predictions** against a **separate human-reviewed acceptable-effort range for each Target Model**.
- Missing counterpart labels remain **unscored** until independently reviewed. Do not generate them by subtracting one effort level.
- Preserve Raw Responses, original confidence/probabilities, parse failures and each physical Attempt. Retries, transport failures and missing labels are distinct from incorrect valid choices.
- Any **effective decision** from an uncertainty threshold, fallback, keep-current rule, bias or bounds is explicitly downstream **policy behavior**, not part of the initial raw-model-quality grade.
- Report Sonnet and Opus results separately, with their sample sizes, to avoid hiding the initial 67:8 label imbalance behind one accuracy number.

## Alternatives considered

**Supply current effort to the judge:** rejected for primary quality evaluation because it can anchor recommendations to irrelevant prior settings. The mod itself may still use that state for session continuity.

**Grade the final effortless policy output:** rejected for the initial model-quality comparison; a future policy benchmark can deliberately test end-to-end production behavior under named policy settings.

**Auto-derive Opus labels from Sonnet labels:** rejected because model capability differences vary by task, and the one-step rule is precisely the unproven heuristic under investigation.

**Send a complete batch in one provider request:** rejected because it is not faithful to individual production Decisions API calls and obscures per-request latency/failure behavior.

## Consequences

Results distinguish decision-model quality from operational reliability and production-policy behavior. The initial coverage of paired, scored Sonnet/Opus cases will be sparse until additional independent labels are reviewed. Raw records must be sufficiently rich to regrade with later dataset revisions without rewriting old evidence.

## Related work

[effortless Issue #4](https://github.com/capyBearista/effortless/issues/4) tracks changes to the **production mod**; this ADR governs the independent **benchmark**.
