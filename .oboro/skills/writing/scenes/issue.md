# Issue

Two readers. The title is read in a list by someone choosing what to open. The body is read by an owner who starts cold and by the human who sets priority; both want the goal and the test for "done" before any detail.

## Title

Follow "Titles" in the [writing skill](../SKILL.md#titles). Real titles from this repo:

| Bad | Good | What moved to the body |
|---|---|---|
| yomiyasu の追随の本文と git の設定とパスの検査の止めない指摘 7 つを直す | fix: yomiyasu の取り込みに review が残した指摘 7 件を直す | The list of parts. It is a table in the body |
| fix: error の担当を agent stop で止めると「デーモンが応答しません」で止まらない | fix: error 状態の agent を `obr agent stop` で止められない | The error message and the second clause. 担当 is agent |
| project_attention が screen_read_turn を数秒待ち、要求のたびに全プロジェクトの確認の Issue と台帳を読み直す | perf: 判断待ちの読み込みが p95 で 9 秒かかる | The function names and the cause. The title keeps the symptom and the number |
| perf: 担当の起動が spawn の鍵で直列になり、鍵の中の約 40 秒が計測されないまま待たされる | perf: agent の起動が p95 で 40 秒を超える | The cause and the missing measurement. 鍵 is lock |

## Body

```text
## 目的
<誰が何に困っているか、直ると何ができるか。1-3 文>

## 現状
<bug は見える症状と再現の手順。原因の推測は「原因の見立て (未確認)」に分ける>

## やること
1. <順番がある時は番号、ない時は箇条書き>

## 受け入れ条件
- <test か画面で確かめられる文。「正しく動く」は条件にならない>
```

| Rule | Why |
|---|---|
| Symptom, reproduction and suspected cause in separate sections | A guess written as a fact sends the owner to the wrong place |
| Acceptance criteria a test or a screen can show | The owner and the reviewer must agree on "done" |
| Measurements and logs in a table, or behind a link | The body stays one screen |
| Function and file names are fine in the body | Only the title bans them |

## Bad and good

Bad: symptom, failed reproduction and suspected cause share one paragraph, with three coined words.

```text
## 問題
error 状態の Grok のレビュー役に `obr agent stop` を流すと、「デーモンが応答しません」で返って止まらないことがある。シェルのセッションで SIGHUP/TERM を無視する子と state=error を作った再現では 53ms で止まり、再現できなかった。`close_with` は spawn の鍵を 2 回取り、kill に失敗すると record を外した後に Err を返す経路がある。
```

Good: the same facts.

```text
## 目的
error 状態の agent を `obr agent stop` で確実に止められるようにする。

## 現状
error 状態の reviewer (Grok) に `obr agent stop` を実行すると、「デーモンが応答しません」と返り、agent が止まらないことがある。
再現はできていない。SIGHUP と TERM を無視する子 process を持つ session では、53 ms で止まった。

## 原因の見立て (未確認)
`close_with` が spawn の lock を 2 回取る。kill に失敗すると、record を外した後に Err を返す経路がある。

## 受け入れ条件
- error 状態の agent を `obr agent stop` で止められる。止まらない時は理由が返る
```
