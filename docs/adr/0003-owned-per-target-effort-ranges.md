# ADR-0003: One owned dataset with per-target effort ranges

**Status:** Accepted (2026-11-03). Supersedes the dataset-provenance, holdout and label-format parts of [ADR-0002](0002-raw-model-relative-evaluation.md).

## Context

The first dataset plan kept the effortless fixture as an untouched source, with each record labelled for whichever model the session was using (`current.model`) and carrying the session's `current.effort`, a category (`kind`) and a holdout flag. A separate derived file would then pair Sonnet and Opus labels. In practice that meant two formats, provenance bookkeeping nobody needed, and labels tied to session state the benchmark deliberately ignores.

## Decision

- decisionbench owns **one dataset file**, converted once from the effortless cases. The upstream file is not kept, tracked or referenced by Case; only the MIT copyright notice and license travel with the data.
- Each Case has a plain stable ID (`c001`…), a message, optional context, and an optional **acceptable-effort range per Target Model** (`"max"`, `"low-high"`). Ranges are contiguous; any effort inside is a hit, below is under, above is over.
- Cases record **no session state**, no category and no holdout flag. Identical message/context records were merged into one Case.
- The effort scale is `low` < `medium` < `high` < `xhigh` < `max`, accepted by every Target Model.

## Alternatives considered

**Keep the upstream fixture plus a derived paired file:** rejected as two formats and parsers for no current consumer of the provenance.

**Keep holdout and category fields:** rejected. Category was only a reporting label. Holdout only protects against tuning instructions on the same Cases you score; no such tuning loop exists. If one is introduced, a held-out set must be reintroduced deliberately.

**Sets with gaps instead of ranges:** rejected. Every existing label was already contiguous, and ranges remove the ambiguous "in-between miss" grade.

## Consequences

Coverage is sparse: of 142 Case–Target slots, 75 are labelled (67 Sonnet, 8 Opus) and the rest are unscored until reviewed by hand. Results can no longer report a holdout estimate. Old Case IDs from effortless are gone, so results cannot be traced to individual effortless records.
