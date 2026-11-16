---
name: writing-en
description: Use when writing or revising English text, for plain wording, sentence structure, articles and number, voice ("we" vs imperative), technical-document tone, and token-efficient harness English.
---

# Writing English

Write on top of the [writing](../writing/SKILL.md) skill. This file adds what English text needs.

## Sentences

| Rule | Example |
|---|---|
| Plain words over clever ones | "use" not "utilize"; "start" not "initiate" |
| Short sentences, one idea each | Split at "and"/"which" when two claims ride one sentence |
| Start statements with the verb | "Run the checks." not "You can run the checks." |
| Conditions before instructions | "If the socket is missing, restart." not "Restart if the socket is missing." |
| Serial comma | "Claude, Codex, and OpenCode" |

## Voice

| Voice | Use it for | Example |
|---|---|---|
| Imperative | What the reader does | "Write the failing test first." |
| "we" | What this project or its agents do | "We verify each merge locally." |
| "you" | The reader's own context and choices | "If your Usage is low, start with the review." |

Never mix "we" and "you" inside one claim; name whose hand does the thing.

## Grammar that drifts

| Watch | Rule |
|---|---|
| Articles | "a daemon" first mention, "the daemon" after; plural when you mean many |
| Singular vs plural | "the agent starts" (one), "agents start" (many); do not write "the agents starts" |
| Verbs after collective nouns | "the team runs", "the list is" |
| Prepositions | "depend on", "consist of"; check fixed phrases when unsure |

## Tone for technical documents

Conversational and friendly, not frivolous: contractions are fine, jokes are not. Say what the thing does, then how, then why. Do not promise future work in a document; unshipped behavior is written as absent. Follow the [Google developer documentation style guide](https://developers.google.com/style) hierarchy where this repo does not decide.

## Token-efficient harness English

Under `.oboro/` (skills, knowledge, checks), English costs tokens on every run of every agent.

| Rule | Example |
|---|---|
| Short | "Do not merge" not "You should not perform a merge operation" |
| Tables over prose | A rule row, not a paragraph of rules |
| One fact in one place | Link to the fact; do not restate it |
| No preamble | Start at the rule; skip "It is important to note that" |

## Examples

Bad and fixed pairs, abstracted from real project writing.

| Hard to read | Easier to read |
|---|---|
| It is considered that the utilization of the stop functionality should be performed prior to the merge. | Stop the agent before you merge. |
| There is a possibility that sessions may not be restored in the event of a daemon crash. | Sessions may be lost if the daemon crashes. Unverified. |
| The various settings and configurations can be modified by the user. | Users change the tier for each role in the project settings. |

## Sources

| Source | Used for | Checked |
|---|---|---|
| [Google developer documentation style guide](https://developers.google.com/style) | Second person, active voice, conditions before instructions, lists, word choice | 2026-11-06 |
| [Microsoft Writing Style Guide, top 10 tips](https://learn.microsoft.com/en-us/style-guide/top-10-tips-style-voice) | Fewer words, verb-first statements, pruning weak phrasing | 2026-11-06 |
| [Plain language principles](https://digital.gov/guides/plain-language/principles) (US GSA) | Audience first, organize up front | 2026-11-06 |
