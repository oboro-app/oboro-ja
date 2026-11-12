#!/usr/bin/env python3
"""実リポジトリ回帰評価(#70 方向性 2、2026-09-17 レビュー対応版)。

合成コーパス(eval-detection.py)では再現できない実分布の検出挙動を、
実リポジトリの**生の重複ペア**(`dump corpus` の duplicates。check の表示
上限後の診断ではない)と解析対象集合のベースライン照合で検証する。
実ファイルは jaio リポジトリに含めず、解析対象のパスはコマンドラインで渡す
(ベースラインにはリポジトリルートからの相対パスだけを記録する)。

照合の分離(#70 の受入条件):
  - 固定条件(入力側): 解析対象 docs のパス集合・走査状態・候補検索方式。
    不一致は「入力または条件の変化」として失敗する
  - 比較変数(実装側): config_fingerprint・algorithms(辞書等)。不一致は
    変更の表示のみで失敗にしない(実装変更は意図した変数)
  - 検出結果: duplicates ペア集合。消失・新規は失敗。良否は内容で判断し、
    意図的な変更なら --update-baseline で更新する

使い方:

    ./scripts/eval-field.py --repo /path/to/repo --name sample \
        --set analysis.dictionary=/path/to/lindera-sudachidict

    # ベースラインの登録・更新(明示的にのみ書き込む)
    ./scripts/eval-field.py --repo ... --name sample --update-baseline [...]

終了コード: 0 = ベースラインと一致、1 = 差分あり、2 = 実行・入力エラー
"""

from __future__ import annotations

import argparse
import datetime
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
BASELINE_DIR = REPO / "scripts" / "field-baselines"

# dump corpus の出力契約(表示上限前の生ペアを検証するため check ではなく
# dump corpus を使う、#70)
KIND = "jaio/dump-corpus"
SUPPORTED_VERSION = 2

# 実行条件を固定する既定上書き(隠しディレクトリを含める。実運用の主要文書が
# .agents 配下にあるため、実運用試用の実績に合わせる)
DEFAULT_SETS = {"paths.include_hidden": "true"}


class EvalError(Exception):
    """入力検証・実行の失敗(終了コード 2 に対応)"""


def toml_literal(value: str) -> str:
    """--set の値を TOML として安全に出力する(eval-* と同じ規則)"""
    import tomllib

    try:
        tomllib.loads(f"k = {value}")
    except tomllib.TOMLDecodeError:
        return json.dumps(value, ensure_ascii=False)
    return value


def build_config(sets: list[str]) -> str:
    import tomllib

    merged: dict[str, str] = dict(DEFAULT_SETS)
    for item in sets:
        key, _, value = item.partition("=")
        section, _, name = key.strip().partition(".")
        if not section or not name:
            raise EvalError(f"--set のキーは section.key 形式で指定してください: {key}")
        merged[f"{section}.{name}"] = value.strip()
    # セクションごとにまとめて有効な TOML を組む
    by_section: dict[str, list[str]] = {}
    for key, value in sorted(merged.items()):
        section, _, name = key.partition(".")
        by_section.setdefault(section, []).append(f"{name} = {toml_literal(value)}")
    text = "\n".join(f"[{s}]\n" + "\n".join(kv) for s, kv in by_section.items()) + "\n"
    try:
        tomllib.loads(text)
    except tomllib.TOMLDecodeError as err:
        raise EvalError(f"生成した jaio.toml が不正です: {err}") from err
    return text


def run_dump_corpus(bin_path: str, repo: Path, config_text: str, timeout: int) -> dict:
    """`jaio dump corpus <repo>` を実行して JSON を返す(生ペアの入手源)"""
    with tempfile.TemporaryDirectory() as tmp:
        cfg = Path(tmp) / "jaio.toml"
        cfg.write_text(config_text, encoding="utf-8")
        proc = subprocess.run(
            [bin_path, "dump", "corpus", str(repo), "--config", str(cfg)],
            capture_output=True,
            text=True,
            timeout=timeout,
        )
        if proc.returncode != 0:
            detail = proc.stderr.strip() or proc.stdout[:500]
            raise EvalError(
                f"jaio dump corpus が異常終了しました(exit {proc.returncode}): {detail}"
            )
        try:
            return json.loads(proc.stdout)
        except json.JSONDecodeError as err:
            raise EvalError(
                f"jaio dump corpus の出力が JSON ではありません: {err}"
            ) from err


def repo_relative(path: str, repo: Path) -> str:
    """出力パスを repo 相対の一意 ID に正規化する(cwd・表記系に依存しない)。

    dump corpus の file は解析ルート(= 引数 repo)基準の表記で返るため、
    相対パスは repo 基準で絶対化してから repo の実体パスを剥がす。
    repo 配下でないパスは絶対表記のまま返す(ベースライン不一致として表面化)
    """
    p = Path(path)
    absolute = p if p.is_absolute() else repo / p
    real = Path(os.path.realpath(absolute))
    repo_real = Path(os.path.realpath(repo))
    try:
        return real.relative_to(repo_real).as_posix()
    except ValueError:
        return real.as_posix()


def validate_report(report: dict) -> None:
    """出力契約と解析の完全性を検証する(#70 の受入条件 2)。

    対応外 schema・走査未完了・運用エラー・ルール失敗がある状態は
    ペア集合が同じでも合格にしない
    """
    if report.get("kind") != KIND:
        raise EvalError(
            f"出力の kind が想定外です: {report.get('kind')!r}(期待: {KIND})"
        )
    if report.get("version") != SUPPORTED_VERSION:
        raise EvalError(
            f"出力の version が対応外です: {report.get('version')}(対応: {SUPPORTED_VERSION})"
        )
    ctx = report.get("context", {})
    targets = ctx.get("targets", {})
    if not targets.get("discovery_complete", False):
        raise EvalError("走査が完了していません(targets.discovery_complete = false)")
    if targets.get("skipped", 0):
        raise EvalError(
            f"読み込みに失敗したファイルがあります(skipped={targets['skipped']})"
        )
    errors = ctx.get("operational_errors", [])
    if errors:
        first = errors[0]
        raise EvalError(
            f"運用エラーがあります({len(errors)} 件): {first.get('path')}: {first.get('reason')}"
        )
    failed = [r for r in ctx.get("rules", []) if r.get("status") == "failed"]
    if failed:
        raise EvalError(
            f"失敗したルールがあります: {', '.join(r['rule_id'] for r in failed)}"
        )
    # 必須構造の確認(dump corpus はルールを実行しないため
    # summary.analysis_complete は持たない。完全性は上記 targets/errors と
    # sources 件数で判断する)。欠損を空集合に変換して黙って通さない(#70 残件 2)
    docs = report.get("docs")
    duplicates = report.get("duplicates")
    sources = ctx.get("sources")
    if not isinstance(docs, list) or not isinstance(duplicates, list):
        raise EvalError("docs / duplicates が配列ではありません(dump corpus 契約違反)")
    if docs and not isinstance(sources, list):
        raise EvalError(
            f"docs が {len(docs)} 件あるのに context.sources がありません"
            "(入力 manifest 不完全、#70)"
        )
    algorithms = ctx.get("algorithms") or {}
    for key in ("morphology", "tokenizer", "similarity", "build"):
        if key not in algorithms:
            raise EvalError(f"context.algorithms.{key} がありません(出力契約違反、#36)")
    if "method" not in (ctx.get("candidate_search") or {}):
        raise EvalError(
            "context.candidate_search.method がありません(出力契約違反、#36)"
        )


def extract_state(report: dict, repo: Path) -> dict:
    """ベースライン化する状態(固定条件 + 比較変数 + 検出結果)を抽出する"""
    ctx = report.get("context", {})
    # 入力 manifest: 解析した文書のパス → 内容 hash(#70 残件 1)。同じパスでも
    # 本文が変われば入力変化として検出できるようにする
    source_hashes = {
        repo_relative(item["path"], repo): item["sha256"]
        for item in ctx.get("sources", [])
        if item.get("path") and item.get("sha256")
    }
    docs = sorted(repo_relative(d["file"], repo) for d in report.get("docs", []))
    pairs = sorted(
        tuple(sorted([repo_relative(d["a"], repo), repo_relative(d["b"], repo)]))
        for d in report.get("duplicates", [])
    )
    doc_hashes = []
    for rel in docs:
        sha = source_hashes.get(rel)
        if sha is None:
            raise EvalError(
                f"docs の {rel} に対応する sources[].sha256 がありません"
                "(入力 manifest 不完全、#70)"
            )
        doc_hashes.append([rel, sha])
    doc_hashes.sort()
    cs = ctx.get("candidate_search", {})
    targets = ctx.get("targets", {})
    if len(set(pairs)) != len(pairs):
        raise EvalError("duplicates に同一ペアが複数回出現します(出力契約の疑い)")
    return {
        "fixed": {
            "docs": docs,
            # 本文が変われば入力変化として検出する(#70 残件 1)
            "doc_hashes": doc_hashes,
            "targets": {
                "analyzed": targets.get("analyzed"),
                "discovered": targets.get("discovered"),
                "skipped": targets.get("skipped"),
                "discovery_complete": targets.get("discovery_complete"),
            },
            # 検出範囲を左右する条件。comparable_docs は docs 集合から導かれる
            # 値なので docs 比較で担保し、ここには方式と閾値だけ置く
            "candidate_search": {
                "method": cs.get("method"),
                "cutoff": cs.get("cutoff"),
            },
        },
        "variables": {
            "config_fingerprint": ctx.get("config_fingerprint"),
            "algorithms": ctx.get("algorithms"),
        },
        "pairs": [list(p) for p in pairs],
    }


def load_baseline(path: Path) -> dict:
    """ベースラインを読み込む。未登録・破損・構造欠損は実行エラーにする"""
    if not path.exists():
        try:
            shown = str(path.relative_to(REPO))
        except ValueError:
            shown = str(path)
        raise EvalError(
            f"ベースラインが未登録です: {shown}。"
            "登録は --update-baseline を明示的に指定してください"
        )
    try:
        baseline = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as err:
        raise EvalError(f"ベースラインが壊れています: {err}") from err
    for key in ("fixed", "variables", "pairs"):
        if key not in baseline:
            raise EvalError(f"ベースラインに必須キーがありません: {key}")
    return baseline


def compare(baseline: dict, actual: dict) -> tuple[list[str], bool]:
    """固定条件・検出結果を照合し、メッセージ列と失敗かを返す"""
    messages: list[str] = []
    failed = False

    # 固定条件(入力側): docs 集合・走査状態・検索方式
    b_fixed, a_fixed = baseline["fixed"], actual["fixed"]
    if b_fixed["docs"] != a_fixed["docs"]:
        missing = sorted(set(b_fixed["docs"]) - set(a_fixed["docs"]))
        added = sorted(set(a_fixed["docs"]) - set(b_fixed["docs"]))
        messages.append(
            f"入力変化: 解析対象がベースラインと異なります"
            f"(消失 {len(missing)} / 追加 {len(added)})"
        )
        for m in missing[:5]:
            messages.append(f"  - {m}")
        for a in added[:5]:
            messages.append(f"  + {a}")
        failed = True
    if b_fixed["targets"] != a_fixed["targets"]:
        messages.append(f"走査状態の変化: {b_fixed['targets']} -> {a_fixed['targets']}")
        failed = True
    if b_fixed["doc_hashes"] != a_fixed["doc_hashes"]:
        b_by_path = dict(b_fixed["doc_hashes"])
        a_by_path = dict(a_fixed["doc_hashes"])
        changed = [
            (rel, b_by_path.get(rel), a_by_path.get(rel))
            for rel in sorted(set(b_by_path) | set(a_by_path))
            if b_by_path.get(rel) != a_by_path.get(rel)
        ]
        messages.append(
            f"入力変化: 文書の本文がベースラインと異なります({len(changed)} 件)"
        )
        for rel, old_sha, new_sha in changed[:5]:
            messages.append(f"  ~ {rel}: {old_sha} -> {new_sha}")
        failed = True
    if b_fixed["candidate_search"] != a_fixed["candidate_search"]:
        messages.append(
            f"検索条件の変化: {b_fixed['candidate_search']} -> {a_fixed['candidate_search']}"
        )
        failed = True

    # 検出結果(生ペア)
    expected = {tuple(p) for p in baseline["pairs"]}
    got = {tuple(p) for p in actual["pairs"]}
    missing = expected - got
    added = got - expected
    if missing:
        messages.append(f"消失ペア({len(missing)}): 検出の回帰(偽陰性化)の疑い")
        for a, b in sorted(missing)[:10]:
            messages.append(f"  - {a} <-> {b}")
        failed = True
    if added:
        messages.append(f"新規ペア({len(added)}): 検出挙動の変化(改善 or 回帰)")
        for a, b in sorted(added)[:10]:
            messages.append(f"  + {a} <-> {b}")
        failed = True

    # 比較変数(実装側): 変更の表示のみ(意図した変数。失敗にしない)
    for key in ("config_fingerprint", "algorithms"):
        if baseline["variables"].get(key) != actual["variables"].get(key):
            messages.append(f"参考(失敗扱いにしない): {key} が変化しています")
    return messages, failed


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--repo", required=True, help="解析対象の実リポジトリのパス")
    parser.add_argument(
        "--name",
        required=True,
        help="ベースライン名(scripts/field-baselines/<name>.json)",
    )
    parser.add_argument(
        "--bin",
        default=str(REPO / "target" / "release" / "jaio"),
        help="jaio バイナリのパス",
    )
    parser.add_argument(
        "--set",
        action="append",
        default=[],
        metavar="SECTION.KEY=VALUE",
        help="jaio.toml の上書き(例: analysis.dictionary=/path/to/dict)",
    )
    parser.add_argument(
        "--timeout", type=int, default=600, help="jaio 1 回のタイムアウト秒"
    )
    parser.add_argument(
        "--update-baseline",
        action="store_true",
        help="ベースラインを登録・更新する(固定条件と比較変数を記録)",
    )
    args = parser.parse_args()

    bin_path = Path(args.bin)
    if not bin_path.exists():
        print(f"error: jaio バイナリが見つかりません: {bin_path}", file=sys.stderr)
        print(
            "hint: cargo build --release -p jaio-cli してから再実行してください",
            file=sys.stderr,
        )
        return 2
    repo = Path(args.repo).resolve()
    if not repo.is_dir():
        print(f"error: 解析対象のディレクトリがありません: {repo}", file=sys.stderr)
        return 2

    try:
        report = run_dump_corpus(
            str(bin_path), repo, build_config(args.set), args.timeout
        )
        validate_report(report)
        state = extract_state(report, repo)
    except EvalError as err:
        print(f"error: {err}", file=sys.stderr)
        return 2

    baseline_path = BASELINE_DIR / f"{args.name}.json"
    if args.update_baseline:
        BASELINE_DIR.mkdir(parents=True, exist_ok=True)
        payload = {
            "name": args.name,
            "recorded_at": datetime.date.today().isoformat(),
            **state,
        }
        baseline_path.write_text(
            json.dumps(payload, ensure_ascii=False, indent=1) + "\n", encoding="utf-8"
        )
        action = "更新" if baseline_path.exists() else "登録"
        print(
            f"{action}: {baseline_path.relative_to(REPO)}"
            f"(docs={len(state['fixed']['docs'])}, pairs={len(state['pairs'])})"
        )
        return 0

    try:
        baseline = load_baseline(baseline_path)
    except EvalError as err:
        print(f"error: {err}", file=sys.stderr)
        return 2

    messages, failed = compare(baseline, state)
    print(
        f"# baseline: {baseline_path.relative_to(REPO)}(recorded {baseline['recorded_at']})"
    )
    print(
        f"# 実測: docs={len(state['fixed']['docs'])}, 生ペア={len(state['pairs'])}"
        f"(baseline: docs={len(baseline['fixed']['docs'])}, "
        f"生ペア={len(baseline['pairs'])})"
    )
    for msg in messages:
        print(msg)
    if not failed:
        print("一致: 実分布の検出挙動(生ペア・入力条件)はベースラインどおりです")
        return 0
    print(
        "差分あり: 意図的な変更なら --update-baseline でベースラインを更新してください"
    )
    return 1


if __name__ == "__main__":
    sys.exit(main())
