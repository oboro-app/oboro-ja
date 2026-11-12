#!/usr/bin/env python3
"""eval-detection.py の振る舞いテスト(#35)。

`jaio` バイナリ不要の軽量テストのみを収める(品質ゲート check-all.sh から
実行される)。バイナリ実行を伴う評価は eval-detection.py 本体で行う。

    python3 scripts/test_eval_detection.py
"""

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import tempfile
import tomllib
import unittest
import unittest.mock
from pathlib import Path

SCRIPT = Path(__file__).with_name("eval-detection.py")
spec = importlib.util.spec_from_file_location("eval_detection", SCRIPT)
eval_detection = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = eval_detection
spec.loader.exec_module(eval_detection)


class BuildJaioTomlTest(unittest.TestCase):
    """--set と既定値を統合した TOML 生成の検証(#35 再オープン対応)。"""

    def parse(self, floor: float, sets: list[tuple[str, str, str]]) -> dict:
        built = eval_detection.build_jaio_toml(floor, sets)
        # 同一テーブルの二重宣言は tomllib が拒否するため、パース成功自体が検証
        return tomllib.loads(str(built["toml"]))

    def test_set_into_existing_section_is_merged(self) -> None:
        # --set detection.comment_similarity=0.5 が [detection] を二重宣言しない
        parsed = self.parse(0.65, [("detection", "comment_similarity", "0.5")])
        self.assertEqual(parsed["detection"]["doc_similarity"], 0.65)
        self.assertEqual(parsed["detection"]["comment_similarity"], 0.5)
        self.assertEqual(parsed["merge"]["min_similarity"], 0.65)

    def test_set_bool_and_existing_section(self) -> None:
        # --set detection.all_pairs=true / --set merge.min_similarity=0.7
        parsed = self.parse(
            0.65,
            [("detection", "all_pairs", "true"), ("merge", "min_similarity", "0.7")],
        )
        self.assertIs(parsed["detection"]["all_pairs"], True)
        self.assertEqual(parsed["merge"]["min_similarity"], 0.7)

    def test_duplicate_key_last_wins(self) -> None:
        # 同一キーの重複指定は後勝ち(明示した規則)
        parsed = self.parse(
            0.65,
            [
                ("detection", "doc_similarity", "0.5"),
                ("detection", "doc_similarity", "0.55"),
            ],
        )
        self.assertEqual(parsed["detection"]["doc_similarity"], 0.55)

    def test_cutoff_above_floor_is_rejected(self) -> None:
        # 候補取得の下限が評価閾値の最小を上回る構成は取りこぼしを生むため拒否
        with self.assertRaises(eval_detection.EvalError):
            eval_detection.build_jaio_toml(
                0.65,
                [
                    ("detection", "doc_similarity", "0.9"),
                    ("merge", "min_similarity", "0.9"),
                ],
            )

    def test_raising_one_threshold_keeps_cutoff(self) -> None:
        # 片方だけ floor より高くても cutoff(低い方)が floor を維持すれば可
        parsed = self.parse(0.65, [("merge", "min_similarity", "0.9")])
        self.assertEqual(parsed["merge"]["min_similarity"], 0.9)

    def test_new_section_is_emitted_once(self) -> None:
        parsed = self.parse(0.65, [("paths", "include_hidden", "true")])
        self.assertIs(parsed["paths"]["include_hidden"], True)

    def test_bare_string_value_is_quoted(self) -> None:
        # --set analysis.dictionary=/path/to/dict のような裸文字列は
        # TOML リテラルとして解釈できないため文字列としてクォートする
        parsed = self.parse(0.65, [("analysis", "dictionary", "/path/to/dict")])
        self.assertEqual(parsed["analysis"]["dictionary"], "/path/to/dict")

    def test_quoted_and_array_values_pass_through(self) -> None:
        # 既に TOML リテラルとして有効な値はそのまま使う
        parsed = self.parse(
            0.65,
            [
                ("comments", "languages", '["rust", "python"]'),
                ("analysis", "dictionary", '"file:///d"'),
            ],
        )
        self.assertEqual(parsed["comments"]["languages"], ["rust", "python"])
        self.assertEqual(parsed["analysis"]["dictionary"], "file:///d")


class BuildCorpusTest(unittest.TestCase):
    """合成コーパス生成の制約(循環重複の禁止・近似例の実在性)を検証。"""

    def test_docs_above_combination_count_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(eval_detection.EvalError):
                eval_detection.build_corpus(Path(tmp), 106)

    def test_docs_below_minimum_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(eval_detection.EvalError):
                eval_detection.build_corpus(Path(tmp), 12)

    def test_generated_labels_are_non_empty(self) -> None:
        # replace/delete は元文書に存在する要素から生成される
        # (対象不在で未変更登録する経路は build_corpus 内の assert が防ぐ)
        # 13 文書では全基本文書ペアがトピックを共有し負例が空になるため、
        # 負例が出る規模(60 文書)で検証する
        with tempfile.TemporaryDirectory() as tmp:
            labels = eval_detection.build_corpus(Path(tmp), 60)
        self.assertEqual(len(labels["positives_exact"]), 3)
        for tier in eval_detection.NEAR_TIERS:
            self.assertEqual(len(labels["positives_near"][tier]), 3)
        self.assertGreater(len(labels["negatives"]), 0)


class RunDumpTest(unittest.TestCase):
    """dump 出力の検証(#35/#39): 契約を満たす完了済みレポートのみ受理し、
    異常終了・契約欠落・矛盾した完全性表示は全て拒否する。
    jaio バイナリは subprocess.run をモックして不要にする。

    正常例の fixture は実際の dump 契約(kind/version/context の必須項目・
    duplicates のペア形)を満たすものを使う。必須情報の欠落を正常例にしない
    """

    def complete_report(self) -> dict:
        """実際の dump corpus 契約を満たす正常レポートの fixture。"""
        return {
            "kind": "jaio/dump-corpus",
            "version": 2,
            "analysis_complete": True,
            "context": {
                "roots": ["."],
                "config_fingerprint": "sha256:abc",
                "sources": [
                    {"path": "a.md", "sha256": "01"},
                    {"path": "b.md", "sha256": "02"},
                ],
                "algorithms": {
                    "morphology": "lindera-sudachidict(mode=normal, dict=embedded:sudachidict)",
                    "tokenizer": "tiktoken o200k_base",
                    "similarity": "simhash64+bigram-jaccard",
                    "build": "jaio 0.1.0 deps-sha256:ff",
                },
                "targets": {
                    "discovered": 2,
                    "analyzed": 2,
                    "skipped": 0,
                    "discovery_complete": True,
                },
                "operational_errors": [],
                "rules": [],
                "candidate_search": {
                    "method": "all_pairs",
                    "all_pairs": True,
                    "comparable_docs": 2,
                    "candidates": 1,
                    "scored_pairs": 1,
                    "cutoff": 0.65,
                },
            },
            "docs": [
                {"file": "a.md", "tokens": 10, "body_chars": 20, "sections": []},
                {"file": "b.md", "tokens": 10, "body_chars": 20, "sections": []},
            ],
            "duplicates": [
                {"a": "a.md", "b": "b.md", "score": 0.8, "intersection": 4, "union": 5}
            ],
            "comments": [],
        }

    def run_dump(
        self,
        report: dict,
        returncode: int = 0,
        floor: float = 0.65,
        sets: list[tuple[str, str, str]] | None = None,
        md_files: list[str] | None = None,
    ) -> dict:
        proc = subprocess.CompletedProcess(
            args=[], returncode=returncode, stdout=json.dumps(report), stderr=""
        )
        self.argv = None
        self.config_text = None

        def capture(*a, **kw):
            self.argv = a[0]
            # 呼び出し時点で --config 実体の内容を保持(一時 dir は後で消える)
            if "--config" in self.argv:
                p = Path(self.argv[self.argv.index("--config") + 1])
                self.config_text = p.read_text(encoding="utf-8")
            return proc

        with tempfile.TemporaryDirectory() as tmp:
            for name in md_files or []:
                (Path(tmp) / name).write_text("# x\n", encoding="utf-8")
            with unittest.mock.patch.object(
                eval_detection.subprocess, "run", side_effect=capture
            ):
                return eval_detection.run_dump(
                    "jaio", Path(tmp), floor, sets or [], 60
                )

    def test_incomplete_analysis_is_rejected(self) -> None:
        # 運用失敗の部分結果(exit 2 + analysis_complete=false)を
        # 正常な成績として報告しない
        report = self.complete_report()
        report["analysis_complete"] = False
        report["context"]["operational_errors"] = [{"stage": "read", "reason": "x"}]
        report["context"]["targets"]["skipped"] = 1
        with self.assertRaises(eval_detection.EvalError):
            self.run_dump(report, returncode=2)

    def test_non_json_output_is_rejected(self) -> None:
        proc = subprocess.CompletedProcess(
            args=[], returncode=1, stdout="", stderr="boom"
        )
        with tempfile.TemporaryDirectory() as tmp:
            with unittest.mock.patch.object(
                eval_detection.subprocess, "run", return_value=proc
            ):
                with self.assertRaises(eval_detection.EvalError):
                    eval_detection.run_dump("jaio", Path(tmp), 0.65, [], 60)

    def test_complete_dump_yields_candidates_and_run_record(self) -> None:
        out = self.run_dump(self.complete_report())
        self.assertEqual(out["candidates"], {frozenset({"a.md", "b.md"}): 0.8})
        self.assertEqual(out["candidate_search"]["method"], "all_pairs")
        self.assertEqual(out["config_fingerprint"], "sha256:abc")

    def test_nonzero_exit_is_rejected_even_if_complete_json(self) -> None:
        # exit 2 + 正常形の JSON + analysis_complete=true でも、非ゼロ終了は
        # 評価データへ昇格させない(JSON は原因説明にのみ使う)
        with self.assertRaises(eval_detection.EvalError):
            self.run_dump(self.complete_report(), returncode=2)

    def test_zero_exit_with_incomplete_flag_is_rejected(self) -> None:
        # exit 0 でも analysis_complete=false なら受理しない
        # (exit code ゲートを素通りする系パターンで検証パスを潰さない)
        report = self.complete_report()
        report["analysis_complete"] = False
        with self.assertRaises(eval_detection.EvalError):
            self.run_dump(report, returncode=0)

    def test_contract_only_minimal_report_is_rejected(self) -> None:
        # {"analysis_complete": true} だけの出力は契約欠落として拒否する
        # (候補0件・完全解析として誤受理しない)
        with self.assertRaises(eval_detection.EvalError):
            self.run_dump({"analysis_complete": True})

    def test_foreign_kind_and_version_are_rejected(self) -> None:
        report = self.complete_report()
        report["kind"] = "jaio/dump-doc"
        with self.assertRaises(eval_detection.EvalError):
            self.run_dump(report)
        report = self.complete_report()
        report["version"] = 999
        with self.assertRaises(eval_detection.EvalError):
            self.run_dump(report)

    def test_contradictory_search_method_is_rejected(self) -> None:
        # method=simhash_banding なのに all_pairs=true(および逆)のような
        # 矛盾した候補列挙記録は受理しない
        for method, all_pairs in (
            ("simhash_banding", True),
            ("all_pairs", False),
        ):
            report = self.complete_report()
            report["context"]["candidate_search"]["method"] = method
            report["context"]["candidate_search"]["all_pairs"] = all_pairs
            with self.assertRaises(
                eval_detection.EvalError, msg=f"{method}/{all_pairs}"
            ):
                self.run_dump(report)

    def test_returned_cutoff_above_floor_is_rejected(self) -> None:
        # 要求 floor=0.65 に対し実行側 cutoff=0.9 では必要な候補が未取得
        report = self.complete_report()
        report["context"]["candidate_search"]["cutoff"] = 0.9
        with self.assertRaises(eval_detection.EvalError):
            self.run_dump(report, floor=0.65)

    def test_complete_but_operational_errors_is_rejected(self) -> None:
        # analysis_complete=true なのに operational_errors が非空は矛盾
        report = self.complete_report()
        report["context"]["operational_errors"] = [{"stage": "read", "reason": "x"}]
        with self.assertRaises(eval_detection.EvalError):
            self.run_dump(report)

    def test_invalid_pair_shapes_are_rejected(self) -> None:
        # 空・同一識別子・docs 非存在・範囲外 score・件数欠落のペアは拒否する
        for bad in (
            {"a": "", "b": "b.md", "score": 0.5, "intersection": 1, "union": 2},
            {"a": "a.md", "b": "a.md", "score": 0.5, "intersection": 1, "union": 2},
            {"a": "a.md", "b": "ghost.md", "score": 0.5, "intersection": 1, "union": 2},
            {"a": "a.md", "b": "b.md", "score": 1.5, "intersection": 1, "union": 2},
            {"a": "a.md", "b": "b.md", "score": "high", "intersection": 1, "union": 2},
            {"a": "a.md", "b": "b.md", "score": 0.5},
        ):
            report = self.complete_report()
            report["duplicates"] = [bad]
            with self.assertRaises(eval_detection.EvalError, msg=f"{bad}"):
                self.run_dump(report)

    def test_invalid_contract_element_shapes_are_rejected(self) -> None:
        # 消費しない項目も宣言契約の形を検証する(#35):
        # sources/docs/comments の要素形が崩れた出力は受理しない
        report = self.complete_report()
        report["context"]["sources"] = [{"path": "", "sha256": "01"}]
        with self.assertRaises(eval_detection.EvalError):
            self.run_dump(report)

        report = self.complete_report()
        report["docs"] = [{"file": "a.md", "tokens": -1, "body_chars": 1, "sections": []}]
        with self.assertRaises(eval_detection.EvalError):
            self.run_dump(report)

        report = self.complete_report()
        report["docs"][0]["sections"] = [{"line": 0, "title": "x"}]
        with self.assertRaises(eval_detection.EvalError):
            self.run_dump(report)

        report = self.complete_report()
        report["comments"] = [{"file": "x.rs"}]
        with self.assertRaises(eval_detection.EvalError):
            self.run_dump(report)

    def test_exact_reference_requires_all_pairs_scored(self) -> None:
        # --set detection.all_pairs=true の exact 参照評価では、
        # 全ペア採点されたことまで検証する
        sets = [("detection", "all_pairs", "true")]
        report = self.complete_report()
        report["context"]["candidate_search"]["method"] = "simhash_banding"
        report["context"]["candidate_search"]["all_pairs"] = False
        with self.assertRaises(eval_detection.EvalError):
            self.run_dump(report, sets=sets)
        # 全ペア方式でも採点数が C(n,2) と一致しなければ拒否
        report = self.complete_report()
        report["context"]["candidate_search"]["scored_pairs"] = 0
        with self.assertRaises(eval_detection.EvalError):
            self.run_dump(report, sets=sets)
        # 正しい全ペア結果は受理する(近似的でない列挙は失敗ではない)
        out = self.run_dump(self.complete_report(), sets=sets)
        self.assertEqual(out["candidate_search"]["method"], "all_pairs")

    def test_approximate_method_is_not_an_operational_failure(self) -> None:
        # 通常評価(exact 要求なし)では simhash_banding も受理する。
        # 近似検索自体を運用エラーとは扱わない
        report = self.complete_report()
        report["context"]["candidate_search"].update(
            {
                "method": "simhash_banding",
                "all_pairs": False,
                "comparable_docs": 2,
                "candidates": 1,
                "scored_pairs": 1,
            }
        )
        out = self.run_dump(report)
        self.assertEqual(out["candidate_search"]["method"], "simhash_banding")

    def test_generated_config_is_passed_via_config_flag(self) -> None:
        # #65: JAIO_CONFIG の環境混入を防ぐため生成 jaio.toml を --config で
        # 明示する(優先順位の最上位)。env に競合設定があっても argv は不変
        import os

        with unittest.mock.patch.dict(
            os.environ, {"JAIO_CONFIG": '[paths]\nexclude = ["**/*.md"]'}
        ):
            self.run_dump(self.complete_report())
        self.assertIsNotNone(self.argv)
        self.assertIn("--config", self.argv)
        config_arg = Path(self.argv[self.argv.index("--config") + 1])
        self.assertEqual(config_arg.name, "jaio.toml")
        # 生成ファイル実体を指し、内容は評価用 TOML(閾値が含まれる)
        self.assertIsNotNone(self.config_text)
        self.assertIn("doc_similarity", self.config_text)

    def test_missing_generated_doc_is_rejected(self) -> None:
        # #65: 外部設定で文書が除外されても欠落を成績として受理しない
        # (fixture の sources は a.md/b.md、コーパスには c.md も存在する)
        with self.assertRaises(eval_detection.EvalError):
            self.run_dump(
                self.complete_report(), md_files=["a.md", "b.md", "c.md"]
            )

    def test_binary_identification_is_separate_from_script_revision(self) -> None:
        # 実行バイナリの識別(実体 hash)とスクリプトの Git revision は別物
        with tempfile.TemporaryDirectory() as tmp:
            fake = Path(tmp) / "jaio"
            fake.write_bytes(b"fake-binary")
            self.assertEqual(
                eval_detection.binary_sha256(str(fake)),
                __import__("hashlib").sha256(b"fake-binary").hexdigest(),
            )
        self.assertEqual(eval_detection.binary_sha256("/nonexistent/bin"), "unknown")


class SummarizeAtTest(unittest.TestCase):
    """ラベル別集計: 未判定検出は FP に数えず、分母 0 は n/a 扱い。"""

    def labels(self) -> dict[str, object]:
        return {
            "positives_exact": {frozenset({"a", "b"})},
            "positives_near": {"append": {frozenset({"c", "d"})}},
            "negatives": {frozenset({"e", "f"})},
        }

    def test_unlabeled_detection_is_not_fp(self) -> None:
        candidates = {
            frozenset({"a", "b"}): 0.9,   # 正例を検出
            frozenset({"x", "y"}): 0.8,   # 未判定ペアの検出(FP にしない)
        }
        r = eval_detection.summarize_at(candidates, self.labels(), 0.65)
        self.assertEqual(r["tp"], 1)
        self.assertEqual(r["fp"], 0)
        self.assertEqual(r["unlabeled_detections"], 1)

    def test_zero_denominator_is_not_forced_to_100(self) -> None:
        self.assertEqual(eval_detection.pct(0, 0), "n/a")


if __name__ == "__main__":
    unittest.main()
