#!/usr/bin/env python3
"""文レベルルール(効率・揺れ系)の再現率・偽陽性ベンチマーク(Issue #62)。

`jaio check --format json` の診断を、ラベル付き合成コーパスで採点する。
文書ペア重複の評価を行う eval-detection.py と対になる評価器。

正解データの区別(eval-detection.py と同じ規律):
  - 正例: 注入した欠陥(ルール別・注入行が既知)
  - 負例: クリーン文書上の全診断(ルール別の偽陽性として計上)。
    クリーン文書には検出パターンに近似する正当文(境界ケース、hardneg-*)を
    含める(#70: 無関係文だけのコーパスでは FP 計測が飽和するため)
  - 未判定: 注入文書上のラベル外診断。注入が別ルールを正当に
    発火させることがあるため、偽陽性には数えない

使い方:

    # 既定パラメータで 1 回計測(clean + defects コーパスを生成して check)
    ./scripts/eval-rules.py

    # jaio.toml の任意キーを上書きして計測
    ./scripts/eval-rules.py --set detection.long_sentence_min_chars=40

    # 機械可読な結果出力(ラベル単位の成否・未判定診断の明細つき)
    ./scripts/eval-rules.py --json

jaio.toml を一時コーパス側に書いて計測するため、リポジトリ自体の設定には
影響しない。
"""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

# 評価対象ルール(#62 の対象セット)。いずれも文・段落単位のルールで、
# 文書ペア類似度を扱う relation/merge 系は eval-detection.py の管轄
TARGET_RULES = [
    "efficiency/redundant-expression",
    "efficiency/filler",
    "efficiency/token-density",
    "efficiency/repeated-quote",
    "consistency/notation-variant",
    "consistency/terminology",
    "consistency/particle",
    "consistency/punctuation",
    "consistency/style-deviation",
]

# クリーン文書用の文プール(ですます体・冗長表現なし・ペア辞書の用語なし・
# 和文文脈の ASCII 句読点なし)。文書間で共有しない使い切りを前提にする
CLEAN_SENTENCES = [
    "起動時に設定ファイルを読み込み、不正な項目があれば警告を出します。",
    "接続が切れた場合は指数バックオフで再接続を試みます。",
    "日次バッチは深夜に実行され、翌朝までに結果が揃います。",
    "キャッシュの有効期限は設定で変更でき、既定は十分間です。",
    "保存したテンプレートは一覧画面から呼び出せます。",
    "検索結果は五十音順で並び替えて表示されます。",
    "入力値は送信前に形式検査を通し、不正な値は拒否されます。",
    "履歴は三十日間保持され、それ以降は自動で削除されます。",
    "通知はメールとアプリ内の両方へ届くように構成できます。",
    "エラー率が閾値を超えるとアラートが発報されます。",
    "初期値は環境変数から読み込まれ、未設定なら既定値が使われます。",
    "変更履歴は監査証跡として保存され、管理者が参照できます。",
    "並列実行数はワーカー数の設定で上限を制御できます。",
    "集計は十部単位で丸めて表示し、明細は別画面で確認します。",
    "失敗したジョブはキューの末尾へ戻され、再試行されます。",
    "接続先の一覧は設定画面で登録し、順序も並べ替えられます。",
    "読み込みに失敗した項目はスキップされ、件数が記録されます。",
    "出力形式は表形式と機械可読形式から選択できます。",
    "同じ名前の項目が複数ある場合は、作成日の新しい方が使われます。",
    "端末間の同期は最終更新時刻を基準に調停されます。",
    "招待された利用者は確認画面を経てから参加できます。",
    "廃止予定の項目は一覧で警告印付きで表示されます。",
    "容量の上限に近づくと、古い世代から順に圧縮されます。",
    "複数の条件を組み合わせた絞り込みも保存して再利用できます。",
    "地図上の範囲選択で対象を限定して一覧できます。",
    "下書きは自動保存され、閉じても内容は失われません。",
    "確認画面では差分が色分けされ、変更点を一目で追えます。",
    "取り込んだ画像は縮小され、原本は別領域に保持されます。",
    "支払い方法の変更は次回の請求から反映されます。",
    "端末の言語設定に応じて表示言語が切り替わります。",
    # token-density の高密度側に使う補充分(クリーン文書とは別文書)
    "認証トークンは発行から一定時間で失効し、更新には再認証が必要です。",
    "監査ログには操作者・対象・時刻が記録され、改変はできません。",
    "索引は非同期で再構築され、完了までは旧索引が使われます。",
    "請求額は月末に確定し、超過分は従量課金として翌月に繰り越されます。",
    "保持期間は重大度ごとに異なり、長期保存は別領域へ移されます。",
    "複数端末の変更は更新時刻で調停され、競合時は利用者に確認します。",
    "鍵は定期的にローテーションされ、旧鍵は復号専用で保持されます。",
    "未翻訳の文言は既定言語にフォールバックして表示されます。",
    "異常値は移動平均からの逸脱で判定し、段階的に通知先を広げます。",
    "処理に失敗したジョブは回数上限を超えると隔離領域へ移動します。",
    "役割は階層構造を持ち、下位の拒否設定が上位の許可を上書きします。",
    "古い世代の保管領域は自動的に圧縮され、閾値超過で書き込みを制限します。",
    "障害と判定した後は自動で縮退運転に切り替え、復旧を待ちます。",
    "照会は集約ビューにキャッシュされ、命中状況を継続的に監視します。",
    "割り当てられた帯域は時間帯で変わり、ピーク時は均等に分け合います。",
    "登録済みの端末だけが管理対象となり、未登録は隔離されます。",
    "集計の単位は部署ごとに設定でき、上位へは合算して報告されます。",
    "一時停止した処理は再開時に途中の状態から継続されます。",
    "表の列幅は内容に応じて自動調整され、手動での固定も可能です。",
    "公開された共有範囲は閲覧のみで、編集には別の許可が必要です。",
    "帳票の雛形は種類ごとに管理され、差し替えは即時に反映されます。",
    "処理の進捗は百分率で表示され、完了時に結果の要約が出ます。",
    "閲覧の記録は利用者ごとに残り、参照権限のある人が確認できます。",
    "複写した項目は原本への参照を持ち、原本の更新に追随します。",
    "絞り込み条件は名前を付けて保存でき、チーム内で共有できます。",
    "連携の設定は疎通確認を済ませてから保存する流れになっています。",
    "削除した項目は一定期間だけ復元でき、期限後は完全に消えます。",
    "登録内容の不備は一覧上で強調表示され、修正を促されます。",
    "連続した失敗は一時的な遮断を招き、解除には管理者の操作が要ります。",
    "利用開始の案内は初回のみ表示され、以後は設定画面から再確認できます。",
    "選択した範囲だけを対象にした一括操作も用意されています。",
    "反映の遅延は数秒程度で、画面の再読み込みで最新状態になります。",
    "承認の依頼は担当者へ届き、差し戻しには理由の記入が求められます。",
    "閲覧用の共有は期限つきで発行でき、期限後は自動で無効になります。",
    "規模の大きい一括処理は夜間の枠へ回され、完了は通知で知らせます。",
    "割当先の候補は稼働状況から絞り込まれ、均等に振り分けられます。",
]


class EvalError(Exception):
    """入力検証や fixture 生成の失敗(終了コード 2 に対応)"""


def write_doc(path: Path, heading: str, paragraphs: list[str]) -> list[int]:
    """見出し + 段落列の文書を書き、各段落の開始行(1 基点)を返す。"""
    lines = [f"# {heading}", ""]
    para_lines: list[int] = []
    for para in paragraphs:
        para_lines.append(len(lines) + 1)
        lines.append(para)
        lines.append("")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(lines), encoding="utf-8")
    return para_lines


def build_rule_corpus(dir_: Path) -> dict[str, object]:
    """クリーン文書とルール別の注入文書を生成し、ラベルを返す。

    戻り値:
        labels:      [{"rule", "file", "lines": [int], "match": "line"|"file"}]
                    lines は受理する報告行の集合(match=line)。
                    match=file は文書内どこへの報告でも検出扱いにする
        clean_files: 負例計測に使うクリーン文書の表示パス集合
    """
    labels: list[dict[str, object]] = []
    clean_files: set[str] = set()

    # ---- クリーン文書(偽陽性計測用) ----
    # 各文書は 3 段落 x 2 文(token-density の標本段落下限 3・段落トークン
    # 下限 20 を満たす構成)。文プールは使い切りで文書間重複を避ける
    pool = iter(CLEAN_SENTENCES)
    n_clean = 5
    for i in range(n_clean):
        paras = [next(pool) + next(pool) for _ in range(3)]
        name = f"clean{i:02d}.md"
        write_doc(dir_ / name, f"運用メモ{i}", paras)
        clean_files.add(name)

    # ---- 境界文書(ハードネガティブ、#70) ----
    # 検出パターンに近似する正当文を含み、クリーン文書と同じ負例計測に
    # 数える。ここで診断が出た場合はパターン・閾値の境界精度の欠陥として
    # 捉える(単純な無関係文だけのコーパスでは飽和するため)
    boundary_paras = [
        # redundant-expression: パターンの部分要素を持つが非一致
        "設定に誤りがある可能性があります。可能な限り簡潔にまとめます。",
        "予備の系は不要とします。次のようになると思います。",
        "ご確認いただきます。異論は言っても仕方ありません。",
        # notation-variant: 同音異義の別語(原形不一致で対象外のはず)
        "計算結果が返ります。設定を変えます。",
        # terminology: 推奨表記の単体使用(揺れにならない)
        "データベースに記録します。対応は翌日以降にします。",
    ]
    write_doc(dir_ / "hardneg-expr.md", "境界の確認", boundary_paras)
    clean_files.add("hardneg-expr.md")

    # repeated-quote: 閾値(30 字)未満の同一文の正当な反復
    short_repeat = "障害発生時は担当者がまず状況を確認して記録します。"
    punct_paras = [
        # punctuation: 数値・版数の ASCII 区切りは和文文脈ではない
        "上限は 1,000 件です。必要な版は 1.2.3 以降です。",
        "詳細は See §2.3, Notes を参照してください。注記は日本語で追記します。",
        short_repeat,
        short_repeat,
    ]
    punct_lines = ["# 境界の確認(句読点)", ""]
    for para in punct_paras:
        punct_lines.append(para)
        punct_lines.append("")
    # style-deviation: 箇条書きの体言止めは段落(判定対象)ではない
    punct_lines.append("- 初期設定の確認")
    punct_lines.append("- 設定値の見直し")
    punct_lines.append("")
    path = dir_ / "hardneg-punct.md"
    path.write_text("\n".join(punct_lines), encoding="utf-8")
    clean_files.add("hardneg-punct.md")

    def defect(
        name: str, heading: str, paragraphs: list[str], marked: dict[int, str]
    ) -> list[int]:
        """注入文書を書き、marked の {段落index: rule_id} をラベル化する。

        段落の開始行を返す(複数段落をまたぐ診断のラベル化に使う)。
        """
        lines = write_doc(dir_ / name, heading, paragraphs)
        for idx, rule in marked.items():
            labels.append(
                {
                    "rule": rule,
                    "file": name,
                    "lines": [lines[idx]],
                    "match": "line",
                }
            )
        return lines

    # ---- efficiency/redundant-expression: 内蔵パターン 3 種 ----
    defect(
        "def-redundant.md",
        "冗長表現の文書",
        [
            next(pool) + next(pool),
            "この画面では設定を変更することができます。",
            next(pool) + next(pool),
            "この機能は必要となります。",
            "手順は次のようになるかと思います。",
        ],
        {
            1: "efficiency/redundant-expression",
            3: "efficiency/redundant-expression",
            4: "efficiency/redundant-expression",
        },
    )

    # ---- efficiency/filler: 削除候補パターン ----
    # 内蔵パターン「と言っても過言ではない」は常体のため、敬体文書への注入で
    # 文レベルの style-deviation(少数派文体の warn)が併発する。これは注入の
    # 正当な副産物なのでラベル化する(1 診断が両文の locations を持つ)
    lines = defect(
        "def-filler.md",
        "フィラーの文書",
        [
            next(pool) + next(pool),
            "これは最も簡単な方法と言っても過言ではない。",
            "十分に安全な構成と言っても過言ではない。",
        ],
        {1: "efficiency/filler", 2: "efficiency/filler"},
    )
    labels.append(
        {
            "rule": "consistency/style-deviation",
            "file": "def-filler.md",
            "lines": [lines[1], lines[2]],
            "match": "line",
        }
    )

    # ---- efficiency/token-density: 高密度 7 段落 + 低密度 1 段落 ----
    # 平均 - 2σ の外れ値判定のため、密度のそろった段落を多めに敷く。
    # 低密度段落は名詞・動詞(コンテンツ語)を持たない中空の応答文で、
    # 比率 0 の外れ値になる(実測: 24 morphs・30 tokens・content 0)
    dense = [next(pool) + next(pool) for _ in range(7)]
    fluffy = "ええ。なるほど。そうですね。たしかにそうです。そうでしょうか。いやはや、なんとも。"
    paras = dense[:3] + [fluffy] + dense[3:]
    defect(
        "def-density.md",
        "密度の文書",
        paras,
        {3: "efficiency/token-density"},
    )

    # ---- efficiency/repeated-quote: 同一文(30 文字超)を 2 段落に ----
    repeated = "この設定値はすべての環境で同一の値を使わなければならないものとします。"
    name = "def-repeat.md"
    lines = write_doc(
        dir_ / name,
        "重複文の文書",
        [next(pool) + next(pool), repeated, next(pool) + next(pool), repeated],
    )
    # 報告は 2 回目の出現行、locations には全出現が載るため両行を受理する
    labels.append(
        {
            "rule": "efficiency/repeated-quote",
            "file": name,
            "lines": [lines[1], lines[3]],
            "match": "line",
        }
    )

    # ---- consistency/notation-variant: 行なう(1) vs 行う(2) ----
    # 少数派の初出行へ報告される。文書全体が常体になるため
    # 文書レベルの style-deviation(既定文体との不一致 info)も併発する。
    # 注入の正当な副産物なのでラベル化する
    lines = defect(
        "def-notation.md",
        "表記揺れの文書",
        [
            "手順は次のとおりだ。",
            "次に設定値を行なう。",
            "確認も行う。記録も行う。",
        ],
        {1: "consistency/notation-variant"},
    )
    labels.append(
        {
            "rule": "consistency/style-deviation",
            "file": "def-notation.md",
            "lines": [lines[0], lines[1], lines[2]],
            "match": "line",
        }
    )

    # ---- consistency/terminology: サーバー(推奨) vs サーバ(別表記) ----
    defect(
        "def-terms.md",
        "用語揺れの文書",
        [
            next(pool) + next(pool),
            "このサーバーは冗長化します。",
            "予備のサーバも起動します。",
        ],
        {2: "consistency/terminology"},
    )

    # ---- consistency/particle: 同一述語「設定する」の に/へ ----
    # opt-in(コーパス側 jaio.toml の detection.particle_pairs で有効化)。
    # 多数派 2 件・少数派 1 件で MIN_MAJORITY_USES=2・頻度比 3 を満たす
    defect(
        "def-particle.md",
        "助詞の文書",
        [
            next(pool) + next(pool),
            "通知先に設定します。",
            "通知先へ設定します。",
            "通知先に設定します。",
        ],
        {2: "consistency/particle"},
    )

    # ---- consistency/punctuation: 、多数派 + 和文文脈の ASCII , 1 箇所 ----
    defect(
        "def-punct.md",
        "句読点の文書",
        [
            next(pool) + next(pool),
            next(pool) + next(pool),
            "結果を確認します, 問題なければ続行します。",
            next(pool) + next(pool),
        ],
        {2: "consistency/punctuation"},
    )

    return {"labels": labels, "clean_files": clean_files}


def _toml_literal(value: str) -> str:
    """--set の値を TOML として安全に出力する(eval-detection.py と同じ規則)"""
    try:
        tomllib.loads(f"k = {value}")
    except tomllib.TOMLDecodeError:
        return json.dumps(value, ensure_ascii=False)
    return value


def build_jaio_toml(sets: list[tuple[str, str, str]]) -> dict[str, object]:
    """評価コーパス用の jaio.toml を生成する。

    既定で評価に必要な最小構成を持つ:
      - detection.particle_pairs: consistency/particle は opt-in のため
        注入対象のペア(に/へ)を有効化する
      - detection.orphan_entry_points: 文書間リンクを張らない合成コーパスでは
        orphan が全文書に発火して計測を埋めるため、全 md を対象外にする
        (orphan は文レベルではなくコーパス位相のルールで本評価の対象外)
    """
    sections: dict[str, dict[str, str]] = {
        "detection": {
            "particle_pairs": '[["に", "へ"]]',
            "orphan_entry_points": '["*.md"]',
        },
    }
    for sec, key, value in sets:
        sections.setdefault(sec, {})[key] = _toml_literal(value)

    lines: list[str] = []
    for sec, pairs in sections.items():
        lines.append(f"[{sec}]")
        for key, value in pairs.items():
            lines.append(f"{key} = {value}")
        lines.append("")
    toml = "\n".join(lines)
    try:
        tomllib.loads(toml)
    except tomllib.TOMLDecodeError as e:
        raise EvalError(f"生成した jaio.toml が TOML として不正です: {e}")
    return {"toml": toml, "sections": sections}


# `jaio check --format json` の出力契約(評価入力として受理できる形のみ)
CHECK_SCHEMA_VERSION = 2


def _require(condition: bool, message: str) -> None:
    if not condition:
        raise EvalError(f"check の出力契約違反: {message}")


def _is_int(value: object) -> bool:
    return isinstance(value, int) and not isinstance(value, bool)


def validate_check_report(report: object) -> dict[str, object]:
    """check の JSON を評価入力として妥当か検証し、context を返す。

    必須項目の欠損・矛盾した完全性表示を拒否する(eval-detection.py の
    validate_dump_report と同じ規律)。診断は消費する項目(file/rule_id/
    span.line/locations)の形だけを検証し、観察内容までは踏み込まない
    """
    _require(isinstance(report, dict), "JSON オブジェクトではありません")
    _require(
        report.get("schema_version") == CHECK_SCHEMA_VERSION,
        f"schema_version が {report.get('schema_version')!r}"
        f"(対応は {CHECK_SCHEMA_VERSION})",
    )
    tool = report.get("tool")
    _require(
        isinstance(tool, dict) and tool.get("name") == "jaio",
        "tool.name が jaio ではありません",
    )
    summary = report.get("summary")
    _require(isinstance(summary, dict), "summary が欠けています")
    _require(
        summary.get("analysis_complete") is True,
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

    diagnostics = report.get("diagnostics")
    _require(isinstance(diagnostics, list), "diagnostics が欠けています")
    for diag in diagnostics:
        _require(
            isinstance(diag, dict), "diagnostics の要素がオブジェクトではありません"
        )
        _require(
            isinstance(diag.get("file"), str) and diag.get("file") != "",
            f"診断の file が不正です: {diag}",
        )
        _require(
            isinstance(diag.get("rule_id"), str) and diag.get("rule_id") != "",
            f"診断の rule_id が不正です: {diag}",
        )
        span = diag.get("span")
        _require(
            isinstance(span, dict) and _is_int(span.get("line")) and span["line"] > 0,
            f"診断の span.line が不正です: {diag}",
        )
        locations = diag.get("locations", [])
        _require(isinstance(locations, list), f"locations が不正です: {diag}")
        for loc in locations:
            loc_span = loc.get("span") if isinstance(loc, dict) else None
            _require(
                isinstance(loc_span, dict)
                and _is_int(loc_span.get("line"))
                and loc_span["line"] > 0,
                f"locations の span.line が不正です: {diag}",
            )
    return context


def run_check(
    bin_: str, corpus: Path, sets: list[tuple[str, str, str]], timeout: int
) -> dict[str, object]:
    """jaio.toml を書いて `jaio check --format json` を 1 回実行する。

    終了コード 1(error 重大度の診断あり)は診断を伴う正常な報告として受理し、
    2(実行エラー・解析未完了)とその他の異常終了は評価入力に昇格させず
    EvalError にする(eval-detection.py の run_dump と同じ規律)
    """
    built = build_jaio_toml(sets)
    toml = str(built["toml"])
    # 環境変数 JAIO_CONFIG による上書きを防ぐため、生成した設定を
    # --config で明示する(#65。設定選択の優先順位は 明示 --config >
    # JAIO_CONFIG > 設定ファイル探索)
    config_path = (corpus / "jaio.toml").resolve()
    config_path.write_text(toml, encoding="utf-8")

    proc = subprocess.run(
        [bin_, "check", ".", "--format", "json", "--config", str(config_path)],
        cwd=corpus,
        capture_output=True,
        text=True,
        timeout=timeout,
    )
    report: object = None
    try:
        report = json.loads(proc.stdout)
    except json.JSONDecodeError:
        pass
    if proc.returncode not in (0, 1):
        detail = proc.stderr[:500]
        if isinstance(report, dict):
            ctx = report.get("context")
            if isinstance(ctx, dict) and ctx.get("operational_errors"):
                detail = json.dumps(ctx["operational_errors"], ensure_ascii=False)[:500]
        raise EvalError(
            f"jaio check が異常終了しました(exit {proc.returncode}): {detail}"
        )
    if report is None:
        raise EvalError("jaio check の出力が JSON ではありません")

    context = validate_check_report(report)
    assert isinstance(report, dict)

    # 生成したコーパスの全文書が解析対象に含まれることを照合する(#65)。
    # 外部設定の混入等で文書が除外されても、欠落を通常の成績として受理しない
    expected_docs = {p.relative_to(corpus).as_posix() for p in corpus.rglob("*.md")}
    analyzed = {str(s["path"]).removeprefix("./") for s in context["sources"]}
    missing = expected_docs - analyzed
    if missing:
        raise EvalError(
            f"解析対象に生成文書が欠けています: {sorted(missing)}"
            "(外部設定の混入でないか確認してください)"
        )

    diagnostics = []
    for diag in report["diagnostics"]:
        lines = {diag["span"]["line"]}
        for loc in diag.get("locations", []):
            lines.add(loc["span"]["line"])
        diagnostics.append(
            {
                "rule": diag["rule_id"],
                "file": diag["file"],
                "lines": lines,
                "severity": diag.get("severity"),
            }
        )

    rule_status = {
        r.get("rule_id"): r.get("status")
        for r in context["rules"]
        if isinstance(r, dict)
    }
    return {
        "diagnostics": diagnostics,
        "jaio_toml": toml,
        "effective_config": built["sections"],
        "config_fingerprint": context["config_fingerprint"],
        "morphology": context["algorithms"].get("morphology"),
        "targets": context["targets"],
        "rule_status": rule_status,
    }


def _norm_file(path: str) -> str:
    """check の file 表示をコーパス相対パスに揃える(./ 接頭辞の除去)"""
    return path[2:] if path.startswith("./") else path


def summarize(
    diagnostics: list[dict[str, object]],
    labels: list[dict[str, object]],
    clean_files: set[str],
) -> dict[str, object]:
    """診断をラベルと照合し、ルール別の再現率・偽陽性・未判定を集計する。

    - 正例への照合: file が一致し、診断の参照行(span/locations)のいずれかが
      ラベルの受理行に含まれる(match=line)。match=file は行を問わない
    - 偽陽性: クリーン文書上の診断(ルール別)
    - 未判定: 注入文書上のラベル外診断(正当な二次検出があり得るため
      偽陽性に数えない)
    """
    matched_label_ids: set[int] = set()
    unlabeled: list[dict[str, object]] = []
    fp_by_rule: dict[str, int] = {}

    for diag in diagnostics:
        diag_file = _norm_file(str(diag["file"]))
        if diag_file in clean_files:
            fp_by_rule[str(diag["rule"])] = fp_by_rule.get(str(diag["rule"]), 0) + 1
            continue
        hit = None
        for i, label in enumerate(labels):
            if label["rule"] != diag["rule"] or label["file"] != diag_file:
                continue
            # 同一ラベルへの重複ヒット(出現全行を載せる診断等)も
            # ラベルに吸収させ、未判定には数えない
            if label["match"] == "file" or diag["lines"] & set(label["lines"]):
                hit = i
                break
        if hit is not None:
            matched_label_ids.add(hit)
        else:
            unlabeled.append(
                {
                    "rule": diag["rule"],
                    "file": diag_file,
                    "lines": sorted(diag["lines"]),
                }
            )

    per_rule: dict[str, dict[str, object]] = {}
    for rule in TARGET_RULES:
        expected = [i for i, lb in enumerate(labels) if lb["rule"] == rule]
        detected = [i for i in expected if i in matched_label_ids]
        per_rule[rule] = {
            "expected": len(expected),
            "detected": len(detected),
            "fp_clean": fp_by_rule.get(rule, 0),
            "unlabeled": sum(1 for u in unlabeled if u["rule"] == rule),
            "missed_labels": [
                {"file": labels[i]["file"], "lines": labels[i]["lines"]}
                for i in expected
                if i not in matched_label_ids
            ],
        }
    # 対象外ルールのクリーン文書 FP も観察として残す
    other_fp = {r: n for r, n in fp_by_rule.items() if r not in per_rule}
    return {
        "per_rule": per_rule,
        "other_fp_clean": other_fp,
        "unlabeled": unlabeled,
        "total_diagnostics": len(diagnostics),
    }


def pct(numer: int, denom: int) -> str:
    """分母 0 を 0%/100% と誤表示せず未定義と明示する。"""
    return "n/a" if denom == 0 else f"{numer / denom:.0%}"


def git_revision() -> str:
    """評価スクリプト側の Git revision。実行バイナリのビルド元とは別物"""
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
    """実行バイナリ実体の SHA-256。読めない場合は unknown を返す"""
    try:
        digest = hashlib.sha256()
        with open(path, "rb") as f:
            for chunk in iter(lambda: f.read(1 << 20), b""):
                digest.update(chunk)
        return digest.hexdigest()
    except OSError:
        return "unknown"


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
        "--set",
        action="append",
        default=[],
        metavar="SECTION.KEY=VALUE",
        help="jaio.toml の任意キーの上書き(例: detection.long_sentence_min_chars=40)。複数回指定可",
    )
    parser.add_argument(
        "--timeout", type=int, default=300, help="jaio 1 回のタイムアウト秒"
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="結果を機械可読 JSON でも出力する(ラベル単位の成否つき)",
    )
    args = parser.parse_args()

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

    bin_path = str(Path(args.bin).resolve())
    bin_id = binary_sha256(bin_path)
    print(f"# bin={args.bin} bin_sha256={bin_id} script_rev={git_revision()}")
    with tempfile.TemporaryDirectory(prefix="jaio-eval-rules-") as tmp:
        corpus = Path(tmp)
        built = build_rule_corpus(corpus)
        result = run_check(bin_path, corpus, sets, args.timeout)
    labels: list[dict[str, object]] = built["labels"]  # type: ignore[assignment]
    clean_files: set[str] = built["clean_files"]  # type: ignore[assignment]
    summary = summarize(result["diagnostics"], labels, clean_files)

    targets = result["targets"]
    print(
        f"# context: fingerprint={result['config_fingerprint']} "
        f"morphology={result['morphology']}"
    )
    print(
        f"# targets: analyzed={targets['analyzed']} "
        f"skipped={targets['skipped']} | 診断 {summary['total_diagnostics']} 件"
        f"(clean {len(clean_files)} 文書 / defects 8 文書)"
    )
    # 対象ルールが実行されなかった設定では再現率が測れないため明示する
    status = result["rule_status"]
    not_executed = [r for r in TARGET_RULES if status.get(r) != "executed"]

    print(
        f"{'rule':<38} {'期待':>3} {'検出':>3} {'再現率':>6}"
        f" {'FP(clean)':>9} {'未判定':>5}"
    )
    total_exp = total_det = 0
    for rule in TARGET_RULES:
        r = summary["per_rule"][rule]
        total_exp += r["expected"]
        total_det += r["detected"]
        mark = "" if status.get(rule) == "executed" else " *未実行"
        print(
            f"{rule:<38} {r['expected']:>3} {r['detected']:>3}"
            f" {pct(r['detected'], r['expected']):>6}"
            f" {r['fp_clean']:>9} {r['unlabeled']:>5}{mark}"
        )
    print(
        f"{'合計':<38} {total_exp:>3} {total_det:>3}"
        f" {pct(total_det, total_exp):>6}"
        f" {sum(r['fp_clean'] for r in summary['per_rule'].values()):>9}"
        f" {len(summary['unlabeled']):>5}"
    )
    if summary["other_fp_clean"]:
        print(
            "# 対象外ルールの clean 文書検出:",
            json.dumps(summary["other_fp_clean"], ensure_ascii=False),
        )
    if not_executed:
        print(
            f"# 注意: 未実行の対象ルール({', '.join(not_executed)})は再現率 n/a 相当です"
        )
    if summary["unlabeled"]:
        print("# 未判定の検出(注入文書上のラベル外):")
        for u in summary["unlabeled"]:
            print(f"#   {u['rule']} {u['file']}:{u['lines']}")

    if args.json:
        print(
            json.dumps(
                {
                    "binary": {"path": args.bin, "sha256": bin_id},
                    "script_revision": git_revision(),
                    "sets": [f"{s}.{k}={v}" for s, k, v in sets],
                    "jaio_toml": result["jaio_toml"],
                    "effective_config": result["effective_config"],
                    "config_fingerprint": result["config_fingerprint"],
                    "morphology": result["morphology"],
                    "targets": result["targets"],
                    "rule_status": status,
                    "labels": labels,
                    "summary": summary,
                },
                ensure_ascii=False,
                indent=2,
            )
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())
