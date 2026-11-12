---
name: check
description: jaio リポジトリの品質ゲート(fmt + clippy + test + スクリプト構文 + CI禁止ガード)を実行し、失敗があれば修正まで行う。変更後の確認やコミット前に使う
---

品質ゲートを実行して結果に対応する。

## 手順

1. リポジトリルートで `./scripts/check-all.sh` を実行する
2. 失敗がある場合: 原因を特定して修正し、再実行して全項目合格を確認する
3. 全項目合格の場合: 変更内容を要点だけ報告する(コマンド出力の全文貼り付けは不要)

## 品質ゲートの内容(scripts/check-all.sh)

1. `cargo fmt --all --check`
2. `cargo clippy --workspace --all-targets -- -D warnings`
3. `cargo test --workspace`
4. スクリプト・フックの構文チェック(`bash -n`)
5. CI 禁止ガード(`.github/workflows` の存在検出)

運用の正本はリポジトリルートの `AGENTS.md` を参照すること。
