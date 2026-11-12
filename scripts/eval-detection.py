#!/usr/bin/env python3
"""検出感度パラメータの調整・検証harness(Issue #35 で再設計)。

`jaio dump corpus` が出力する表示上限適用前の重複候補ペア(score 付き)を使い、
ラベル付きグラウンドトゥルースで候補抽出の precision/recall を計測する。
表示上の診断件数は評価の根拠にしない。

正解データの区別:
  - 正例: 植えた完全重複ペア / 近似重複ペア(ティア別)
  - 負例: トピックを共有しない基本文書同士のペア
  - 未判定: 上記以外(派生文書 vs 派生文書、トピックを共有する未植えペア等)。
    検出されても偽陽性に数えない

使い方:

    # 既定パラメータで 1 回計測
    ./scripts/eval-detection.py

    # detection.doc_similarity を縦断比較(チューニング)
    ./scripts/eval-detection.py --grid 0.6,0.65,0.7,0.75

    # [detection] 等の任意キーを上書きして計測
    ./scripts/eval-detection.py --set detection.comment_similarity=0.5

    # 実リポジトリの類似度分布を測定(閾値検討の材料)
    ./scripts/eval-detection.py --measure-dist /path/to/repo

jaio.toml を一時コーパス側に書いて計測するため、リポジトリ自体の設定には
影響しない。分布測定は jaio を実行せず直接計算する(コードブロックと見出しを
除く簡易抽出のため、本体の本文抽出とは若干異なる近似)。
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
import tempfile
import time
import tomllib
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

# 合成コーパスのトピック文(相互に語彙が離れている想定)
TOPICS = [
    (
        "設定",
        "設定ファイルは起動時に一度だけ読み込まれ、変更は再起動後に反映されます。検証は読み込み直後に行い、不正な項目があれば起動を中断します。",
    ),
    (
        "認証",
        "トークンは発行から一定時間で失効し、失効後は更新トークンでの再取得が必要になります。スコープは最小権限に限定します。",
    ),
    (
        "課金",
        "利用量は日次で集計され、請求額は月末に確定します。超過分は従量課金として翌月に繰り越されます。",
    ),
    (
        "ログ",
        "ログは構造化形式で出力され、相関 ID でトレースを連結します。保持期間は重大度ごとに異なります。",
    ),
    (
        "検索",
        "インデックスは非同期で更新され、反映遅延は数秒以内に収まります。クエリはキャッシュされ、命中率を継続監視します。",
    ),
    (
        "通知",
        "通知はイベント駆動で配信され、失敗時は指数バックオフで再試行します。重複配信を防ぐため冪等キーを使います。",
    ),
    (
        "更新",
        "マイグレーションは起動時に自動適用され、適用済みのものはスキップします。失敗時は前の状態へ巻き戻します。",
    ),
    (
        "権限",
        "ロールは階層構造を持ち、継承された権限は明示的な拒否で上書きできます。監査ログに全操作を記録します。",
    ),
    (
        "容量",
        "ストレージは世代管理され、古い世代は自動的に圧縮されます。容量しきい値を超えると書き込みを制限します。",
    ),
    (
        "障害",
        "ヘルスチェックは数秒間隔で実行され、連続失敗で障害と判定します。判定後は自動で縮退運転に切り替えます。",
    ),
    (
        "同期",
        "複数端末間の状態は最終更新時刻で調停され、競合は利用者に確認を求めます。オフライン変更はキューに蓄積します。",
    ),
    (
        "暗号",
        "鍵は定期的にローテーションされ、旧鍵は復号専用として期間を限って保持します。アルゴリズムは設定で選択します。",
    ),
    (
        "翻訳",
        "文言はキーで管理され、未翻訳のキーは既定言語にフォールバックします。翻訳メモリで同一文の重複を避けます。",
    ),
    (
        "監視",
        "メトリクスは秒粒度で収集し、異常値は移動平均からの逸脱で判定します。アラートは段階的にエスカレーションします。",
    ),
    (
        "搬送",
        "ジョブはキューに入り、ワーカーが並列で消費します。処理失敗はリトライ回数を超えるとデッドレターへ移動します。",
    ),
]

# 近似重複の難易度別ティア(検証する類似度帯を覆う)
NEAR_TIERS = ("append", "replace", "delete")

# 近似重複の append ティアで末尾に足す文(複数の派生文書で共有され得るため、
# 派生同士のペアは「未判定」として扱い負例に数えない)
APPEND_SENTENCE = "補足として運用時の確認手順を追記します。"


class EvalError(Exception):
    """入力検証や fixture 生成の失敗(終了コード 2 に対応)"""


def build_corpus(
    dir_: Path, n_docs: int
) -> dict[str, object]:
    """合成コーパスを生成し、ラベル付きペアを返す。

    戻り値:
        positives_exact: 完全重複ペア(frozenset の集合)
        positives_near:  {ティア: 近似重複ペアの集合}
        negatives:       トピックを共有しない基本文書ペア(真の負例)
        doc_topics:      {基本文書名: トピック index の集合}(検査用)
    """
    combos: list[tuple[int, int]] = [
        (a, b) for a in range(len(TOPICS)) for b in range(a + 1, len(TOPICS))
    ]
    if n_docs > len(combos):
        raise EvalError(
            f"--docs {n_docs} はトピック組合せ数 {len(combos)} を超えています。"
            "循環で同一本文が再登場するため評価できません"
        )
    if n_docs < 13:
        raise EvalError("--docs は 13 以上が必要です(正例の派生元を確保するため)")

    names: list[str] = []
    doc_topics: dict[str, set[int]] = {}
    for idx in range(n_docs):
        a, b = combos[idx]
        topics = {a, b, (a + b) % len(TOPICS)}
        body = "".join(TOPICS[t][1] for t in (a, b, (a + b) % len(TOPICS)))
        name = f"doc{idx:03d}.md"
        (dir_ / name).write_text(f"# 文書{idx}\n\n{body}\n", encoding="utf-8")
        names.append(name)
        doc_topics[name] = topics

    positives_exact: set[frozenset[str]] = set()
    positives_near: dict[str, set[frozenset[str]]] = {t: set() for t in NEAR_TIERS}

    # 完全重複: 本文同一・見出しのみ別
    for k in range(3):
        body = (dir_ / names[k]).read_text(encoding="utf-8").split("\n\n", 1)[1]
        dst = f"copy{k}.md"
        (dir_ / dst).write_text(f"# 複製{k}\n\n{body}", encoding="utf-8")
        positives_exact.add(frozenset({names[k], dst}))

    # 近似重複(ティア別)。置換・削除は元文書に実際に存在する
    # トピック文だけを対象にし、生成後に正規化本文が元と異なることを保証する
    for k in range(3):
        src = names[10 + k]
        text = (dir_ / src).read_text(encoding="utf-8")
        head, body = text.split("\n\n", 1)
        src_topics = sorted(doc_topics[src])

        dst = f"near_append{k}.md"
        (dir_ / dst).write_text(
            f"{head}\n\n{body.rstrip()}\n{APPEND_SENTENCE}\n", encoding="utf-8"
        )
        positives_near["append"].add(frozenset({src, dst}))

        # 置換: src のトピック文の 1 つを、src に含まれない別トピック文へ
        victim = src_topics[k % len(src_topics)]
        replacement = next(t for t in range(len(TOPICS)) if t not in doc_topics[src])
        replaced = body.replace(TOPICS[victim][1], TOPICS[replacement][1], 1)
        assert replaced != body, f"{src}: 置換対象が存在しません"
        dst = f"near_replace{k}.md"
        (dir_ / dst).write_text(f"{head}\n\n{replaced}", encoding="utf-8")
        positives_near["replace"].add(frozenset({src, dst}))

        # 削除: src のトピック文の 1 つを除去
        victim = src_topics[(k + 1) % len(src_topics)]
        deleted = body.replace(TOPICS[victim][1], "", 1)
        assert deleted != body, f"{src}: 削除対象が存在しません"
        dst = f"near_delete{k}.md"
        (dir_ / dst).write_text(f"{head}\n\n{deleted.strip()}\n", encoding="utf-8")
        positives_near["delete"].add(frozenset({src, dst}))

    # 負例: トピックを 1 つも共有しない基本文書同士のペアのみ。
    # トピックを共有する未植えペアや派生文書を含むペアは「未判定」で、
    # 検出されても偽陽性に数えない(真の重複であり得るため)
    negatives: set[frozenset[str]] = set()
    for i in range(len(names)):
        for j in range(i + 1, len(names)):
            if doc_topics[names[i]].isdisjoint(doc_topics[names[j]]):
                negatives.add(frozenset({names[i], names[j]}))

    return {
        "positives_exact": positives_exact,
        "positives_near": positives_near,
        "negatives": negatives,
        "doc_topics": doc_topics,
    }


def _toml_literal(value: str) -> str:
    """--set の値を TOML として安全に出力する。数値・真偽値・配列・
    クォート済み文字列などリテラルとして解釈できる値はそのまま、
    解釈できない値(裸の文字列)は基本文字列としてクォートする"""
    try:
        tomllib.loads(f"k = {value}")
    except tomllib.TOMLDecodeError:
        return json.dumps(value, ensure_ascii=False)
    return value


def build_jaio_toml(
    floor: float, sets: list[tuple[str, str, str]]
) -> dict[str, object]:
    """実効 jaio.toml を生成する。既定値と --set をセクション別マップで
    統合してから各テーブルを 1 回だけ出力する(同一テーブルの二重宣言は
    TOML として不正、#35)。同一キーの重複指定は後勝ち。

    戻り値: {"toml": 生成テキスト, "sections": {セクション: {キー: 値}}}
    """
    sections: dict[str, dict[str, str]] = {
        "detection": {"doc_similarity": f"{floor}"},
        "merge": {"min_similarity": f"{floor}"},
    }
    for sec, key, value in sets:
        sections.setdefault(sec, {})[key] = _toml_literal(value)

    # 候補取得の下限は 2 閾値の低い方(Corpus::new の cutoff と同じ)。
    # これが評価閾値の最小(floor)を上回ると必要な候補が dump 側で落とされ
    # 再現率を過大評価するため、その構成は拒否する
    try:
        cutoff = min(
            float(sections["detection"]["doc_similarity"]),
            float(sections["merge"]["min_similarity"]),
        )
    except (KeyError, ValueError) as e:
        raise EvalError(f"実効設定の閾値を解釈できません: {e}")
    if cutoff > floor:
        raise EvalError(
            f"候補取得の下限 {cutoff} が評価閾値の最小 {floor} を上回っています。"
            "(--set の閾値を下げるか評価範囲を見直してください)"
        )

    lines: list[str] = []
    for sec, pairs in sections.items():
        lines.append(f"[{sec}]")
        for key, value in pairs.items():
            lines.append(f"{key} = {value}")
        lines.append("")
    toml = "\n".join(lines)
    # 生成物が TOML として有効か自検する(放出後の失敗をここで検出する)
    try:
        tomllib.loads(toml)
    except tomllib.TOMLDecodeError as e:
        raise EvalError(f"生成した jaio.toml が TOML として不正です: {e}")
    return {"toml": toml, "sections": sections}


# `jaio dump corpus` の出力契約(#39)。評価入力として受理できるのは
# この契約を満たす完了済みレポートのみ
DUMP_KIND = "jaio/dump-corpus"
DUMP_VERSION = 2
CANDIDATE_METHODS = {"all_pairs", "simhash_banding"}


def _require(condition: bool, message: str) -> None:
    if not condition:
        raise EvalError(f"dump corpus の出力契約違反: {message}")


def _is_int(value: object) -> bool:
    return isinstance(value, int) and not isinstance(value, bool)


def _is_number(value: object) -> bool:
    return (isinstance(value, (int, float)) and not isinstance(value, bool)) and (
        value == value and abs(value) != float("inf")
    )


def validate_dump_report(
    report: object, floor: float, require_all_pairs: bool
) -> dict[str, object]:
    """dump corpus の JSON を評価入力として妥当か検証し、context を返す。

    必須項目の欠損を空値に補完せず、異常終了・契約外の出力・矛盾した
    完全性表示は全て拒否する(#35)。近似検索自体は運用失敗ではないため
    拒否しないが、exact 参照評価(require_all_pairs)のときだけ全ペア性を
    検証する
    """
    _require(isinstance(report, dict), "JSON オブジェクトではありません")
    _require(
        report.get("kind") == DUMP_KIND,
        f"kind が {report.get('kind')!r}({DUMP_KIND} 以外は対応外)",
    )
    _require(
        report.get("version") == DUMP_VERSION,
        f"version が {report.get('version')!r}(対応は {DUMP_VERSION})",
    )
    _require(
        report.get("analysis_complete") is True,
        "analysis_complete が true ではありません(部分結果は評価に使いません)",
    )

    context = report.get("context")
    _require(isinstance(context, dict), "context が欠けています")

    errors = context.get("operational_errors")
    _require(isinstance(errors, list), "context.operational_errors が欠けています")
    _require(
        not errors,
        "analysis_complete=true なのに operational_errors が非空です(矛盾した結果)",
    )

    targets = context.get("targets")
    _require(isinstance(targets, dict), "context.targets が欠けています")
    for key in ("discovered", "analyzed", "skipped"):
        _require(
            _is_int(targets.get(key)) and targets[key] >= 0,
            f"targets.{key} が非負整数ではありません",
        )
    _require(
        targets.get("discovery_complete") is True,
        "targets.discovery_complete が true ではありません(入力発見が未完)",
    )
    _require(
        targets["analyzed"] + targets["skipped"] == targets["discovered"],
        "targets の件数が矛盾しています(analyzed+skipped != discovered)",
    )
    _require(
        targets["skipped"] == 0,
        "解析完了なのに skipped が非 0 です(矛盾した結果)",
    )

    fingerprint = context.get("config_fingerprint")
    _require(
        isinstance(fingerprint, str) and fingerprint.startswith("sha256:"),
        "context.config_fingerprint が sha256 識別ではありません",
    )
    _require(isinstance(context.get("rules"), list), "context.rules が欠けています")
    _require(
        isinstance(context.get("algorithms"), dict), "context.algorithms が欠けています"
    )
    # 消費しない項目も宣言された契約の形だけは検証する(契約外の出力を
    # 成功した評価として扱わない、#35)
    sources = context.get("sources")
    _require(isinstance(sources, list), "context.sources が欠けています")
    for src in sources:
        _require(
            isinstance(src, dict)
            and isinstance(src.get("path"), str)
            and src.get("path") != ""
            and isinstance(src.get("sha256"), str)
            and src.get("sha256") != "",
            f"sources の要素が不正です: {src}",
        )

    search = context.get("candidate_search")
    _require(isinstance(search, dict), "context.candidate_search が欠けています")
    _require(
        search.get("method") in CANDIDATE_METHODS,
        f"candidate_search.method が {search.get('method')!r}(未知の方式)",
    )
    _require(
        search.get("all_pairs") is True or search.get("all_pairs") is False,
        "candidate_search.all_pairs が真偽値ではありません",
    )
    _require(
        (search["method"] == "all_pairs") == search["all_pairs"],
        "candidate_search の method と all_pairs が矛盾しています",
    )
    for key in ("comparable_docs", "candidates", "scored_pairs"):
        _require(
            _is_int(search.get(key)) and search[key] >= 0,
            f"candidate_search.{key} が非負整数ではありません",
        )
    cutoff = search.get("cutoff")
    _require(_is_number(cutoff), "candidate_search.cutoff が有限数値ではありません")
    _require(
        cutoff <= floor + 1e-9,
        f"実行側の cutoff {cutoff} が要求 floor {floor} を上回っています"
        "(必要な候補が取得されていない)",
    )
    _require(
        search["comparable_docs"] <= targets["analyzed"],
        "comparable_docs が analyzed を超えています(矛盾した結果)",
    )
    _require(
        search["scored_pairs"] <= search["candidates"],
        "scored_pairs が candidates を超えています(矛盾した結果)",
    )
    if require_all_pairs:
        _require(
            search["method"] == "all_pairs" and search["all_pairs"] is True,
            "exact 参照評価を要求したのに近似列挙(simhash_banding)でした",
        )
        expected = search["comparable_docs"] * (search["comparable_docs"] - 1) // 2
        _require(
            search["scored_pairs"] == expected,
            f"全ペア採点数 {search['scored_pairs']} が "
            f"C({search['comparable_docs']},2)={expected} と一致しません",
        )

    docs = report.get("docs")
    _require(isinstance(docs, list), "docs が欠けています")
    doc_ids: set[str] = set()
    for doc in docs:
        _require(isinstance(doc, dict), "docs の要素がオブジェクトではありません")
        _require(
            isinstance(doc.get("file"), str) and doc.get("file") != "",
            f"docs の要素の file が不正です: {doc}",
        )
        _require(doc["file"] not in doc_ids, f"docs の file が重複しています: {doc}")
        doc_ids.add(doc["file"])
        for key in ("tokens", "body_chars"):
            _require(
                _is_int(doc.get(key)) and doc[key] >= 0,
                f"docs の要素の {key} が非負整数ではありません: {doc}",
            )
        sections = doc.get("sections")
        _require(isinstance(sections, list), f"docs の要素の sections が不正です: {doc}")
        for section in sections:
            _require(
                isinstance(section, dict)
                and _is_int(section.get("line"))
                and section["line"] > 0
                and isinstance(section.get("title"), str),
                f"docs の要素の sections が不正です: {section}",
            )

    duplicates = report.get("duplicates")
    _require(isinstance(duplicates, list), "duplicates が欠けています")
    for pair in duplicates:
        _require(
            isinstance(pair, dict), "duplicates の要素がオブジェクトではありません"
        )
        a, b, score = pair.get("a"), pair.get("b"), pair.get("score")
        _require(
            isinstance(a, str) and isinstance(b, str) and a and b and a != b,
            f"ペアの識別子が不正です: {pair}",
        )
        _require(
            a in doc_ids and b in doc_ids,
            f"ペアの識別子が docs に存在しません: {pair}",
        )
        _require(
            _is_number(score) and 0.0 <= score <= 1.0,
            f"score が 0..1 の有限数値ではありません: {pair}",
        )
        for key in ("intersection", "union"):
            _require(
                _is_int(pair.get(key)) and pair[key] >= 0,
                f"ペアの {key} が非負整数ではありません: {pair}",
            )

    comments = report.get("comments")
    _require(isinstance(comments, list), "comments が欠けています")
    for comment in comments:
        _require(
            isinstance(comment, dict)
            and isinstance(comment.get("file"), str)
            and comment.get("file") != ""
            and _is_int(comment.get("body_chars"))
            and comment["body_chars"] >= 0,
            f"comments の要素が不正です: {comment}",
        )
    return context


def run_dump(
    bin_: str, corpus: Path, floor: float, sets: list[tuple[str, str, str]], timeout: int
) -> dict[str, object]:
    """jaio.toml を書いて `jaio dump corpus` を 1 回実行し、候補ペア列を返す。

    候補の下限は detection.doc_similarity と merge.min_similarity の低い方。
    評価したい全閾値をカバーするよう floor に下げて実行し、閾値ごとの
    判定は score でクライアント側に行う(1 回の実行で縦断評価できる)。

    異常終了・契約外の出力・不完全な解析は評価入力に昇格させず EvalError
    にする(#35)。`--set detection.all_pairs=true` の exact 参照評価では
    全ペア採点まで検証する
    """
    built = build_jaio_toml(floor, sets)
    toml = str(built["toml"])
    # 環境変数 JAIO_CONFIG による上書きを防ぐため、生成した設定を
    # --config で明示する(#65。設定選択の優先順位は 明示 --config >
    # JAIO_CONFIG > 設定ファイル探索)
    config_path = (corpus / "jaio.toml").resolve()
    config_path.write_text(toml, encoding="utf-8")

    start = time.perf_counter()
    proc = subprocess.run(
        [bin_, "dump", "corpus", ".", "--config", str(config_path)],
        cwd=corpus,
        capture_output=True,
        text=True,
        timeout=timeout,
    )
    elapsed = time.perf_counter() - start
    # JSON は異常終了時も原因説明のため読んでよいが、評価用データへ
    # 昇格させるのは returncode == 0 のときだけに分離する(#35)
    report: object = None
    try:
        report = json.loads(proc.stdout)
    except json.JSONDecodeError:
        pass
    if proc.returncode != 0:
        detail = proc.stderr[:500]
        if isinstance(report, dict):
            ctx = report.get("context")
            if isinstance(ctx, dict) and ctx.get("operational_errors"):
                detail = json.dumps(ctx["operational_errors"], ensure_ascii=False)[:500]
        raise EvalError(
            f"jaio dump corpus が異常終了しました(exit {proc.returncode}): {detail}"
        )
    if report is None:
        raise EvalError("jaio dump corpus の出力が JSON ではありません")

    require_all_pairs = any(
        sec == "detection" and key == "all_pairs" and value.strip() == "true"
        for sec, key, value in sets
    )
    context = validate_dump_report(report, floor, require_all_pairs)
    assert isinstance(report, dict)

    # 生成したコーパスの全文書が解析対象に含まれることを照合する(#65)。
    # 外部設定の混入等で文書が除外されても、欠落を通常の成績として受理しない
    expected_docs = {p.relative_to(corpus).as_posix() for p in corpus.rglob("*.md")}
    analyzed = {
        str(s["path"]).removeprefix("./") for s in context["sources"]
    }
    missing = expected_docs - analyzed
    if missing:
        raise EvalError(
            f"解析対象に生成文書が欠けています: {sorted(missing)}"
            "(外部設定の混入でないか確認してください)"
        )

    # パスはコーパス相対の一意な識別子として扱い、左右を正規化する
    candidates: dict[frozenset[str], float] = {}
    for pair in report["duplicates"]:
        candidates[frozenset({pair["a"], pair["b"]})] = float(pair["score"])
    return {
        "candidates": candidates,
        "elapsed": elapsed,
        "jaio_toml": toml,
        "effective_config": built["sections"],
        # 実際に実行された側の記録(#35/#39): 生成した設定と実効設定の一致確認用
        "config_fingerprint": context["config_fingerprint"],
        "candidate_search": context["candidate_search"],
        "targets": context["targets"],
    }


def pct(numer: int, denom: int) -> str:
    """分母 0 を 0%/100% と誤表示せず未定義と明示する。"""
    return "n/a" if denom == 0 else f"{numer / denom:.0%}"


def summarize_at(
    candidates: dict[frozenset[str], float], labels: dict[str, object], threshold: float
) -> dict[str, object]:
    """閾値 threshold での検出結果をラベル別に集計する。"""
    detected = {p for p, s in candidates.items() if s >= threshold}
    pos_exact: set[frozenset[str]] = labels["positives_exact"]
    pos_near: dict[str, set[frozenset[str]]] = labels["positives_near"]
    negatives: set[frozenset[str]] = labels["negatives"]
    all_pos = pos_exact | set().union(*pos_near.values())

    tp = len(detected & all_pos)
    fp = len(detected & negatives)
    unlabeled = len(detected - all_pos - negatives)
    tp_fp = tp + fp
    return {
        "threshold": threshold,
        "exact_recall": (len(detected & pos_exact), len(pos_exact)),
        "near_recall": {
            tier: (len(detected & pairs), len(pairs))
            for tier, pairs in pos_near.items()
        },
        "tp": tp,
        "fp": fp,
        "fn": len(all_pos) - tp,
        "unlabeled_detections": unlabeled,
        "precision": (tp, tp_fp),
        "candidates": len(candidates),
        "detected": len(detected),
    }


def evaluate(
    bin_: str,
    n_docs: int,
    settings: list[float],
    sets: list[tuple[str, str, str]],
    timeout: int,
) -> dict[str, object]:
    """dump を 1 回実行し、全閾値の結果をまとめて返す。"""
    floor = min(settings) if settings else 0.65
    with tempfile.TemporaryDirectory(prefix="jaio-eval-") as tmp:
        corpus = Path(tmp)
        labels = build_corpus(corpus, n_docs)
        result = run_dump(bin_, corpus, floor, sets, timeout)
        candidates: dict[frozenset[str], float] = result["candidates"]
        return {
            "jaio_toml": result["jaio_toml"],
            "effective_config": result["effective_config"],
            "config_fingerprint": result["config_fingerprint"],
            "candidate_search": result["candidate_search"],
            "targets": result["targets"],
            "elapsed": result["elapsed"],
            "floor": floor,
            "results": [
                summarize_at(candidates, labels, value) for value in settings
            ],
            "labels": {
                "positives_exact": len(labels["positives_exact"]),
                "positives_near": {
                    t: len(p) for t, p in labels["positives_near"].items()
                },
                "negatives": len(labels["negatives"]),
            },
        }


def git_revision() -> str:
    """評価スクリプト側の Git revision。実行バイナリのビルド元とは
    別物として記録する(#35)"""
    try:
        out = subprocess.run(
            ["git", "rev-parse", "--short", "HEAD"],
            cwd=REPO,
            capture_output=True,
            text=True,
            timeout=10,
        )
        return out.stdout.strip() if out.returncode == 0 else "unknown"
    except OSError:
        return "unknown"


def binary_sha256(path: str) -> str:
    """実行バイナリ実体の SHA-256。外部バイナリでもビルドを一意に識別する。
    読めない場合は識別不能として "unknown" を返す(#35)"""
    try:
        digest = hashlib.sha256()
        with open(path, "rb") as f:
            for chunk in iter(lambda: f.read(1 << 20), b""):
                digest.update(chunk)
        return digest.hexdigest()
    except OSError:
        return "unknown"


# ---- 実リポジトリの類似度分布測定(jaio と同じ算法の python 再現) ----

FENCE = re.compile(r"```.*?```", re.DOTALL)
HEADING = re.compile(r"^#+ .*$", re.MULTILINE)


def normalize_text(text: str) -> str:
    return "".join(c.lower() for c in text if c.isalnum())


def extract_body(source: str) -> str:
    """コードフェンスと見出し行を除く簡易本文抽出(本体の抽出の近似)。"""
    source = FENCE.sub("", source)
    source = HEADING.sub("", source)
    return normalize_text(source)


def bigrams(text: str) -> set[str]:
    return {text[i : i + 2] for i in range(len(text) - 1)}


def jaccard(a: set[str], b: set[str]) -> float:
    if not a or not b:
        return 0.0
    return len(a & b) / len(a | b)


def measure_dist(root: Path, min_chars: int) -> int:
    files = sorted(
        p for p in root.rglob("*.md") if ".git" not in p.parts and p.stat().st_size > 0
    )
    bodies = [
        (p, extract_body(p.read_text(encoding="utf-8", errors="replace")))
        for p in files
    ]
    bodies = [(p, b) for p, b in bodies if len(b) >= min_chars]

    scores: list[float] = []
    for i in range(len(bodies)):
        for j in range(i + 1, len(bodies)):
            scores.append(jaccard(bigrams(bodies[i][1]), bigrams(bodies[j][1])))
    scores.sort()

    buckets = [0.3, 0.5, 0.6, 0.65, 0.7, 0.75, 0.85, 1.01]
    labels = [
        "<0.3",
        "0.3-0.5",
        "0.5-0.6",
        "0.6-0.65",
        "0.65-0.7",
        "0.7-0.75",
        "0.75-0.85",
        "0.85+",
    ]
    counts = [0] * len(labels)
    for s in scores:
        for k, upper in enumerate(buckets):
            if s < upper:
                counts[k] += 1
                break
    median = scores[len(scores) // 2] if scores else 0.0

    print(f"対象: {root} の md {len(bodies)} 件(正規化後本文 {min_chars} 文字以上)")
    print(f"全ペア数: {len(scores)}、中央値: {median:.2f}")
    print(f"{'帯':>10} {'件数':>8}")
    for label, count in zip(labels, counts):
        print(f"{label:>10} {count:>8}")
    print("注: 本文抽出は簡易版(コードフェンス・見出し除去)。本体の抽出とは若干異なる")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "--bin",
        default=str(REPO / "target" / "debug" / "jaio"),
        help="jaio バイナリのパス",
    )
    parser.add_argument(
        "--docs",
        type=int,
        default=60,
        help="生成する基本文書数(上限はトピック組合せ数。派生文書は別)",
    )
    parser.add_argument(
        "--min-similarity",
        type=float,
        default=0.65,
        help="評価する detection.doc_similarity(既定は本体の既定値と同じ 0.65)",
    )
    parser.add_argument(
        "--set",
        action="append",
        default=[],
        metavar="SECTION.KEY=VALUE",
        help="jaio.toml の任意キーの上書き(例: detection.comment_similarity=0.5)。複数回指定可",
    )
    parser.add_argument(
        "--grid",
        help="カンマ区切りで detection.doc_similarity を縦断比較(例: 0.6,0.65,0.7,0.75)",
    )
    parser.add_argument(
        "--measure-dist",
        metavar="REPO",
        help="合成計測ではなく、指定リポジトリの全ペア類似度分布を測定する",
    )
    parser.add_argument(
        "--min-chars",
        type=int,
        default=50,
        help="--measure-dist で比較対象とする本文の最小文字数(本体の doc_body_min_chars に対応)",
    )
    parser.add_argument(
        "--timeout", type=int, default=300, help="jaio 1 回のタイムアウト秒"
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="結果を機械可読 JSON でも出力する(実効設定・ラベル件数つき)",
    )
    args = parser.parse_args()

    if args.measure_dist:
        root = Path(args.measure_dist).expanduser()
        if not root.is_dir():
            print(f"error: ディレクトリが見つからない: {root}", file=sys.stderr)
            return 2
        return measure_dist(root, args.min_chars)

    if not Path(args.bin).exists():
        print(f"error: jaio バイナリが見つかりません: {args.bin}", file=sys.stderr)
        print(
            "hint: cargo build -p jaio-cli してから再実行してください", file=sys.stderr
        )
        return 2

    sets: list[tuple[str, str, str]] = []
    for item in args.set:
        key, _, value = item.partition("=")
        sec, _, name = key.partition(".")
        if not (sec and name and value):
            print(
                f"error: --set の形式が不正: {item}(SECTION.KEY=VALUE)", file=sys.stderr
            )
            return 2
        sets.append((sec, name, value))

    try:
        settings = (
            [args.min_similarity]
            if not args.grid
            else [float(v) for v in args.grid.split(",")]
        )
    except ValueError:
        print("error: --grid の値が不正です", file=sys.stderr)
        return 2

    # コーパスを cwd にして実行するため、バイナリは絶対パスに解決する
    bin_path = str(Path(args.bin).resolve())
    # 実行記録: バイナリ実体の hash とスクリプトの Git revision は区別する
    # (#35)。外部ビルドのバイナリでも sha256 が実体を一意に識別する
    bin_id = binary_sha256(bin_path)
    print(f"# bin={args.bin} bin_sha256={bin_id} script_rev={git_revision()} docs={args.docs}")
    try:
        outcome = evaluate(bin_path, args.docs, settings, sets, args.timeout)
    except EvalError as e:
        print(f"error: {e}", file=sys.stderr)
        return 2

    labels = outcome["labels"]
    print(
        f"# 正例: 完全 {labels['positives_exact']} / "
        + " / ".join(f"{t} {n}" for t, n in labels["positives_near"].items())
        + f"、負例 {labels['negatives']}、候補ペア {outcome['results'][0]['candidates'] if outcome['results'] else 0} 件"
        + f"(dump {outcome['elapsed']:.2f} 秒)"
    )
    # 実際に使われた候補列挙方式と完全性の記録(#39)。近似列挙なら全ペア
    # 未採点であることを明示し、表示上限への依存を避ける
    search = outcome.get("candidate_search") or {}
    targets = outcome.get("targets") or {}
    print(
        f"# candidate_search: method={search.get('method')} "
        f"all_pairs={search.get('all_pairs')} "
        f"comparable={search.get('comparable_docs')} "
        f"scored={search.get('scored_pairs')} cutoff={search.get('cutoff')} "
        f"| targets: analyzed={targets.get('analyzed')} "
        f"skipped={targets.get('skipped')} "
        f"fingerprint={outcome.get('config_fingerprint')}"
    )
    suffix = f"  (+--set x{len(sets)})" if sets else ""
    print(
        f"{'doc_similarity':>15} {'完全':>5} {'1文追加':>7} {'1文置換':>7} {'1文削除':>7}"
        f" {'TP':>4} {'FP':>4} {'未判定':>6} {'精度':>6} {'秒':>6}{suffix}"
    )
    for r in outcome["results"]:
        nr = r["near_recall"]
        print(
            f"{r['threshold']:>15.2f} {pct(*r['exact_recall']):>5}"
            f" {pct(*nr['append']):>6} {pct(*nr['replace']):>6} {pct(*nr['delete']):>6}"
            f" {r['tp']:>4} {r['fp']:>4} {r['unlabeled_detections']:>6}"
            f" {pct(*r['precision']):>6} {outcome['elapsed']:>6.2f}"
        )
    if args.json:
        print(
            json.dumps(
                {
                    "binary": {"path": args.bin, "sha256": bin_id},
                    "script_revision": git_revision(),
                    "docs": args.docs,
                    "sets": [f"{s}.{k}={v}" for s, k, v in sets],
                    "jaio_toml": outcome["jaio_toml"],
                    "effective_config": outcome["effective_config"],
                    "config_fingerprint": outcome["config_fingerprint"],
                    "candidate_search": outcome["candidate_search"],
                    "targets": outcome["targets"],
                    "floor": outcome["floor"],
                    "elapsed": outcome["elapsed"],
                    "labels": labels,
                    "results": outcome["results"],
                },
                ensure_ascii=False,
                indent=2,
            )
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
