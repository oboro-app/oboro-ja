# Message to a human

A question, a notice, or a chat reply. The reader is interrupted in other work and has no context. They want to know what you need from them, or that nothing is needed.

This shape is for a request with one action, a notice, or an answer. A message that asks the human to choose between options is a Decision: use the [five parts](../SKILL.md#decisions-and-questions-to-a-human) instead, at their length.

## Shape

```text
<頼みたいこと、または答え。1 文>
<理由。1 文>

- <してほしいことの詳細。なければ書かない>
- <応じない間に起きること>
```

| Rule | Why |
|---|---|
| The request or the answer is the first sentence | A status report with the request at the end is read as a report |
| One request per message | Two requests get one answer |
| In chat, answer the question asked, then stop | A recap and an offer of more help are noise |
| The persona stays in chat | A question, a notice and a report are records. Their register is in the language skill |

## Bad and good

Bad: a real message. The request is in the third sentence, after the status, and 枠 stands for slot.

```text
Issue #123 / PR #124 は製品コード abc1234 で pnpm check:light と関連 177 本が成功し、検証知識を def5678 で補正しました。designer の agent start はマシン 16/16 で拒否されました。完了に必要な独立 CLI レビューも枠が要るため、Main 側で終了済み補助役の整理か先行 PR の完了を進め、レビュー枠を確保できますか。製品方針の未決定事項はありません。
```

Good: the same message.

```text
review 用の slot を 1 つ空けてください。slot が 16/16 で埋まり、designer と reviewer を起動できません。

- 空け方: 終わった support の agent を止めるか、先行する PR を merge してください
- 空かない間: PR #124 は review を待ったまま進みません
- 実装は済み、`pnpm check:light` は通っています
```
