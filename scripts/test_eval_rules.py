#!/usr/bin/env python3
"""eval-rules.py の振る舞いテスト(#62)。

`jaio` バイナリ不要の軽量テストのみを収める(品質ゲート check-all.sh から
実行される)。バイナリ実行を伴う評価は eval-rules.py 本体で行う。

    python3 scripts/test_eval_rules.py
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

SCRIPT = Path(__file__).with_name("eval-rules.py")
spec = importlib.util.spec_from_file_location("eval_rules", SCRIPT)
eval_rules = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = eval_rules
spec.loader.exec_module(eval_rules)


class BuildRuleCorpusTest(unittest.TestCase):
    """ラベル付き合成コーパス生成の制約を検証する。"""

    def build(self) -> tuple[dict, Path]:
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        corpus = Path(tmp.name)
        return eval_rules.build_rule_corpus(corpus), corpus

    def test_every_target_rule_has_a_label(self) -> None:
        # 対象ルール全てに正例ラベルがある(再現率の分母が 0 にならない)
        built, _ = self.build()
        labeled = {lb["rule"] for lb in built["labels"]}
        self.assertEqual(labeled, set(eval_rules.TARGET_RULES))

    def test_label_lines_point_into_their_files(self) -> None:
        # 受理行はファイル内の実在行を指す(貼り忘れ・行ズレの検出)
        built, corpus = self.build()
        for lb in built["labels"]:
            path = corpus / lb["file"]
            n_lines = len(path.read_text(encoding="utf-8").splitlines())
            for line in lb["lines"]:
                self.assertGreaterEqual(line, 1)
                self.assertLessEqual(line, n_lines)

    def test_clean_and_defect_files_are_disjoint(self) -> None:
        built, _ = self.build()
        defect_files = {lb["file"] for lb in built["labels"]}
        self.assertTrue(built["clean_files"].isdisjoint(defect_files))
        # クリーン 5 文書 + 境界(ハードネガティブ)2 文書(#70)
        self.assertEqual(len(built["clean_files"]), 7)
        self.assertIn("hardneg-expr.md", built["clean_files"])
        self.assertIn("hardneg-punct.md", built["clean_files"])

    def test_corpus_is_deterministic(self) -> None:
        # 同一入力で同一ラベル・同一文書列(計測の再現性)
        first, c1 = self.build()
        second, c2 = self.build()
        self.assertEqual(first["labels"], second["labels"])
        for lb in first["labels"]:
            self.assertEqual(
                (c1 / lb["file"]).read_bytes(), (c2 / lb["file"]).read_bytes()
            )


class BuildJaioTomlTest(unittest.TestCase):
    """評価コーパス用 jaio.toml 生成の検証。"""

    def parse(self, sets: list[tuple[str, str, str]]) -> dict:
        return tomllib.loads(str(eval_rules.build_jaio_toml(sets)["toml"]))

    def test_default_enables_opt_in_particle_rule(self) -> None:
        # consistency/particle は opt-in のため既定構成でペアを有効化する
        parsed = self.parse([])
        self.assertEqual(parsed["detection"]["particle_pairs"], [["に", "へ"]])

    def test_default_excludes_orphan_from_corpus(self) -> None:
        # リンクを張らない合成コーパスで orphan が全件発火しないよう対象外にする
        parsed = self.parse([])
        self.assertIn("*.md", parsed["detection"]["orphan_entry_points"])

    def test_set_is_merged_without_duplicate_section(self) -> None:
        parsed = self.parse([("detection", "long_sentence_min_chars", "40")])
        self.assertEqual(parsed["detection"]["long_sentence_min_chars"], 40)
        self.assertEqual(parsed["detection"]["particle_pairs"], [["に", "へ"]])

    def test_bare_string_value_is_quoted(self) -> None:
        parsed = self.parse([("analysis", "dictionary", "/path/to/dict")])
        self.assertEqual(parsed["analysis"]["dictionary"], "/path/to/dict")


class RunCheckTest(unittest.TestCase):
    """check 出力の契約検証: 完了済み・契約適合の報告のみ受理する。"""

    def complete_report(self) -> dict:
        """実際の check --format json 契約を満たす正常レポートの fixture。"""
        return {
            "schema_version": 2,
            "tool": {"name": "jaio", "version": "0.4.0"},
            "summary": {"analysis_complete": True, "diagnostics": 1},
            "context": {
                "config_fingerprint": "sha256:abc",
                "algorithms": {"morphology": "lindera-sudachidict(mode=normal)"},
                "targets": {
                    "discovered": 1,
                    "analyzed": 1,
                    "skipped": 0,
                    "discovery_complete": True,
                },
                "operational_errors": [],
                "rules": [{"rule_id": "efficiency/filler", "status": "executed"}],
                "sources": [{"path": "def-filler.md", "sha256": "aa"}],
            },
            "diagnostics": [
                {
                    "file": "./def-filler.md",
                    "rule_id": "efficiency/filler",
                    "severity": "warn",
                    "span": {"line": 5, "col": 1},
                    "locations": [
                        {"file": "./def-filler.md", "span": {"line": 7, "col": 1}}
                    ],
                }
            ],
        }

    def run_check(
        self,
        report: object,
        returncode: int = 0,
        md_files: list[str] | None = None,
    ) -> dict:
        stdout = report if isinstance(report, str) else json.dumps(report)
        proc = subprocess.CompletedProcess(
            args=[], returncode=returncode, stdout=stdout, stderr=""
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
                eval_rules.subprocess, "run", side_effect=capture
            ):
                return eval_rules.run_check("jaio", Path(tmp), [], 60)

    def test_complete_report_is_accepted(self) -> None:
        out = self.run_check(self.complete_report())
        self.assertEqual(out["config_fingerprint"], "sha256:abc")
        # span + locations の参照行が診断の行集合に畳まれる
        self.assertEqual(out["diagnostics"][0]["lines"], {5, 7})
        self.assertEqual(out["rule_status"], {"efficiency/filler": "executed"})

    def test_exit_1_is_accepted_as_diagnostics_present(self) -> None:
        # error 重大度の診断を含む正常報告(exit 1)は受理する
        out = self.run_check(self.complete_report(), returncode=1)
        self.assertEqual(len(out["diagnostics"]), 1)

    def test_exit_2_is_rejected_even_if_complete_json(self) -> None:
        with self.assertRaises(eval_rules.EvalError):
            self.run_check(self.complete_report(), returncode=2)

    def test_non_json_output_is_rejected(self) -> None:
        with self.assertRaises(eval_rules.EvalError):
            self.run_check("not json", returncode=0)

    def test_wrong_schema_version_is_rejected(self) -> None:
        report = self.complete_report()
        report["schema_version"] = 1
        with self.assertRaises(eval_rules.EvalError):
            self.run_check(report)

    def test_incomplete_analysis_is_rejected(self) -> None:
        report = self.complete_report()
        report["summary"]["analysis_complete"] = False
        with self.assertRaises(eval_rules.EvalError):
            self.run_check(report)

    def test_operational_errors_with_complete_flag_is_rejected(self) -> None:
        report = self.complete_report()
        report["context"]["operational_errors"] = [{"stage": "read"}]
        with self.assertRaises(eval_rules.EvalError):
            self.run_check(report)

    def test_inconsistent_targets_are_rejected(self) -> None:
        # analyzed+skipped != discovered や skipped 残留は受理しない
        report = self.complete_report()
        report["context"]["targets"]["skipped"] = 1
        with self.assertRaises(eval_rules.EvalError):
            self.run_check(report)
        report = self.complete_report()
        report["context"]["targets"]["discovered"] = 5
        with self.assertRaises(eval_rules.EvalError):
            self.run_check(report)

    def test_malformed_diagnostic_is_rejected(self) -> None:
        report = self.complete_report()
        report["diagnostics"] = [{"file": "a.md", "rule_id": "x"}]
        with self.assertRaises(eval_rules.EvalError):
            self.run_check(report)

    def test_generated_config_is_passed_via_config_flag(self) -> None:
        # #65: JAIO_CONFIG の環境混入を防ぐため生成 jaio.toml を --config で
        # 明示する(優先順位の最上位)
        import os

        with unittest.mock.patch.dict(
            os.environ, {"JAIO_CONFIG": '[paths]\nexclude = ["**/*.md"]'}
        ):
            self.run_check(self.complete_report())
        self.assertIsNotNone(self.argv)
        self.assertIn("--config", self.argv)
        config_arg = Path(self.argv[self.argv.index("--config") + 1])
        self.assertEqual(config_arg.name, "jaio.toml")
        # 生成ファイル実体を指す(内容が評価用 TOML である)
        self.assertIsNotNone(self.config_text)
        self.assertIn("particle_pairs", self.config_text)

    def test_missing_generated_doc_is_rejected(self) -> None:
        # #65: 外部設定で文書が除外されても欠落を成績として受理しない
        report = self.complete_report()  # sources は def-filler.md のみ
        with self.assertRaises(eval_rules.EvalError):
            self.run_check(report, md_files=["def-filler.md", "clean00.md"])


class SummarizeTest(unittest.TestCase):
    """ラベル照合: 行一致・文書一致・クリーン文書 FP・未判定の扱い。"""

    def test_line_match_and_unlabeled_separation(self) -> None:
        labels = [
            {"rule": "efficiency/filler", "file": "d.md", "lines": [5], "match": "line"}
        ]
        diags = [
            {"rule": "efficiency/filler", "file": "d.md", "lines": {5}},
            # 注入文書上のラベル外診断は FP でなく未判定
            {"rule": "consistency/terminology", "file": "d.md", "lines": {9}},
            # クリーン文書上の診断は FP
            {"rule": "efficiency/filler", "file": "clean00.md", "lines": {3}},
        ]
        r = eval_rules.summarize(diags, labels, {"clean00.md"})
        self.assertEqual(r["per_rule"]["efficiency/filler"]["detected"], 1)
        self.assertEqual(r["per_rule"]["efficiency/filler"]["fp_clean"], 1)
        self.assertEqual(r["per_rule"]["consistency/terminology"]["unlabeled"], 1)

    def test_file_match_accepts_any_line(self) -> None:
        labels = [{"rule": "r/x", "file": "d.md", "lines": [], "match": "file"}]
        diags = [{"rule": "r/x", "file": "d.md", "lines": {42}}]
        r = eval_rules.summarize(diags, labels, set())
        # r/x は対象外ルールなので per_rule には出ないが、未判定にも残らない
        self.assertEqual(r["unlabeled"], [])

    def test_missed_label_is_reported(self) -> None:
        labels = [
            {"rule": "efficiency/filler", "file": "d.md", "lines": [5], "match": "line"}
        ]
        r = eval_rules.summarize([], labels, set())
        self.assertEqual(r["per_rule"]["efficiency/filler"]["detected"], 0)
        self.assertEqual(
            r["per_rule"]["efficiency/filler"]["missed_labels"],
            [{"file": "d.md", "lines": [5]}],
        )

    def test_dot_slash_prefix_is_normalized(self) -> None:
        labels = [
            {"rule": "efficiency/filler", "file": "d.md", "lines": [5], "match": "line"}
        ]
        diags = [{"rule": "efficiency/filler", "file": "./d.md", "lines": {5}}]
        r = eval_rules.summarize(diags, labels, set())
        self.assertEqual(r["per_rule"]["efficiency/filler"]["detected"], 1)

    def test_zero_denominator_is_not_forced_to_100(self) -> None:
        self.assertEqual(eval_rules.pct(0, 0), "n/a")


if __name__ == "__main__":
    unittest.main()
