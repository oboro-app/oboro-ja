# Commit message

The reader runs `git log` or blame months later. The subject tells them what changed; the body tells them why.

The commit message form is the repo's Notation in [AGENTS.md](../../../../AGENTS.md). Decision records, when split out, live under `docs/` per the docs map.

## Shape

```text
<type>: <何が変わるか。題名の決まりに従う>

## Intent
<なぜ変えるか。困っていたことと、変えた後の状態>

## History
<試して捨てた案と、分かったこと。次に触る人が同じ道を通らないために書く>
```

| Rule | Why |
|---|---|
| The subject follows "Titles" in the [writing skill](../SKILL.md#titles) | `git log --oneline` is a list |
| One change per commit, so one clause per subject | A subject joined by "and" is two commits |
| Intent is the reason, not the diff retold | The diff is already there |

## Bad and good

Real subjects from this repo.

| Bad | Good | Why |
|---|---|---|
| fix: 記録を終えて待つレビュー役を枠の数えから外す | fix: review を記録し終えた reviewer を slot に数えない | レビュー役 is reviewer and 枠 is slot. 数え is a verb turned into a noun |
| fix: 検証用 build の保存先を隔離し、デーモンの上限を上げ、担当がいる入れ替えに確認を取る | Three commits: 「fix: 検証用 build の保存先を隔離する」「fix: daemon の上限を上げる」「fix: agent がいる時は入れ替えの前に確認を取る」 | Three changes in one subject |

Good as it is: 「fix: 再起動で戻したタブの点を閉じる前と同じ状態の色にする」. One clause, and the reader can see the behavior.
