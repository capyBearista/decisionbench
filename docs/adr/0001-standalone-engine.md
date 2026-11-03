# ADR-0001: Standalone Rust engine with CLI and terminal UI

**Status:** Accepted as product direction (2026-11-03)

## Context

The existing benchmark runs inside effortless and cannot independently compare arbitrary Decisions-model candidates or export reusable, reproducible evidence. We want a standalone tool now and eventual inclusion in Valbench later. Valbench is Python with Harbor/Docker-backed agentic benchmark tracks and an external-evidence import precedent, but judging effort does not require any such containerized task execution.

Possible approaches included extending effortless's embedded benchmark, building this inside Valbench, starting a generic HTTP service, or creating an independent command-line application.

## Decision

- Build **decisionbench** as an independent **Rust** application, initially a **single Cargo package** with a reusable library and binary. Use the newest stable Rust compiler and **2024 edition**.
- Supply a **headless CLI** and **Ratatui** TUI over the same benchmark engine. A terminal-first interface is deliberate; no web application is intended.
- Make versioned **machine-readable local evidence and CLI behavior** the integration contract. Use local **JSON/JSONL** for runs/evidence and **TOML** for configuration, with no mandatory database.
- Keep provider-specific HTTP mappings behind small, testable provider adapters. Start with OpenCode Zen and OpenRouter Decisions API; add other providers only after their contracts are verified.
- Use the typography, semantic statuses, restrained charcoal/orange accents and interaction principles of homelab-design in a terminal-appropriate form; do not copy its browser stack.
- Defer HTTP service, Docker packaging and Valbench integration to separately scoped work ([Issue #1](https://github.com/capyBearista/decisionbench/issues/1)).

## Alternatives considered

**Keep benchmarking inside effortless:** rejected because harness coupling and fallback behavior obscure raw Decisions-model quality.

**Build directly in Valbench/Python:** deferred because the user wants an independently useful, lightweight terminal tool now; future consumption can use the export/CLI interface.

**Start with an HTTP API/server:** rejected for the initial release because no initial consumer needs a running service; a documented CLI and stable evidence schema provide a useful programmatic interface first.

**Use another Rust TUI framework:** Ratatui chosen after considering iocraft and other options. Reversing the UI library should not require changing the benchmark engine.

**Start with a multi-crate workspace or plugin loader:** rejected as speculative complexity before a real requirement for independent packages or dynamically installed providers.

## Consequences

The core evaluation logic remains independent of the UI and agent harnesses. Initial implementation and packaging stay small. A future Valbench adapter can consume evidence without duplicating provider logic, but schema versioning and provenance become important from the first release. The Rust library and CLI form a public behavioral contract even though no HTTP service exists.

## Revisit when

A concrete third-party consumer needs a long-lived network interface, Valbench integration demands additional packaging, or multiple independently maintained crates justify a workspace.
