---
name: writing
description: Use before writing or revising any text a human reads (a report, an Issue, a PR, a review comment, a Decision, UI text, docs, a release note, a commit message, a message or chat reply to a human), to decide the reader, what they want first, the shape and the length, and to choose the words.
---

# Writing

Carried into the harness oboro generates for each repo. The examples here and under `scenes/` are abstracted from real text; their terms (Main, Partner, Watcher, Resources, Usage) are oboro's app and workflow terms.

Text exists to move one reader to one decision or action. Grammar comes last: a correct sentence the reader cannot use is a defect.

Language rules are in [writing-ja](../writing-ja/SKILL.md) and [writing-en](../writing-en/SKILL.md). Where each language goes is the repo's Notation in [AGENTS.md](../../../AGENTS.md). The chat persona never enters documents, comments, commits, or reports.

## Decide before writing

| Decide | Ask |
|---|---|
| Reader | Who reads this, what do they already know, and which words do they use at work |
| First | What does this reader want first: the verdict, the request, or the danger. Put it in the first line |
| Shape | Which scene below is this. Use its shape; do not invent one |
| Length | How much will this reader read here. Cut to that and move the rest behind a link |

Write the point as one sentence before drafting. If you cannot, you do not know it yet; do not start.

## Scenes

Find the scene, then read its file before writing. Each file has the shape to fill in, and bad and good examples.

| Scene | Reader | Wants first | Shape | Length |
|---|---|---|---|---|
| [Report to a human](scenes/report.md) | The human who handed off the work and comes back later | Is it done, and must I do anything | Verdict line, then changes, verified, unverified, next decision, harness learnings | One screen |
| [Issue](scenes/issue.md) | Title: someone scanning a list. Body: an owner who starts cold | What is wrong or wanted, and when it is done | Title by "Titles". Body: goal, current state, what to do, acceptance criteria | Title: one clause. Body: one screen |
| [PR](scenes/pr.md) | The reviewer, then whoever merges | What changes in behavior, and how it was verified | Summary, changes by behavior, test notes, unverified, closing reference | One screen |
| [Review comment](scenes/review.md) | The owner who must fix it | The verdict, then the worst finding | Verdict line, then findings by severity: place, what breaks, wanted state | 2-5 lines per finding |
| [Decision](scenes/decision.md) | A human with no context, deciding alone | What happened | The five parts below | One screen |
| [UI text](scenes/ui.md) | A user in the middle of a task, glancing | What this is, or what to do next | A registered term; an explanation says what happened and what to do | A word or two; one sentence to explain |
| [Docs](scenes/docs.md) | Someone who looks up one fact later | Whether this section answers the question | Lead, then a table or diagram; one fact in one place | 1-3 sentences per lead |
| [Release note](scenes/release-note.md) | A user deciding whether to update | What they can do now | Sections by kind, one item per change | One line per item |
| [Commit message](scenes/commit.md) | A maintainer reading `git log` | What changed, then why | Subject by "Titles"; body sections from Notation | Subject: one clause |
| [Message to a human](scenes/message.md) | A human interrupted in other work | What you need from them, or that nothing is needed | Request or answer, reason, options with results | 3-6 lines |

Text that fits no scene still gets the four decisions above.

## Titles

For Issue titles, PR titles and commit subjects. A title is a label read in a list among fifty others, not a summary of the body.

| Rule | How |
|---|---|
| One clause, one verb | `<type>: <what> <does what>`. Always give the type. A second clause, a list joined by "and", or a parenthesis goes to the body |
| Say what the reader can see | `fix`: what goes wrong, in the reader's words; the error text goes to the body. `feat`: what becomes possible; the purpose goes to the body. `perf`: what is slow, with the measured number if there is one. `docs`, `chore`: what changes |
| Keep what tells it apart | Who or what, and in which state: 「error 状態の agent」「review を記録した reviewer」. A state the reader can see is not a cause |
| No internal names | No function, file, variable, lock or event names: names only the code has. A registered term (slot, Usage, Gate) and a command the user types are not internal |
| No cause, list of parts, or steps to reproduce | They are the body's job |
| Short | Japanese: under about 40 characters. English: under about 60 |
| Cut, do not unpack | When a title strings nouns together, drop parts until one claim is left. Do not expand the chain into clauses; the sentence rules of the language skills are for bodies |

## Decisions and questions to a human

Any text that asks a human to decide (a Decision, an `obr problem ask`, an `oboro:confirm` Issue, an `oboro` block) has these five parts, in this order. A missing part is a defect.

| Part | Content |
|---|---|
| What happened | One line: the fact, not the cause you suspect |
| Why a human | Why no agent can decide: product direction, cost, scope, or a permission only a human holds |
| Options and results | Each option with what happens if chosen (cost, risk, what changes) |
| If left alone | Whether it resolves by itself (when) or stays stopped |
| Where to read more | Links to the Issue, PR, agent, CLI or session; a way into the log or Terminal |

## Words

Use the word the reader already uses at work. The test: would a developer who has never seen this project understand the word, and say it in a stand-up? If not, replace it.

| Rule | How |
|---|---|
| Technical and English terms stay as they are | agent, reviewer, worktree, rate limit, reset, version, PR, CI, session, slot, daemon, lock, log. Keep the English spelling; do not translate or transliterate them |
| Names follow their owner | Spell a product, CLI, service or screen name as its owner does: GitHub's Issue and Pull request, the repo's own term list |
| One thing, one name | A role, component or mechanism with a registered name is called by it everywhere. Do not give it a second name in the native language |
| Never coin a word | Do not make up a noun for a state, a place or a thing. Use the existing term, or say it as a plain clause |
| Never force a translation | Do not swap a standard term for a literal or rare native word. An everyday loanword that is not a technical term is not a translation and stays |
| Explain, do not rename | If the reader may not know a term, add a short clause after it |
| A coined word you find joins the table | Add it with its replacement to the language skill's table in the same PR |

Coined words found in this repo, and what to write instead: [writing-ja](../writing-ja/SKILL.md#coined-words-and-their-replacements).

## Sentences and structure

| Rule | Meaning |
|---|---|
| Conclusion first | State the result or request in the first sentence or heading; support comes after |
| One idea per sentence | Split a sentence that carries two claims |
| Active voice | Name the actor; a passive that hides who acts is a bug |
| Concrete words | Name the thing, count, or command; not "various", "appropriate", "some" |
| Verified only | Write what you did not check as unverified; never dress a guess as a fact |
| Structure follows content | Three or more of one kind: table. Flow, state, structure: diagram. Prose: 1-3 sentences at the top of a section |

## AI slop

These make text read as generated, and the reader stops trusting the rest. Remove them on sight. One pattern per row; the "Match" strings are in this repo's language so a check can use them.

| Pattern | Match | Fix |
|---|---|---|
| Exaggeration | 大幅に、劇的に、完璧に、非常に、極めて、包括的、堅牢、シームレス、強力な | The number, or the concrete change |
| Empty opener | 承知しました、了解しました、ご質問ありがとうございます、以下に示します、以下のとおりです | Start with the content |
| Ability filler | することができます、することが可能です | The plain verb: 「確認できます」 |
| Noun-plus-verb filler | を行います、を行う、を実施します、を図ります | The verb itself: 「確認を行う」は「確認する」 |
| Empty closer | 以上です、ご不明な点があれば、お気軽に、いかがでしょうか、幸いです、お役に立てば | End on the last fact or request |
| Hedge pile | と思われます、と考えられます、かもしれません、可能性があります | State it, or write once that it is unverified |
| Self-labeling | 重要なのは、ポイントは、注目すべきは、結論から言うと、端的に言えば | Say the thing |
| Closing recap | まとめると、要するに、総じて | Cut the paragraph; it repeats the text above |
| Bullet overuse | Bullets of one short phrase each; nested bullets | 1-3 sentences, or a table when items share columns |
| Bold overuse | More than one bold phrase in a paragraph; a bold label opening every bullet | No bold, or one key phrase |
| Decoration | Emoji, arrows and other non-ASCII symbols in headings and lists | Plain text |
| Coined or translated word | The table in the language skill | The term the reader uses |

## Revision

Revise in this order. Each pass is one read from the top.

1. Reader: read the first line as the reader. If it is not what they want first, move that up.
2. Read it aloud in your head. Split or rewrite any part you stumble on.
3. Words: replace coined, translated and abstract words with the term or the concrete thing.
4. Cut: ask of each sentence what the reader loses without it. Cut what loses nothing, then scan the AI slop table.
5. Claims: every claim is verified, or marked unverified.

## Sources

| Source | Used for | Checked |
|---|---|---|
| [Plain language principles](https://digital.gov/guides/plain-language/principles) (US GSA) | Audience first, topic sentence, active voice, organize with tables and lists | 2026-11-06 |
| [Microsoft Writing Style Guide, top 10 tips](https://learn.microsoft.com/en-us/style-guide/top-10-tips-style-voice) | Fewer words, read aloud, get to the point, prune excess, start with verbs | 2026-11-06 |
