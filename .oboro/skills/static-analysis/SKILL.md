---
name: static-analysis
description: Use when a bug reached runtime that a type, lint, build step, or machine check could have caught; when setting up or tightening a repo's static checks; or when a check fails and you are tempted to loosen it.
---

# Static analysis

Move bugs left: what a type, lint, or check can reject must not wait for runtime. Start from oboro's recommended baseline, respect the repo's adjustments, and add checks by measured yield.

```mermaid
flowchart LR
  A[Read the repo's adjustments] --> B[Compare with the baseline]
  B --> C[Classify runtime bugs]
  C --> D[Pick checks by yield]
  D --> E[Fail on the old bug]
  E --> F[Fix the code, keep the check]
  F --> G[Record adjustments]
```

## Read the adjustments first

The repo's own settings win: read the tool configs (`rustfmt.toml`, `Cargo.toml`, `jaio.toml`, `scripts/check-all.sh`, `.githooks/`). Keep documented adjustments unless their reason no longer holds; distinguish them from missing checks.

## How oboro picks tools

Pick tools that are fast, comprehensive, and need little configuration. Prefer one integrated tool that covers lint, format, and related checks (like uv for Python projects) over a chain of narrow tools. Add a separate narrow tool only when the classification shows bugs the integrated tool misses. Confirm the current version and coverage from the tool's official documentation before adopting it.

| Stack | Default tools |
|---|---|
| TypeScript and JavaScript | The compiler for types; Biome for lint and format |
| Rust | cargo: clippy for lint, rustfmt for format |
| Python | uv for environments and runs; Ruff for lint and format |
| Other | The stack's integrated toolchain first |

## oboro baseline

The default when a repo has not decided otherwise. A repo may tighten or loosen an item by editing the tool's own config and recording the reason in this skill's repo section or `AGENTS.md`. Do not add a separate settings mechanism.

| Area | Baseline |
|---|---|
| Check command | One command runs types, lints, tests, and repo checks; every check in this table runs in it, not in a separate optional script |
| TypeScript | `strict`, `noUncheckedIndexedAccess`, `exactOptionalPropertyTypes`, `noFallthroughCasesInSwitch`, `noImplicitOverride`, `noUnusedLocals`, `noUnusedParameters`. Lint `any`, non-null assertions, and unchecked `as` as errors outside generated code |
| JS/TS lint | Biome's recommended set, run with warnings as errors. Unused-code or duplicate checks from separate tools only when measured yield justifies them |
| Rust | `clippy` with warnings as errors, plus a `[lints]` table that decides `unwrap`/`expect` outside tests, holding locks across `await`, and blocking calls in async code |
| Boundaries | Types that cross a process or language boundary (IPC, HTTP, files) are generated from one source (OpenAPI or the backend's types), and a check fails when the copy drifts. No runtime schema validation on hot paths |
| Size | Files at most about 400 lines, excluding tests; functions at most about 60 lines and low cognitive complexity. Enforce with the stack's lint (for example clippy `too_many_lines` and cognitive complexity) and a repo check for file length |
| Narrow types | Identifiers, units, and states use dedicated types (Rust newtypes, TypeScript branded types and unions), not bare `string`, `number`, or `bool`, wherever mixing them up is possible |
| Side effects | I/O, processes, time, and randomness sit in thin adapters behind typed interfaces; the logic that decides takes plain inputs and is tested without them |
| Repo checks | Patterns specific to the repo that a machine can decide, each with its own test, in the repo's check directory |

## Classify runtime bugs

For recent fixes of runtime failures, record for each: the symptom, the cause, and which check would have rejected it (type setting, lint rule, generated boundary type, repo check, or none). Count per check. "None" is common for design faults (races, lifecycle, limits); route those to integration tests instead of forcing a static rule.

## Introduce limits without stopping work

When existing code already breaks a size limit, ratchet: record each over-limit file at its current size, fail only when it grows, and shrink it when a change touches it. New files meet the limit from the start.

## Add checks by yield

| Rule | Why |
|---|---|
| Add the check that would have caught the most past bugs first | Effort goes where failures were |
| Show the new check failing on the code before the fix | A check that never fails proves nothing |
| Fix existing violations; never weaken the check to pass | Same as never weakening a test |
| Write a project-specific check only for a pattern that recurred or caused a severe failure | Each check has upkeep |
| Keep false positives near zero; a noisy rule gets disabled | Mask comments and literals; test the fixed code and nearby valid forms |

## Record

Keep the checks in force and each deviation from the baseline with its reason in this file's repo section (or an Issue when it needs discussion). When a new runtime failure shape appears, record it there and decide whether a check is worth it.

## This repo

| Item | Use |
|---|---|
| Check command | `./scripts/check-all.sh` (fmt + clippy + test + jaio self-application + script syntax + CI-ban guard). Per-PR scope: the testing skill's [Check scope](../testing/SKILL.md#check-scope) |
| Format | `cargo fmt` with `rustfmt.toml` (`style_edition = "2024"`) is the source of truth |
| Lint | `clippy --workspace --all-targets -- -D warnings`; warnings are never committed |
| Repo checks | `scripts/check-all.sh` steps 4-6: jaio self-application on this repo's own documents, script syntax checks, and the `.github/workflows` ban |
| Known runtime shapes | Markdown syntax handling that left comrak's sourcepos; path matching that did not go through `jaio_core::relativize_for_match` (see AGENTS.md, 文字列とパスの処理) |
