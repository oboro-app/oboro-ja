---
name: release
description: jaio のプライベートリリースを GitHub Releases に作成する。feat/fix を push したセッションで実行(変更ベースの機械的判断)
---

jaio リポジトリでリリースを行うときの標準手順。正本は `docs/release.md`。

## 手順

1. `git pull origin main` で最新化
2. 未リリースの変更を機械的に判定:
   ```sh
   git fetch --tags
   git log $(git describe --tags --abbrev=0)..main --format='%s' | grep -E '^(feat|fix)(\(|:)'
   ```
   出力があればリリースする(feat あり→マイナー、fix のみ→パッチ)。
   `docs` 等のみならリリースしない
3. `gh auth status` でリリース権限のあるアカウントが active であることを確認
4. バージョニング(docs/release.md 参照)に従って Cargo.toml の version を更新してコミット
5. **bump コミットを `git push origin main` で push する**(未 push だと release.sh が中止する、#78)
6. `./scripts/release.sh` を実行(品質ゲート → ビルド → タグ → アップロード)
7. `git push origin main --tags`
8. `gh release view v<version> -R oboro-app/oboro-ja` で完了確認

## 判定基準(変更ベース・機械的。日数は使わない)

- **リリースする**: 前回タグ以降に `feat` または `fix` のコミットがある
- **しない**: `docs`/`chore`/`test`/`refactor`/`style` のみ、品質ゲートが失敗する
- **セキュリティ対応**: `fix` 扱い。できるだけ早くリリースする

## 参照

- `docs/release.md`: リリース運用の正本(判定・配布物・トラブルシュート)
- `scripts/release.sh`: 実行スクリプト
