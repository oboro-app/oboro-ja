# リリース運用手順

jaio のリリースのビルド・配布・更新サイクルを定めます。

## リリース判定(変更ベース・機械的)

**日数では判断しません。** 前回リリースタグ以降の main のコミットメッセージ
(conventional commits 接頭辞)だけで機械的に判定します:

| 未リリースのコミット | リリース | バージョン |
|---|---|---|
| `feat` を含む | する | マイナー(`0.x.0`) |
| `fix` のみ | する | パッチ(`0.1.x`) |
| `docs` / `chore` / `test` / `refactor` / `style` のみ | しない | —(次の `feat`/`fix` に同乗) |

判定コマンド(出力があればリリースする):

```sh
git fetch --tags
git log $(git describe --tags --abbrev=0)..main --format='%s' | grep -E '^(feat|fix)(\(|:)'
```

- **タイミング**: `feat`/`fix` を含むコミットを push したら、**同じセッション中に**
  リリースする(次のセッションに先送りしない)
- **セキュリティ対応**(依存の脆弱性修正・誤検出で実害を伴う修正等)は `fix` 扱い。
  通常の `fix` よりも優先してできるだけ早くリリースする
- 品質ゲートが失敗する場合はリリースしない(修正してからやり直す)

| タイミング | 作業 | 実行者 |
|---|---|---|
| `feat`/`fix` を push したセッション | `scripts/release.sh` を実行 | 当番エージェント(下記) |
| `docs` 等のみのセッション | 何もしない | — |
| hotfix(必要なら) | `scripts/release.sh <バージョン>` | 任意のエージェント |

### リリースの手順

```sh
# 1. main が最新であることを確認
git pull origin main

# 2. バージョンを決定(上の表)。Cargo.toml の workspace version を更新してコミット

# 3. bump コミットを push する(先に push しないと gh release create が
#    リモート main の旧コミットにタグを作る、#78)
git push origin main

# 4. リリーススクリプトを実行(品質ゲート → ビルド → タグ → アップロード)
./scripts/release.sh

# 5. push(タグも含めて)
git push origin main --tags

# 6. GitHub Release を確認
gh release view v<version> -R oboro-app/oboro-ja
```

### タグ未リリースの補填(気づき駆動)

タグ push をトリガーとする自動添付は行いません(AGENTS.md の CI・GitHub Actions 禁止)。
代わりに、**タグに対応する GitHub Release が存在しないことに気づいた時点で補填**します:

```sh
# 1. タグとリリースの差分を確認
git fetch --tags
comm -23 <(git tag -l | sort) <(gh release list -R oboro-app/oboro-ja --json tagName --jq '.[].tagName' | sort)

# 2. リリースがないタグがあれば、そのタグ時点のコードで補填リリース
git checkout v<version>
./scripts/release.sh <version>          # 品質ゲート → ビルド → アップロード(既存タグは使い回す)
git checkout main
# タグが未 push の場合は push も行う(release.sh はローカルタグを作るだけ)
git push origin v<version>

# 3. ローカルに作業中の変更がある場合は checkout せず、worktree で分離する
```

補填のタイミングの例: リリース取得を試みて 404 になったとき、`gh release list` を
見て欠落に気づいたとき、定期的な棚卸しのとき。予防的なチェックは不要です
(気づいたエージェント/人間がその場で埋める運用)。

### バージョニング

- **パッチ**(`0.1.x`): バグ修正・ドキュメント整備
- **マイナー**(`0.x.0`): ルール追加・出力形式の追加・設定キーの追加
- **メジャー**(`x.0.0`): schema_version の更新・既存キーの削除・意味変更

バージョンは `Cargo.toml` の `workspace.version` を更新してから
`scripts/release.sh`(引数なし)を実行する(自動で Cargo.toml から読む)。

## リリーススクリプト

`scripts/release.sh` の詳細:

| オプション | 動作 |
|---|---|
| (引数なし) | Cargo.toml のバージョンでリリース。辞書なしバイナリ + 辞書パッケージ |
| `<version>` | バージョンを明示指定 |
| `--with-dict` | 辞書同梱版(600MB、即実行可)も追加 |

スクリプト内部の流れ:
1. `gh auth status` でアカウント確認(リリース権限のあるアカウントだけを使う)
2. `./scripts/check-all.sh` で品質ゲート
3. 埋め込みビルドで辞書成果物を確保 → `package-dict.sh` で tar.gz 化
4. `--no-default-features` で軽量バイナリ(20MB)をビルド
5. git タグ `v<version>` を作成
6. `gh release create` でアップロード

## 配布物

| ファイル | サイズ | 用途 |
|---|---|---|
| `jaio-v<ver>-<os>-<arch>.tar.gz` | ~20MB | 辞書なしバイナリ。`rules`/`explain` はそのまま動作 |
| `jaio-dict-<os>-<arch>.tar.gz` | ~128MB | Sudachi 辞書(Apache-2.0、`.sha256` 付き) |
| `jaio-full-v<ver>-<os>-<arch>.tar.gz` | ~600MB | 辞書同梱版(`--with-dict` 時のみ) |

## 取得方法

```sh
# 最新リリースの取得
gh release download -R oboro-app/oboro-ja

# 特定バージョン
gh release download v0.1.0 -R oboro-app/oboro-ja

# 展開して実行
tar xzf jaio-v*-*.tar.gz && tar xzf jaio-dict-*.tar.gz
./jaio check . --config jaio.toml
```

`check` の実行には辞書ディレクトリの指定が必須:

```toml
# jaio.toml
[analysis]
dictionary = "/path/to/lindera-sudachidict"
```

## リリース担当(エージェント運用)

リリースは `/release` コマンド(または `scripts/release.sh` の直接実行)で行います。
エージェントが把握する手段:

- **手順の正本はこのファイル**(`docs/release.md`)
- `/release` コマンド(`.opencode/commands/release.md`)はこのファイルの薄いラッパ
- `feat`/`fix` を含むコミットを push したエージェントが、同じセッション中に
  リリースする

**時限式で勝手にリリースは走りません。**変更内容(コミットの接頭辞)だけで
機械的に判断します。

### 未リリース変更の確認

```sh
git fetch --tags
# 出力があればリリースする(feat あり→マイナー、fix のみ→パッチ)
git log $(git describe --tags --abbrev=0)..main --format='%s' | grep -E '^(feat|fix)(\(|:)'
```

AGENTS.md の「リリースの確認(変更ベース)」に、コミット前の判定手順を記載しています。

## トラブルシュート

| 症状 | 対処 |
|---|---|
| `gh release create` が 422(タグ重複) | `git tag -d v<ver> && git push origin :refs/tags/v<ver>` で削除してやり直す |
| 辞書成果物が見つからない | `cargo build --release --workspace` を先に実行(埋め込みビルドが必要) |
| pre-push ゲートが fmt で失敗 | `cargo fmt --all` → commit → push をやり直す |
| no-dict バイナリで check が失敗 | `[analysis] dictionary` の指定漏れ。`jaio rules` は辞書なしで動作する |
