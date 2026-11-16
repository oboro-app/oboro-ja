# Release note

The reader uses the product and decides whether to update. They want what they can do now, in the words they see on screen.

The sections, what never to write, and the form of an item are in the [release-notes skill](../../release-notes/SKILL.md). This file adds only wording examples.

## Bad and good

| Bad | Good | Why |
|---|---|---|
| fix: マシンの予算を待つセッションを Usage の回復待ちと分けて表示する | Resources の空きを待つ session と、rate limit の reset を待つ session を分けて表示するよう変更 | A commit subject pasted in, with 予算 and 回復待ち |
| 利用枠が尽きると Main と Partner が止まる問題を修正 | rate limit に達すると Main と Partner が止まる問題を修正 | 利用枠 is a translation; the CLI says rate limit |
