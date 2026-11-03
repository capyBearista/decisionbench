# Domain documentation

This is a **single-context Rust repository**, not a monorepo.

- [GLOSSARY.md](../../GLOSSARY.md) defines the shared domain terms. Read it before naming new types, interfaces, test cases or tickets. Keep implementation decisions out of the glossary.
- [docs/SPEC.md](../SPEC.md) records the agreed observable behavior, scoring contract, scope and verification requirements.
- [docs/adr/](../adr/) records only trade-offs that are costly to reverse, surprising without context, and have genuine alternatives. Consult relevant ADRs before changing behavior or architecture.
- [docs/agents/issue-tracker.md](issue-tracker.md) explains issue tracking.

During implementation, raise conflicts between code and the spec explicitly; do not silently resolve them by changing requirements.
