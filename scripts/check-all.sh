#!/usr/bin/env bash
# 品質ゲート一括実行。このリポジトリのローカル検証の正本。
# CI(GitHub Actions)は同じ cargo 検査(fmt / clippy / test)と各 OS build を
# 別途回す(.github/workflows/ci.yml)。ここにはローカル専用の検査
# (jaio 自己適用、スクリプト構文)も含む。
# 使い方: ./scripts/check-all.sh
set -euo pipefail

cd "$(dirname "$0")/.."

echo "== [1/5] cargo fmt =="
cargo fmt --all --check

echo "== [2/5] cargo clippy =="
cargo clippy --workspace --all-targets -- -D warnings
# 既定(辞書を埋め込まない)の構成も型検査する。clippy と test は
# dev-dependencies が embedded-dict を有効にするため、既定の構成を通らない
cargo check -q -p jaio-cli

echo "== [3/5] cargo test =="
cargo test --workspace

echo "== [4/5] jaio 自己適用(常設ドッグフーディング、#71)=="
# このリポジトリの文書を jaio 自身で解析する。error 重大度の診断、または
# 実行エラー(解析未完了を含む exit 2)で落とす。warn / info は許容
# (将来のノイズが増えた場合は jaio.toml の [lint.rule_excludes] 等で管理する。
# 抑制仕組みの設計は #71)。既定のビルドは辞書を持たないため、辞書を
# 埋め込んだ CLI で解析する(設計書 §6.5)
cargo run -q -p jaio-cli --features embedded-dict -- check . --quiet

echo "== [5/5] スクリプト構文チェック =="
for f in scripts/*.sh .githooks/pre-commit .githooks/pre-push; do
  bash -n "$f"
done
# python は AST 解析で構文のみ確認(.pyc を生成しない)
for f in scripts/*.py; do
  python3 -c "import ast,sys; ast.parse(open(sys.argv[1],encoding='utf-8').read())" "$f"
done
# 評価器の振る舞いテスト(#35/#62/#70)。バイナリ不要・標準ライブラリのみ
python3 scripts/test_eval_detection.py
python3 scripts/test_eval_rules.py
python3 scripts/test_eval_field.py

echo "全チェック合格"
