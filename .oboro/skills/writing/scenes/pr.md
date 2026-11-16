# PR

The reviewer reads first and needs to know what to check. Whoever merges needs to know what was verified. Neither needs the history of the work.

## Shape

The title follows "Titles" in the [writing skill](../SKILL.md#titles). The `## Test notes` heading and the closing reference are required by the [issue-owner skill](../../issue-owner/SKILL.md).

```text
## 概要
<何が変わるか。利用者か agent から見た振る舞いで 1-3 文。理由は Issue に任せる>

## 変更
- <振る舞いごとに 1 行。file ごとに並べない>

## Test notes
- <回した command と結果>
- 回さなかった物: <範囲と理由>

## 未確認
<なければ見出しごと書かない>

Closes #<Issue>
```

| Rule | Why |
|---|---|
| The summary states the behavior after the change | The reviewer compares it with the diff |
| Changes by behavior, not by file | The diff already lists the files |
| What was not run, with the reason | A gap left unlisted reads as verified |
| No story of how the work went | It belongs in the commit's History |

## Bad and good

Bad: the opening of a real PR. Two dense paragraphs in polite form, with 担当, 利用枠 and 回復待ち for agent, rate limit and reset.

```text
信頼するフォルダの確認、ログイン切れ、利用枠で止まった担当を Decisions の先頭へまとめます。CLI と理由、未解消の担当数、現在の画面の文を読み、人間が同じ場所で回答、認証確認、再開、CLI の切り替え、回復待ちを選べます。

人間が押した信頼の答えだけを、世代、現在画面、選択肢を送り直前に再検査して届けます。未確認の画面は Open Terminal へ案内します。
```

Good: the same content.

```text
## 概要
trust の確認、login 切れ、rate limit で止まった agent を、Decisions の先頭にまとめる。人間はその場で理由を読み、再開できる。

## 変更
- 止まった agent を、CLI と理由ごとに 1 行にまとめる
- 行から、回答、login の確認、再開、CLI の切り替え、reset 待ちを選べる
- trust への回答は、送る直前に画面と選択肢を確かめ直してから届ける。確かめられない画面は Open Terminal へ案内する
```

Good: real test notes. What ran, the result, and what did not run.

```text
## Test notes
- `pnpm check:light`: abc1234 で exit 0
- `services/release-api` の test: 43 件通過。新しい test は先に失敗を確かめた
- 回さなかった物: Cloudflare への deploy と疎通。prerelease の version 名を Cargo と Tauri が受け付けるかは未確認
```
