---
description: jaio のプライベートリリースを作成する(変更ベースの機械的判断)
---

# jaio リリース

手順の正本は `docs/release.md`。このコマンドはそのラッパ。

## 手順

1. `git pull origin main` で main を最新化
2. 未リリースの変更を機械的に判定(日数は使わない):
   ```sh
   git fetch --tags
   git log $(git describe --tags --abbrev=0)..main --format='%s' | grep -E '^(feat|fix)(\(|:)'
   ```
   出力があればリリースする(feat あり→マイナー、fix のみ→パッチ)。
   `docs` 等のみならリリースしない
3. バージョンを決定(パッチ/マイナー/メジャー、docs/release.md 参照)し、コミット
4. **`git push origin main` で bump コミットを push**(未 push だと release.sh が中止する、#78)
5. `./scripts/release.sh` を実行(引数なしで Cargo.toml のバージョン)
6. `git push origin main --tags` でタグを push
7. `gh release view v<version> -R oboro-app/oboro-ja` で確認

## 注意

- リポジトリは private。リリースはメンバーのみ取得可
- `gh auth status` でリリース権限のあるアカウントが active であることを確認してから実行
- 品質ゲートが失敗した場合はリリースしない(修正してからやり直す)
