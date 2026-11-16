---
name: harness-review
description: Use when reviewing a change to the harness (AGENTS.md, a skill, a knowledge note, a check), when auditing the whole harness periodically, or when deciding whether a skill earns its context cost.
---

# Harness review

The harness is code that every agent runs on every task. Review it with the same rigor as code, plus its cost: every instruction and skill description is read again and again. Review someone else's harness change; do not approve your own.

## When

| Trigger | Scope |
|---|---|
| A PR touches `.oboro/` or an `AGENTS.md` | The changed files and everything they reference or overlap |
| Periodic audit (with the inventory role, or after several harness PRs) | The whole harness: list every instruction, skill, knowledge note, and check |
| A failure the harness should have prevented | The rules that applied, and why agents did not follow them |

## Criteria

| Criterion | Pass when |
|---|---|
| Changes behavior | Each line changes what an agent does. Vague advice ("be careful", "consider") and restated common sense fail |
| Triggers right | The `description` names the situations that need the skill and no others. Two skills never claim the same situation; an AGENTS principle points to the skill instead of repeating it |
| One fact, one place | No rule appears in two files; no rule contradicts another file, a skill, or `docs/design.md`. Search before approving |
| Right place | Placement follows the `harness-learning` table: all-area rule in AGENTS, repeated procedure in a skill, tool facts in knowledge, machine-decidable rules in a check |
| Earns its cost | Lines and description length are justified by how often the situation recurs and how bad the failure is. Prefer extending an existing skill over a new one; merge skills that are usually used together |
| Verifiable | A reader can tell whether the rule was followed (from a diff, a report, the ledger, or a check). Rules a machine can decide become checks |
| Abstract and portable | No Issue numbers, dates, or story in rules (repo-specific sections excepted). Repo-specific parts are marked as not for the product |
| Evidence-based | New rules cite the failure or human request behind them in the commit; a rule built from one case is a default with its reason, not an absolute |
| Respects the human | Rules match human decisions in `docs/design.md` and do not loosen an existing rule without an `oboro:confirm` decision |
| Readable | Follows the `writing` and `writing-en` skills: short, tables for 3+ items, plain English |

## Measure effect

For a periodic audit, measure instead of guessing:

| Measure | Source |
|---|---|
| Size | Lines per file and total; description characters loaded per session |
| Use | How often each skill is invoked or cited in reports and commits; skills never used are candidates to merge or remove |
| Compliance | Sample recent work: did agents follow the rule? Failures the rule should have prevented point to a trigger, wording, or placement problem |
| Outcome | Repeated failures of the same shape after a rule exists mean the rule does not work; change it or make it a check |

## Return

Findings ranked by impact, each with the file and line, the criterion it fails, and a concrete edit (merge, move, shorten, turn into a check, delete). For an audit, also the size and use table and the candidates to merge or remove. Do not edit the harness yourself unless asked; the author applies the findings.
