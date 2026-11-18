---
name: testing
description: Use when writing or changing code, adding or fixing tests, debugging a failing test, or verifying behavior in the real app.
---

# Testing

Tests are evidence of completion. When behavior changes, make its test fail first, then fix. Acceptance criteria come from `bdd` skill scenarios.

## What a test suite must cover

Judge every change's tests on four axes, in this order.

| Axis | Rule |
|---|---|
| Scenarios first | Unit tests derive from the `bdd` scenarios. The user's real flows tell which inputs can happen: narrow impossible ranges with types instead of testing them, and decide from the scenario which error each failure returns |
| Boundaries and classes | Every unit test covers all boundary values and each equivalence class of its inputs (empty, one, many, limit, limit plus one, absent, invalid) that the types still allow |
| Density | Write many cases compactly (a table of named cases looped in one Rust test, or the stack's parameterized form). One readable table per behavior beats many copied tests |
| Coverage | Changed code is covered. Uncovered branches either get a case or a written reason |

Performance belongs in the verification items too: when a scenario or change touches a hot path (parsing, indexing, similarity over large corpora), add a measured check under equal load; design.md §10 keeps the measurement policy.

## Where to verify

The closer to the outside, the stronger the evidence and the slower. Pick the innermost layer that can catch what you want to verify.

| Layer | Target | Location |
|---|---|---|
| Module | Rule logic, AST, normalization, config | `#[cfg(test)]` in `crates/*/src/` |
| Cross-crate behavior | CLI output contract, schema files | `crates/jaio-cli/tests/` (drives the real binary or library entry) |
| Harness | Check tooling | `scripts/test_*.py` |
| Real CLI | Look, exit codes, output a user reads | `cargo run -p jaio-cli --features embedded-dict` on a real corpus (steps below) |

## How to write

- Name the test after the behavior to protect. Do not repeat the function name.
- Use real things over mocks: a temporary directory tree for file walking, real Markdown input for parsing, real config text for `jaio.toml` handling.
- Put temporary files and directories in `tempfile` and always clean up, including child processes.
- Do not wait with `sleep`; wait with a deadline until a condition holds. The condition is "usable".
- When a fixture asserts a relative path, build it from the repo's own fixtures (for example `crates/oboro-jaio/tests/fixtures/`) and keep the exclusion list in `jaio.toml` in sync.
- When adding a rule, give it: a minimal positive case, a minimal negative case, and a case at the threshold (the config value boundary). One table per rule beats scattered tests.
- When a rule reports line numbers or sourcepos, assert the exact position on a fixture; Markdown link and heading handling must stay inside comrak's sourcepos, not raw-text normalization.
- When a test fails only occasionally, treat it as a bug that was passing by environmental luck. "Cannot reproduce" is not completion.

## New worktree

A worktree has an empty `target/`. The first `cargo test` or `clippy` builds every dependency and can far exceed 5 minutes; start that first build in the background (or with a raised timeout through `resources run`) rather than reading the timeout as a failure.

Run every cargo command through `resources run`, never bare, so the machine budget applies.

## When a test fails

For a statically decidable runtime bug, follow the [static-analysis skill](../static-analysis/SKILL.md).

1. Read the failure and identify the cause. Do not fix by guessing.
2. Decide whether code or test is right, using spec or the real thing.
3. If the test was wrong, fix it in a way whose reason you can explain.
4. Do not weaken expectations to pass. Do not delete tests. Do not clean up with `git checkout -- .` or `git reset --hard`.

A test that passes locally but fails in another checkout (different OS, dictionary availability, parallel build load) is an environment-dependent bug: repeat under the failing conditions before claiming a fix, and record conditions and pass/fail counts.

## Verifying with the real CLI

Even if automated tests pass, do not treat user-visible output as verified until produced by the real command on a real corpus.

1. Run `cargo run -p jaio-cli --features embedded-dict -- check <path>` (and `stats`, `rules`, `explain` when touched) on the repository itself or a fixture corpus. The default build embeds no dictionary, so `check`, `stats` and `dump` need the feature or `[analysis] dictionary` in `jaio.toml`.
2. Read the actual output: severity, rule ID, file, line. A command that exits 0 with wrong output is not verified.
3. For `--format json`, validate the output against `docs/schema/` and the schema tests in `crates/jaio-cli/tests/schema.rs`.
4. If cleanup is needed (temporary corpora, caches), verify the cleanup steps too, and report what verification left behind.

## Check scope

Per PR, build only what the change needs. This binds the implementer and every reviewer. Reuse recorded results for the same commit and command. Review the PR's check scope and run missing affected checks; do not repeat a check on each review round.

| Change | Run |
|---|---|
| Any | `cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D warnings` |
| Touches Rust (`crates/`, `*.rs`, `Cargo.toml`) | Also `cargo test --workspace` with a filter for the changed scope when the full suite is not needed. No unfiltered run just to watch it pass |
| Touches docs or harness only | No cargo command beyond fmt/clippy on unchanged Rust (they pass unchanged). Run the scripts touched with `bash -n` (shell) or `ast.parse` (python), and the evaluator behavior tests `python3 scripts/test_eval_*.py` when `scripts/eval-*.py` changed |
| Full `./scripts/check-all.sh` (adds jaio self-application) | On pre-push and release, not per PR; run it when the change touches documents the self-application reads or the gate itself |

`cargo test` takes one filter; it is a substring, not a regex. Run several tests with a shared substring, or several `cargo test` calls in one `resources run`. Write the scope run and not run in the PR's "Test notes".

A queued `resources run` reads the worktree when the run starts, not when it was queued. To keep fail-first evidence, commit only the failing tests and queue their run before writing any implementation.

## Reading check results

If `check-all.sh` stops midway, later checks do not run. Do not filter the output; confirm the exit code.

## Report

Separate the check results (fmt, clippy, the filtered tests), the scope verified, and what was not verified. Do not mix passing tests with what was seen in the real CLI. A confirmation whose precondition was off does not count as verified.
