# Report to a human

The reader handed the work off and comes back later, remembering little. They want to know whether it is done and whether they must act. They do not want the order in which you worked.

## Shape

The parts after the first line are the repo's rule "Report honestly" in [AGENTS.md](../../../../AGENTS.md).

```text
<完了、未完了、失敗のどれか>。<済んだことか、残っていることを 1 文で>。<人間がすること。なければ「人間がすることはない」>

- 変更: <利用者か agent から見て変わったこと>
- 確認済み: <回した command と結果>
- 未確認: <確かめていないことと理由>
- 次の判断: <人間か Main が決めること。なければ「なし」>
- harness へ返した学び: <内容。なければ「なし」>
```

| Rule | Why |
|---|---|
| The verdict is the first word: 完了 (every part asked for is finished), 未完了 (still going, and it will finish), 失敗 (stopped with a part undone; it will not finish without a change) | The reader decides from it whether to read on |
| A part with nothing to say is 「なし」 | A missing part reads as forgotten |
| Facts you checked, with the command | "Mostly works" cannot be acted on |
| No account of the process | What you tried belongs in the commit's History |
| Only references the reader can open | An Issue or PR number and a commit SHA can be opened. A PID, a queue length or an internal state name is noise unless it is the request |

## Bad and good

Bad: a progress report. State, request and internals arrive in the order they happened, and the reader cannot tell whether to act.

```text
更新済み条件の実装を終え、#123 と abc1234 を取り込み済みです。旧集約で実装と同種を数えてしまう赤を確認しました。焦点試験の未起動待機は取り下げ、全体 1 回へまとめています。現在その全体検査が 13 分以上の FIFO 待機で、heavy=1、waiting=9 です。保持元 PID 17574 の obr と pnpm/node 子系統が動いており、他担当は触っていません。検査結果が出ればコミット、push して SHA を連絡します。
```

Good: the same facts.

```text
未完了。`pnpm check:light` の順番を待っている。人間がすることはない。

- 変更: review の集計が、実装と同じ種類の CLI を数えないようにした。文書 3 file は未 commit
- 確認済み: 直す前の集計で失敗する test を足し、失敗を確かめた
- 未確認: `pnpm check:light`。ほかの check の後ろで 13 分待っている
- 次の判断: なし。check が通ったら push して SHA を知らせる
- harness へ返した学び: なし
```
