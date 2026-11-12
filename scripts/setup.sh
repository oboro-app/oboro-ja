#!/usr/bin/env bash
# 初回セットアップ。git hooks を有効化する。冪等なので何度実行してもよい。
# 使い方: ./scripts/setup.sh
set -euo pipefail

cd "$(dirname "$0")/.."

git config core.hooksPath .githooks
echo "git hooks 有効化: core.hooksPath=.githooks"

# 環境の確認(参考情報。失敗はしない)
echo "rustc: $(rustc --version)"
echo "cargo: $(cargo --version)"
