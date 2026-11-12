---
description: 品質ゲート一括実行(fmt + clippy + test + スクリプト構文 + CI禁止ガード)
---

!`cd "$(git rev-parse --show-toplevel)" && ./scripts/check-all.sh`

上の品質ゲートの結果を確認して対応してください。

- 失敗がある場合: 原因を特定して修正し、再実行して全項目合格を確認する
- 全項目合格の場合: 変更内容を要点だけ報告する(コマンド出力の全文貼り付けは不要)
