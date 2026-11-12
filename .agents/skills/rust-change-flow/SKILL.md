---
name: rust-change-flow
description: jaio のコード・文書を変更し、品質ゲートに通してコミットするまでの標準手順。何かを変更するときに使う
---

jaio のリポジトリで変更を行うときの標準手順。正本はリポジトリルートの `AGENTS.md`「変更フロー」。

## 手順

1. 対象ファイルと既存差分(`git diff`)を確認してから変更する
2. 設計と矛盾する変更が必要な場合は、先に `docs/design.md` を更新する
3. 変更後は品質ゲート `./scripts/check-all.sh` を実行する(スキル `check` と同じ)
4. 未解決事項・検証待ちが残る場合は GitHub Issue を起票する。この変更で解消した項目があれば該当 Issue をクローズする
5. コミットメッセージは変更の意図を 1 行で書く(conventional commits 風: feat / fix / docs / chore / test / refactor)

## 禁止事項

- 作業進捗を実装コードや `docs/` に書かない(未解決事項は GitHub Issues だけに置く)
- `.github/workflows` を追加しない(CI 不使用・ローカル品質ゲート運用)
- 無関係なリファクタリングやフォーマット変更を混ぜない
- emoji を使わない。コメントは日本語、識別子は英語
