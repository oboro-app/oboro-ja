# jaio - コーディングエージェントガイド

## プロジェクト概要

リポジトリ内の日本語文書を静的解析し、**人間の可読性**と**AI のトークン効率**の両面から
冗長性・揺れを検出する Rust 製 CLI ツール。「文書版 clippy」を目指す。

- jaio は**決定論的な静的解析のみ**を行う。意味判断(統合すべきか、どう書き直すか)は LLM/人間に委譲し、判断材料の構造化出力(JSON)を提供する
- 設計の正本は `docs/design.md`。実装は Phase 1 から順に進める

## 技術スタック

| レイヤ | 技術 |
|---|---|
| 言語 | Rust(edition 2024、Cargo workspace) |
| Markdown 解析 | `comrak` |
| 形態素解析 | `lindera`(Sudachi 辞書) |
| トークン計測 | `tiktoken-rs` |
| コメント抽出 | `tree-sitter`(feature flag で段階提供) |
| ファイル走査 | `ignore` |
| 参照集合 | `HashSet`/`HashMap` |
| 並列処理 | `rayon` |
| CLI | `clap`(derive API) |
| 直列化 | `serde` / `serde_json` |

## 必須コマンド

```bash
# --- 初回セットアップ(git hooks 有効化、冪等) ---
./scripts/setup.sh

# --- 開発 ---
cargo run -p jaio-cli                     # CLI 実行
cargo build --workspace                   # ビルド

# --- Lint・型チェック ---
cargo fmt --all --check                   # フォーマット検査
cargo clippy --workspace --all-targets -- -D warnings

# --- テスト ---
cargo test --workspace

# --- 全チェック一括実行(品質ゲートの正) ---
./scripts/check-all.sh                    # fmt + clippy + test + jaio 自己適用 + CI禁止ガード

# --- リリース(プライベート GitHub Releases) ---
./scripts/release.sh                      # 品質ゲート → ビルド → タグ → アップロード
./scripts/release.sh --with-dict          # 辞書同梱版も追加
./scripts/release.sh 0.2.0                # バージョン指定

# --- 検出感度パラメータの調整・検証 ---
./scripts/eval-detection.py               # グラウンドトゥルース付き合成コーパスで再現率・偽陽性を計測(文書ペア重複)
./scripts/eval-rules.py                   # 同上(文・段落レベルの効率・揺れ系ルール、#62)
./scripts/eval-field.py --repo <path> --name <name>   # 実リポジトリの生ペアをベースライン照合(実分布の回帰、#70)
./scripts/eval-detection.py --grid 0.6,0.7,0.75,0.85  # min_similarity を縦断比較
# チューニング手順: docs/design.md §8 の [detection]/[merge] を jaio.toml で調整し、
# 上記スクリプトと実リポジトリ試用 (結果は公開しない)で効果を確認する
```

## 検証運用

- **CI と GitHub Actions は使用しない**。`.github/workflows` へのファイル追加は禁止。
  `./scripts/check-all.sh` が workflows の存在を検出して失敗させる。
- 品質ゲートはローカル環境での `./scripts/check-all.sh` を正とする。
  ゲートには **jaio 自己適用**(このリポジトリの文書を jaio で解析、error 重大度
  のみで落とす常設ドッグフーディング、#71)が含まれる。
- コミット前(pre-commit)に fmt + clippy(Rust 関連のステージがある場合)、
  プッシュ前(pre-push)に check-all 全体が自動実行される。
- この CI 禁止は **jaio 自身のリポジトリ開発運用**についてのルール。
  jaio を他リポジトリの CI から利用するための機能(SARIF 出力等、設計書 Phase 3)は対象外。

## プロジェクト構成

```
jaio/
├── AGENTS.md               # このファイル。指示の正本(OpenCode2 / Codex 共通)
├── docs/
│   ├── README.md           # ドキュメント読み順・規約
│   └── design.md           # 設計の正本
├── crates/
│   ├── jaio-core/          # 診断モデル・設定・ルール trait(依存最小)
│   ├── jaio-collect/       # ファイル走査・コメント抽出(Phase 2 で追加)
│   ├── jaio-parse/         # 文書構造 AST・日本語 AST(Phase 1 で追加)
│   ├── jaio-index/         # 転置索引・SimHash・リンクグラフ(Phase 2 で追加)
│   ├── jaio-rules/         # ルール実装群(Phase 1 で追加)
│   ├── jaio-report/        # 出力 terminal / json / md / sarif(Phase 1 で追加)
│   └── jaio-cli/           # バイナリ(clap)
├── scripts/
│   ├── check-all.sh        # 品質ゲート一括実行
│   └── setup.sh            # 初回セットアップ
├── .githooks/              # pre-commit / pre-push
├── .agents/skills/         # 共有スキル(OpenCode2・Codex 両方が読む)
├── .opencode/commands/     # OpenCode2 用スラッシュコマンド(/check, /handover)
└── logs/                   # ログ置き場(git 管理外)
```

依存の方向は一方向のみ(`設計書 §6.3`): `cli → report/rules/index/parse/collect → core`。
循環依存を許さない。crate 追加時はこの構成に従う。

## コーディング規約

- **コメント**: 日本語で書く。自明なコードにはコメントを追加せず、設計意図がある場合のみ記述する
- **変数・関数名**: 英語(国際標準)
- **文字列・パス処理の注意**(実装レビュー #66〜#68/#73/#76/#79 の教訓):
  Markdown 構文(リンク・見出し・コード)を扱う処理は comrak の構文範囲
  (sourcepos)に限定し、生テキストへの正規化を書かない。パターン照合
  (gitignore 構文)とパス解決は `jaio_core::relativize_for_match`
  (jaio.toml 基準 → cwd の多段相対化)を使い、cwd 単独基準を書かない
- **フォーマット**: `cargo fmt`(`rustfmt.toml` の `style_edition = "2024"`)を正とする。
  エディタや他ツールの import 自動整理に任せず、コミット前に `cargo fmt --all` を通す
- **emoji**: 使用しない
- **ログ**: ログファイルはリポジトリ TOP の `logs/` に配置する
- **会話**: 日本語で行う

## ドキュメント運用

| 知りたいこと | 参照先 |
|---|---|
| 設計全体 | `docs/design.md` |
| 読み順・文書規約 | `docs/README.md` |
| 未解決事項・検証待ち | GitHub Issues(`gh issue list -R oboro-app/oboro-ja`) |

- 設計判断を変える変更は、実装前に `docs/design.md` を更新する
- 作業進捗・実装済み/未実装の一覧を `docs/` に記録しない。未解決事項は GitHub Issues で管理する(ローカルファイルでは管理しない)。
  ただし設計書ロードマップのチェックボックス(完了マーク)は設計の状態表現として許可する

## 変更フロー

コード・文書を変更するときの標準手順。スキル `rust-change-flow` はこの手順のラッパ。

1. 対象ファイルと既存差分(`git diff`)を確認してから変更する
2. 設計と矛盾する変更が必要な場合は、先に `docs/design.md` を更新する
3. 変更後は `./scripts/check-all.sh` を実行して全項目合格を確認する
4. 未解決事項・検証待ちが残る場合は GitHub Issue を起票し、Issue 番号をコミットメッセージ等に残す。この変更で解消した項目があれば該当 Issue をクローズする
5. コミットメッセージは変更の意図を 1 行で書く(conventional commits 風: feat / fix / docs / chore / test / refactor)

禁止事項:

- 作業進捗を実装コードや `docs/` に書かない(未解決事項は GitHub Issues だけに置く)
- `.github/workflows` を追加しない
- 無関係なリファクタリングやフォーマット変更を混ぜない

## ハーネス構成(エージェント運用)

このリポジトリは **OpenCode V2(`opencode2`)と Codex CLI の両方**で運用する。

- 指示の正本はこの `AGENTS.md`。両ツールともリポジトリルートの `AGENTS.md` を自動読込する
- **共有スキルは `.agents/skills/`** に置く。OpenCode2(互換ソース)と Codex(REPO スコープ)が
  どちらもこのディレクトリを読む。スキルは薄いラッパとし、手順の正本は `scripts/` とこのファイルが持つ
- OpenCode2 のプロジェクト設定は `opencode.json`(モデル固定、`git push` は ask、`.github/workflows` 編集は deny)
- 同じ操作を両ツールで呼ぶための対応表:

| 操作 | OpenCode2 | Codex |
|---|---|---|
| 品質ゲート実行 | `/check` コマンド または `check` スキル | `$check` スキル |
| リリース実行 | `/release` コマンド または `release` スキル | `$release` スキル |
| セッション引き継ぎ | `/handover` コマンド または `handover` スキル | `$handover` スキル |
| 変更の標準手順 | `rust-change-flow` スキル | `$rust-change-flow` スキル |

- Codex のカスタムプロンプト(`~/.codex/prompts`)は非推奨・リポジトリ共有不可のため使わない

## 並行運用プロトコル(複数エージェント)

このリポジトリは複数のエージェント(OpenCode2・Codex 等)が同一チェックアウトで作業することがある。

- 書き込みは原則として 1 エージェントずつ。並行する場合は役割を分ける
  (例: 実装 / ハーネス・検証・レビュー)ことを、作業開始前に利用者へ確認する
- `git status` に他者の未コミット変更があるとき、そのファイルへ書き込まない。
  編集が必要なら相手の作業完了を待つ
- コミットやプッシュが index ロック等で失敗した場合は、少し待って再試行する
- pre-push ゲートは**作業ツリー全体**を検証するため、他者の未コミット WIP が原因で
  失敗することがある。プッシュ対象のコミットがコミット時ゲートを通過済みなら
  `git push --no-verify` を使ってよい(利用者への報告でその旨を明示する)
- 衝突が頻発する場合は git worktree でチェックアウトを分離する
  (OpenCode2 の worktree 機能、Codex の Git worktrees)
- リモートは `origin`(github.com/oboro-app/oboro-ja、公開)。push はバックアップを兼ねる

## セッション引き継ぎ

作業を中断・引き継ぐときは `/handover` コマンド(OpenCode2)または `$handover` スキル(Codex)で
「現在の状態・完了作業・残作業・注意点」を整理する。残作業は GitHub Issue として起票・更新する。

## リリースの確認(変更ベース・機械的)

リリースは**日数ではなく変更内容で機械的に判断**する(正本は `docs/release.md`)。
`feat`/`fix` を含むコミットを push したら、**同じセッション中に**リリースする:

```sh
git fetch --tags
# 出力があればリリース(feat あり→マイナー、fix のみ→パッチ)
git log $(git describe --tags --abbrev=0)..main --format='%s' | grep -E '^(feat|fix)(\(|:)'
```

`docs`/`chore`/`test`/`refactor`/`style` のみのセッションではリリースしない
(次の `feat`/`fix` に同乗)。品質ゲートが失敗する場合もリリースしない。
