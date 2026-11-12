#!/bin/sh
# プライベートリリースのビルド・タグ付け・アップロードを 1 コマンドで行う。
#
# 使い方:
#   scripts/release.sh [バージョン] [--with-dict]
#
#   バージョンを省略すると Cargo.toml の workspace version を使う。
#   --with-dict を付けると埋め込み辞書バイナリ(+565MB)も同梱する。
#   GitHub Releases にプライベートリリース(リポジトリが private なので
#   アクセス権限のあるメンバーのみ取得可能)を作成する。
#
# 生成物(既定):
#   jaio-v<version>-<os>-<arch>.tar.gz       — 辞書なしバイナリ(20MB 程度)
#   jaio-dict-<os>-<arch>.tar.gz            — Sudachi 辞書(別配布、Apache-2.0)
#
# 生成物(--with-dict):
#   上記に加えて jaio-full-v<version>-<os>-<arch>.tar.gz(辞書同梱、即実行可)
#
# CI 不使用運用(AGENTS.md): GitHub Actions は使わず、ローカルでビルドして
# gh CLI でアップロードする。実行者のマシンがビルド環境となる。
#
# バイナリの取得(メンバー):
#   gh release download v<version> -R oboro-app/oboro-ja
#   # 辞書なし版 + 辞書
#   tar xzf jaio-v*-*.tar.gz && tar xzf jaio-dict-*.tar.gz
#   # または full 版(辞書同梱)
#   tar xzf jaio-full-*.tar.gz
set -euo pipefail

REPO="oboro-app/oboro-ja"
cd "$(dirname "$0")/.."

# GitHub アカウントの確認(リリース権限のあるアカウントだけを使う)
ACTIVE=$(gh auth status 2>&1 | grep "Active account: true" | head -1 || true)
if [ -z "$ACTIVE" ]; then
    echo "error: gh にログインしていません。gh auth login してください。" >&2
    exit 2
fi

# オプション解析
WITH_DICT=""
VERSION=""
HAS_VERSION_ARG=""
for arg in "$@"; do
    case "$arg" in
        --with-dict) WITH_DICT=1 ;;
        --*) echo "error: 未知のオプションです: $arg(--with-dict のみ対応)" >&2; exit 2 ;;
        *) VERSION="$arg"; HAS_VERSION_ARG=1 ;;
    esac
done

# バージョン決定
if [ -z "$VERSION" ]; then
    VERSION=$(grep '^version' Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/')
fi
echo "リリースバージョン: v${VERSION}"

# 指定バージョンと Cargo.toml の一致(#78: アセットの版と Cargo 版のずれ防止)
if [ -n "$HAS_VERSION_ARG" ]; then
    CARGO_VERSION=$(grep '^version' Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/')
    if [ "$VERSION" != "$CARGO_VERSION" ]; then
        echo "error: 指定バージョン v${VERSION} と Cargo.toml の version ${CARGO_VERSION} が一致しません。Cargo.toml を更新してコミットしてから実行してください。" >&2
        exit 1
    fi
fi

# 作業ツリーの検証(#78: 未コミット変更はビルドに混入するがタグには記録されない)
if [ -n "$(git status --porcelain)" ]; then
    echo "error: 作業ツリーに未コミットの変更があります。コミットしてから実行してください(#78)。" >&2
    exit 1
fi

# 品質ゲート
echo "== [1/5] 品質ゲート =="
./scripts/check-all.sh

# 辞書成果物の確保(embedded ビルドが必要)
echo "== [2/5] 辞書成果物の生成 =="
cargo build --release --workspace -q
DICT_DIR=$(ls -d target/release/build/lindera-sudachidict-*/out/lindera-sudachidict 2>/dev/null | head -1)
if [ -z "$DICT_DIR" ]; then
    echo "warning: 辞書成果物が見つかりません。辞書なしバイナリのみリリースします。" >&2
fi

# リリースバイナリのビルド
echo "== [3/5] リリースビルド =="
cargo build --release --workspace --no-default-features -q
BIN="target/release/jaio"
[ -x "$BIN" ] || { echo "error: バイナリが生成されていません" >&2; exit 2; }

# アーキテクチャ情報
OS_ARCH="$(uname -s)-$(uname -m)"
echo "対象: ${OS_ARCH}"

# アセットの準備
TAR_DIR="target/release-dist"
rm -rf "$TAR_DIR"
mkdir -p "$TAR_DIR"
ASSETS=""

# 辞書なしバイナリ
BIN_TAR="jaio-v${VERSION}-${OS_ARCH}.tar.gz"
tar -czf "${TAR_DIR}/${BIN_TAR}" -C target/release jaio
echo "バイナリ: ${TAR_DIR}/${BIN_TAR} ($(du -h "${TAR_DIR}/${BIN_TAR}" | cut -f1))"
ASSETS="${ASSETS} ${TAR_DIR}/${BIN_TAR}"

# 辞書パッケージ
if [ -n "$DICT_DIR" ]; then
    DICT_TAR="jaio-dict-${OS_ARCH}.tar.gz"
    if ./scripts/package-dict.sh "$DICT_DIR" -o "${TAR_DIR}/${DICT_TAR}"; then
        echo "辞書: ${TAR_DIR}/${DICT_TAR} ($(du -h "${TAR_DIR}/${DICT_TAR}" | cut -f1))"
        ASSETS="${ASSETS} ${TAR_DIR}/${DICT_TAR}"
        # package-dict.sh が .sha256 も出すので同梱
        [ -f "${TAR_DIR}/${DICT_TAR}.sha256" ] && ASSETS="${ASSETS} ${TAR_DIR}/${DICT_TAR}.sha256"
    else
        # 辞書 asset の欠落を静かに通さない(#108)。利用者が辞書を取得できなくなる
        echo "warning: 辞書のパッケージングに失敗しました。辞書 asset 無しでリリースされます" >&2
    fi
fi

# 辞書同梱版(--with-dict)
if [ -n "$WITH_DICT" ]; then
    FULL_TAR="jaio-full-v${VERSION}-${OS_ARCH}.tar.gz"
    cargo build --release --workspace -q
    tar -czf "${TAR_DIR}/${FULL_TAR}" -C target/release jaio
    echo "フル版: ${TAR_DIR}/${FULL_TAR} ($(du -h "${TAR_DIR}/${FULL_TAR}" | cut -f1))"
    ASSETS="${ASSETS} ${TAR_DIR}/${FULL_TAR}"
fi

# リリースノート
# リリースノート(with-dict の注記を組み立て)
WITH_DICT_NOTE=""
if [ -n "$WITH_DICT" ]; then
    WITH_DICT_NOTE="| \`jaio-full-v${VERSION}-${OS_ARCH}.tar.gz\` | 辞書同梱バイナリ(即実行可、600MB 程度) |
"
fi

NOTES=$(cat <<EOF
## jaio v${VERSION}

読み取り専用の日本語文書静的解析ツール(schema v2、#31)。

### 含物

| ファイル | 内容 |
|---|---|
| \`jaio-v${VERSION}-${OS_ARCH}.tar.gz\` | 辞書なしバイナリ(20MB 程度。\`rules\`/\`explain\` はそのまま動作) |
| \`jaio-dict-${OS_ARCH}.tar.gz\` | Sudachi 辞書(check/stats/dump に必要、Apache-2.0) |
${WITH_DICT_NOTE}### インストール(辞書なし + 辞書)

\`\`\`sh
gh release download v${VERSION} -R ${REPO}
tar xzf jaio-v${VERSION}-*.tar.gz
tar xzf jaio-dict-*.tar.gz
sudo mv jaio /usr/local/bin/
\`\`\`

\`check\` の実行には辞書ディレクトリが必要です。\`jaio.toml\` で指定:
\`\`\`toml
[analysis]
dictionary = "/path/to/lindera-sudachidict"
\`\`\`

または環境変数:
\`\`\`sh
JAIO_CONFIG='[analysis]
dictionary = "/path/to/lindera-sudachidict"' jaio check .
\`\`\`

\`jaio rules\` / \`jaio explain\` は辞書なしで動作します。
EOF
)

# タグ作成(既に存在する場合はスキップ)。タグを付ける HEAD がリモートに
# push 済みであることを先に検証する(#78): gh release create はリモートに
# 無いタグをリモート main の先頭に作るため、未 push の bump コミットで
# 実行すると「タグが旧バージョンのソースを指す」不整合が起きる
echo "== [4/5] タグ付け =="
git fetch origin main --quiet
# ① HEAD と origin/main が同一コミット(祖先チェックだとローカルが古い
# 場合も通ってしまい、gh release create がリモート main の旧先頭へタグを
# 作る。#78)。② 作業ツリーがクリーン(未コミット変更はビルドに混入するが
# タグには記録されない)
if [ "$(git rev-parse HEAD)" != "$(git rev-parse origin/main)" ]; then
    echo "error: HEAD と origin/main が異なります。bump コミットを git push origin main で push してから実行してください(#78)。" >&2
    exit 1
fi
if git rev-parse "v${VERSION}" >/dev/null 2>&1; then
    echo "タグ v${VERSION} は既に存在します(使い回します)"
else
    git tag "v${VERSION}"
fi
# gh release create はリモートにタグが無い場合にリモート main の先頭へタグを
# 作るため、タグを先に push する(#78)
git push origin "v${VERSION}" --quiet
REMOTE_TAG_SHA=$(gh api "repos/${REPO}/git/ref/tags/v${VERSION}" --jq '.object.sha')
LOCAL_TAG_SHA=$(git rev-parse "v${VERSION}^{commit}")
if [ "$REMOTE_TAG_SHA" != "$LOCAL_TAG_SHA" ]; then
    echo "error: リモートタグ v${VERSION} の SHA がローカルと一致しません(#78)。" >&2
    echo "  remote: $REMOTE_TAG_SHA / local: $LOCAL_TAG_SHA" >&2
    exit 1
fi

# GitHub Release 作成・アップロード
echo "== [5/5] アップロード =="
gh release create "v${VERSION}" $ASSETS \
    --repo "$REPO" \
    --title "jaio v${VERSION}" \
    --notes "$NOTES" \
    --latest 2>&1 || {
    echo "error: リリース作成に失敗しました。既にタグが push されている場合は" >&2
    echo "  gh release delete v${VERSION} -R $REPO && git tag -d v${VERSION}" >&2
    echo "  でやり直せます。" >&2
    exit 2
}

echo
echo "リリース完了: https://github.com/${REPO}/releases/tag/v${VERSION}"
echo
echo "取得方法(メンバー):"
echo "  gh release download v${VERSION} -R ${REPO}"
