#!/bin/sh
# lindera 形式の Sudachi 辞書ディレクトリを配布用 tar.gz にまとめる(Issue #27)
#
# 使い方:
#   scripts/package-dict.sh [辞書ディレクトリ] [-o 出力ファイル]
#
# 辞書ディレクトリを省略した場合は埋め込みビルドの成果物
# (target/<profile>/build/lindera-sudachidict-*/out/lindera-sudachidict)を
# 新しい順に探して使う。lindera-cli で自前ビルドした辞書にもそのまま使える。
#
# 配布時の注意: SudachiDict は Apache License 2.0。配布物にはライセンス条文と
# 著作権表示(Works Applications)を同梱すること。
# https://github.com/WorksApplications/SudachiDict/blob/develop/LICENSE-2.0.txt
set -eu

usage() {
    echo "usage: $0 [dict-dir] [-o output.tar.gz]" >&2
    exit 2
}

DICT_DIR=""
OUT=""
while [ $# -gt 0 ]; do
    case "$1" in
        -o)
            [ $# -ge 2 ] || usage
            OUT="$2"
            shift 2
            ;;
        -h | --help) usage ;;
        *)
            [ -z "$DICT_DIR" ] || usage
            DICT_DIR="$1"
            shift
            ;;
    esac
done

# 辞書ディレクトリの自動検出(新しいビルド成果物を優先)
if [ -z "$DICT_DIR" ]; then
    for d in $(ls -dt target/*/build/lindera-sudachidict-*/out/lindera-sudachidict 2>/dev/null); do
        DICT_DIR="$d"
        break
    done
fi
if [ -z "$DICT_DIR" ] || [ ! -d "$DICT_DIR" ]; then
    echo "error: 辞書ディレクトリが見つかりません。lindera-cli でビルドするか、埋め込みビルドの成果物を指定してください" >&2
    exit 1
fi

# lindera 辞書の必須ファイルを検査する
missing=""
for f in dict.trie dict.vals dict.valsidx dict.words dict.wordsidx matrix.mtx char_def.bin unk.bin metadata.json; do
    [ -f "$DICT_DIR/$f" ] || missing="$missing $f"
done
if [ -n "$missing" ]; then
    echo "error: $DICT_DIR は lindera 辞書ではありません(欠落:$missing)" >&2
    exit 1
fi

VERSION=$(sed -n 's/.*"version"[": ]*\([0-9]*\).*/\1/p' "$DICT_DIR/metadata.json" | head -1)
[ -n "$OUT" ] || OUT="lindera-sudachidict${VERSION:+-$VERSION}.tar.gz"

tar -czf "$OUT" -C "$(dirname "$DICT_DIR")" "$(basename "$DICT_DIR")"
if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$OUT" >"$OUT.sha256"
elif command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$OUT" >"$OUT.sha256"
fi

echo "packaged: $DICT_DIR -> $OUT"
[ -f "$OUT.sha256" ] && echo "checksum: $OUT.sha256"
echo "note: 配布時は SudachiDict の Apache-2.0 ライセンス条文・著作権表示を同梱してください"
