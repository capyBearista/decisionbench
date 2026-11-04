# HANDOFF: decisionbench, #5 to #7 done; #8 or #10 next

## Objective

- **Original request (this session):**
  - Implement [#5](https://github.com/capyBearista/decisionbench/issues/5) (offline grading).
  - Then plan and implement [#6](https://github.com/capyBearista/decisionbench/issues/6) and [#7](https://github.com/capyBearista/decisionbench/issues/7) (one Decisions request through OpenRouter or OpenCode Zen, with saved raw evidence) in **one PR**.
  - Follow up with review-gap fixes.
  - All of this is part of spec [#2](https://github.com/capyBearista/decisionbench/issues/2).
  - The work used TDD and independent review, flagged contradictions rather than silently changing the spec, and started no dependent tickets.
- **Next-session goal:** not chosen yet. [#8](https://github.com/capyBearista/decisionbench/issues/8) (batches, retries, cancellation) and [#10](https://github.com/capyBearista/decisionbench/issues/10) (manual prices and costs) are both unblocked. Ask the user which one to do. Separately, re-run the Zen smoke check once its quota resets.

## Current state

- **Status:**
  - #5, #6 and #7 are **completed and closed**. The #2 checklist is ticked for #4 to #7.
  - #8 to #12 are open and not started.
- **Repo / branch / PR:**
  - `capyBearista/decisionbench`. Local `main` matches `origin/main` at `a44e7cf`.
  - PR #14 (#5) and PR #15 (#6/#7) were merged with merge commits.
  - The follow-up branch `fix/review-gaps-paths-url` was merged locally with `--no-ff` and pushed **directly to `main`**, at the user's explicit request.
  - Remote branches `feat/5-offline-grading`, `feat/6-7-decisions-requests` and `docs/initial-specification` still exist. They were kept by default and are safe to delete if the user asks.
- **Working tree / artifacts:** clean, apart from this file, which is uncommitted.
- **Verification:**
  - On `a44e7cf`:
    - `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo test --locked` (10 unit + 59 CLI tests) and `cargo build --locked` all pass.
    - GitHub Actions `check` passed ([run 41770399265](https://github.com/capyBearista/decisionbench/actions/runs/41770399265)).
  - **Live OpenRouter runs succeeded**, all hits on c001 and c002, for Sonnet and Opus.
  - **No successful Zen reply has ever been observed** (see the pitfalls section).

## Decisions and constraints

The settled decisions live in [docs/SPEC.md](SPEC.md) (especially the new §4a), [GLOSSARY.md](../GLOSSARY.md) and ADRs [0002](adr/0002-raw-model-relative-evaluation.md), [0003](adr/0003-owned-per-target-effort-ranges.md) and [0004](adr/0004-judge-instructions-axis.md). These were decided in this session:

- **Grading (#5):**
  - An unlabelled target is **unscored first**, whatever the response. "Invalid" applies only to labelled targets (SPEC §6, GLOSSARY).
  - Effort matching is **strict**: exact lowercase names only, with no trimming or case-folding.
  - `null` dataset labels are rejected; to leave a target unscored, omit its key.
  - An empty predictions file `[]` is rejected. The user approved this.
- **Judge Instructions (ADR-0004), added by the user during planning:**
  - They are a **separate axis**, so a Trial is Candidate × Judge Instructions × Case × Target × repetition.
  - Each set is a Markdown file `prompts/<name>.md`, identified by name plus the SHA-256 of the file, and evidence also stores the full text.
  - Sets are selected with `--instructions PATH`. They are **never editable through the CLI or TUI**. The bundled default is `prompts/neutral.md`.
  - The user wants to compare instruction sets side by side, as accuracy per Decision Model × instructions. That comparison was added to #9's body, and #8's trial axis was amended.
  - The neutral text deliberately drops effortless's "Opus one step lower" and "keep current effort" rules.
- **Provider routes (SPEC §4a):**
  - Both routes use one request body: `{model, state:{target_model, recent_conversation?, next_message}, questions:{effort:{type:"choice", instructions, criteria}}}`.
  - `current_effort` and `current_model` are **omitted**. Only the effort question is asked.
  - Zen uses the **documented** URL `https://opencode.ai/inference/systemone/v1/systemone`, not effortless's `/zen/v1/systemone`. The user chose this.
- **No user-facing URL override (option B, chosen by the user):**
  - Candidates have no `url` field, and an old config with one fails with `unknown field url`.
  - The only redirect is the test hook `DECISIONBENCH_TEST_BASE_URL`. It is documented as tests-only, because it also redirects the API key.
  - Evidence records the URL actually used in a top-level `url` field.
- **Credentials** come only from `OPENROUTER_API_KEY` (required) and `OPENCODE_API_KEY` (optional; free Zen models need none). Code never reads key files.
- **Evidence:**
  - One immutable file per evaluation, mode 0600, in a folder with mode 0700, opened with `create_new` **before** the request is sent.
  - The grade is never stored; `show` recomputes it.
  - `schema_version` is 1.
- **Exit codes:**
  - 0: answered
  - 3: provider or transport failure, evidence saved
  - 1: input, config, key or `--out` error, nothing sent
  - 2: usage error
- **Process preferences the user expressed:**
  - Use **Fixer subagents** for code edits. The user asked for this mid-session; I write docs and tickets myself.
  - Ask about the merge strategy each time. The merge-commit style has been used every time so far.
  - Don't push to a branch with an open PR without asking.
  - Don't start dependent tickets.

## Work completed

- **#5**, [PR #14](https://github.com/capyBearista/decisionbench/pull/14): `decisionbench grade PREDICTIONS.json [--dataset]`, with `src/grade.rs` and the `null`-label rejection in `src/dataset.rs`.
- **#6/#7**, [PR #15](https://github.com/capyBearista/decisionbench/pull/15):
  - `decisionbench evaluate --candidate --case --target [--instructions --config --dataset --out]` and `decisionbench show FILE`.
  - Modules: `src/provider.rs` (routes, request body, `send` via blocking `ureq` 3, normalization), `src/evidence.rs` and `src/instructions.rs`.
  - New dependencies: `ureq`, `toml`, `sha2`, and `serde_json` with `preserve_order` (keeps the criteria in scale order).
- **Review fixes** (in PR #15):
  - The evidence file is reserved before sending.
  - Echoed keys are redacted.
  - Control characters are stripped from terminal output.
  - The HTTP status is kept on a body-read failure.
  - Non-UTF-8 bodies are decoded lossily. Devin found this; it is in `1e1a79e`.
- **Follow-up `57b4683`:** the URL override was removed, and tests were added for the default config and output folders.
- **Docs:** ADR-0004 (new), GLOSSARY ("Judge Instructions"), SPEC §4a, §4–§6 and the outstanding-verification items, and the README.
- **Issue bodies edited:**
  - #2: status, links, dataset text, checklist and Judge Instructions.
  - #8: trial axis.
  - #9: compare instruction sets; rename "prompt" revisions to "Judge Instructions" revisions.
- **Devin's comments on PR #15:**
  - The non-UTF-8 finding was fixed.
  - The two URL-override "won't fix" replies were posted, but they are now moot, because the override was removed afterwards.

## Investigations, attempted approaches and pitfalls

- **Zen has never answered successfully.**
  - On 2026-11-03 the free model `jev-1.13-free` returned `429 FreeUsageLimitError` with `Retry-After: 74815` (about 20.8 h). Both Zen URLs share that quota.
  - The paid `jev-1.13` with no key returns `401 AuthError "Missing API key."`, so free models skip authentication.
  - **Unknown:** whether Zen accepts a `state` without `current_effort`, and what a successful reply looks like. Effortless also parses `result.answers`. That path was deliberately **not** added until it is observed. Any other shape grades as invalid, and the raw reply is kept.
  - The user said not to treat the rate limit as a failure.
- **OpenRouter replies with a dated model** (`typesafe/jev-1.13-20260917`) and a `provider` (`TypeSafe`). Both are stored as `response_model` and `response_provider`.
- **ureq 3 gotchas** (checked in the source and by experiment):
  - It honours the `*_PROXY` and `NO_PROXY` variables by default.
  - `read_to_string` **fails** on non-UTF-8 bodies despite a `lossy_utf8` flag in its source. Use `read_to_vec` and `from_utf8_lossy`.
  - The body limit is 10 MB.
- **Subagent limits:**
  - `code-reviewer` subagents cannot run cargo or git (Bash is blocked), so they review by reading code only.
  - Fixer subagents *can* run cargo.
  - Always re-run the checks yourself before claiming results.
- **GitHub CLI:** `gh issue view` and `gh pr edit` fail with the "Projects (classic)" GraphQL error. Use `gh api` REST calls (`PATCH repos/.../issues/N -F body=@file`, `.../pulls/N/comments/ID/replies`).
- **Devin Review** runs on every PR as a status check. Wait for it to finish (the PR shows as `UNSTABLE` until then) and read its inline comments before merging.
- **Live smoke runs:**
  - The OpenRouter key is in `~/.config/openrouter.env` (from effortless). Load it into `OPENROUTER_API_KEY` in the shell for manual runs only; the code never reads that file.
  - Never print the key.
- **Known accepted limits:**
  - The 60 s timeout is fixed (`shortcut:` comment, to be made configurable in #8).
  - Usage is copied raw, not normalized across routes.
  - The path that prints evidence to stderr when the final write fails is untested.
  - A Markdown `## ` line inside a code fence would start a new instruction section.

## Remaining work

1. **Zen smoke check**, after the quota resets (from about 2026-11-04T23:00 UTC, based on the 429's `Retry-After`).
   - Set up a `zen` candidate with `model = "jev-1.13-free"` and run `decisionbench evaluate --candidate <zen> --case c002 --target opus`.
   - If the reply is a 200 with `answers.effort.choice`, update SPEC §4a to say a successful Zen reply was observed.
   - If the reply differs (for example it uses `result.answers`), add that one path in `provider::normalize`, with a test using the real body as a fixture.
2. **Next ticket** (ask the user which one):
   - **#8:** batches, retries and cancellation. It must count Judge Instructions in the trial axis and make the timeout configurable. It will need `attempt` to become a list, which means a `schema_version` bump.
   - **#10:** manual prices per Provider Route.
   - Both depend only on #6, which is closed.
3. **#9** after #8: per-Candidate × Judge Instructions accuracy, with a warning when one name has several hashes.
4. **Optional housekeeping:** delete merged remote branches if the user asks, and commit this HANDOFF.md.

## Resume instructions

- **Start here:** ask the user whether to start #8 or #10, or whether to run the Zen smoke check first if its quota has reset. Then read that issue with `gh api repos/capyBearista/decisionbench/issues/N --jq .body`.
- **Read first:**
  - docs/SPEC.md (§4a, §5, §6)
  - docs/adr/0004-judge-instructions-axis.md
  - GLOSSARY.md
  - src/provider.rs, src/evidence.rs, src/main.rs (`evaluate`)
  - tests/cli.rs (the stub-server helpers, `Eval`, and `DECISIONBENCH_TEST_BASE_URL`)
- **Suggested skills:**
  - `shared-workflow:implement`: the user's standard flow.
  - `shared-workflow:test-driven-development`.
  - `shared-workflow:verification-before-completion`.
  - `shared-workflow:requesting-code-review`, then the `shared-workflow:code-reviewer` agent.
  - `shared-workflow:fixer`, for code edits.
  - `merge-pr`, only when the user asks.
- **Avoid repeating:**
  - Don't re-ask: unscored before invalid, strict effort matching, the Zen URL choice, removing the URL override, Judge Instructions as Markdown files on a separate axis, or merging #5 into #6.
  - Don't reintroduce `current_effort`, a candidate `url`, the effortless instruction text or a `result.answers` path without evidence.
  - Don't treat the Zen 429 as a failure.
