#!/usr/bin/env bash
# 観察系 Issue(#2 dir-mismatch 閾値、#8 terminology ペア、#12 大規模性能)の
# 定期再計測を 1 コマンドで実行する。結果を各 Issue に貼る運用。
#
# 使い方: JAIO_BIN=<jaioバイナリ> ./scripts/observe.sh <repo>...
#   JAIO_BIN の既定は target/release/jaio(なければ debug にフォールバック)
set -euo pipefail

cd "$(dirname "$0")/.."

BIN="${JAIO_BIN:-target/release/jaio}"
[ -x "$BIN" ] || BIN=target/debug/jaio
[ -x "$BIN" ] || { echo "error: jaio バイナリがない(cargo build -p jaio-cli)"; exit 2; }

# 観察対象リポジトリは引数で渡す (公開リポジトリに対象の名前を残さない)
REPOS=("$@")
[ "${#REPOS[@]}" -gt 0 ] || { echo "error: 観察対象のリポジトリを引数で渡す"; exit 2; }

echo "# jaio 観察再計測 $(date '+%Y-%m-%d %H:%M')"
echo "binary: $BIN($("$BIN" --version 2>/dev/null || echo unknown))"
echo

echo "## リポジトリ別 診断サマリ(#2/#8 の観察データ)"
printf '%-22s %8s %8s %6s %6s %6s\n' repository md errors warns infos dir-mis term
for repo in "${REPOS[@]}"; do
  [ -d "$repo" ] || continue
  name=$(basename "$repo")
  # 集計行はサマリ末尾に固定されない(top_rules 等が後続する)ため全体から拾う
  out=$(cd "$repo" && "$OLDPWD/$BIN" check . 2>/dev/null)
  checked=$(echo "$out" | grep -oE 'checked [0-9]+ documents' | grep -oE '[0-9]+' | head -1 || echo 0)
  counts=$(echo "$out" | grep -oE '[0-9]+ errors, [0-9]+ warnings, [0-9]+ infos' | head -1 || echo "0 errors, 0 warnings, 0 infos")
  e=$(echo "$counts" | awk '{print $1}')
  w=$(echo "$counts" | awk '{print $3}')
  i=$(echo "$counts" | awk '{print $5}')
  dm=$(cd "$repo" && "$OLDPWD/$BIN" check . 2>/dev/null | grep -c 'dir-mismatch' || true)
  tm=$(cd "$repo" && "$OLDPWD/$BIN" check . 2>/dev/null | grep -c 'terminology' || true)
  printf '%-22s %8s %8s %6s %6s %6s %6s\n' "$name" "$checked" "$e" "$w" "$i" "$dm" "$tm"
done
echo "(dir-mis = dir-mismatch 検出数、term = terminology 検出数)"

echo
echo "## 性能(#12 の観察データ)"
for repo in "${REPOS[@]}"; do
  [ -d "$repo" ] || continue
  n=$(cd "$repo" && "$OLDPWD/$BIN" check . 2>/dev/null | grep -oE 'checked [0-9]+' | grep -oE '[0-9]+')
  [ "${n:-0}" -lt 100 ] && continue
  echo "- $(basename "$repo")(${n} md):"
  ( cd "$repo" && /usr/bin/time -p "$OLDPWD/$BIN" check . >/dev/null 2>/tmp/jaio-time.txt || true
    awk '/^real/{printf "  real %ss", $2} /^user/{printf " / user %ss", $2} END{print ""}' /tmp/jaio-time.txt )
done
echo
echo "この出力を該当 Issue(#2/#8/#12)にコメントとして貼る"
