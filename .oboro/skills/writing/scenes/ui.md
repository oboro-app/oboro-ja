# UI text

The reader is in the middle of a task and glances. A label must be a word they already know. An explanation must say what happened and what to do next.

Repo-specific: this repo has no screen. The nearest scene is user-facing CLI text (diagnostic messages, `--help`, JSON field descriptions); the term list is the repo's Notation in [AGENTS.md](../../../../AGENTS.md).

## Shape

| Kind | Shape | Length |
|---|---|---|
| Label, button, state word | A registered term | A word or two |
| Explanation, tooltip | What this is, or what happened; terms as they are | One sentence |
| Error, warning | What failed, why, and what to do next | One or two sentences |
| Empty state | What is absent, as a plain clause | One clause |

| Rule | Why |
|---|---|
| The text never contradicts the state | A wrong word is worse than none |
| A time or a count beside a waiting state | "Waiting" alone does not say for how long |
| No noun made up for a state | The user learned rate limit and reset from the CLI, not from this app |

## Bad and good

Real text on screen. The right column shows the words; whether a label fits its frame is the screen design's job.

| Bad | Good | Why |
|---|---|---|
| 動いている担当 3 件 | 動いている agent 3 件 | 担当 is a second name for agent |
| Usage の回復を待っています | rate limit の reset を待っています | 回復 is a translation of reset |
| 待つ物なし | 待っている処理はありません | A noun made up for an empty state |
