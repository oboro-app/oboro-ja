#!/usr/bin/env python3
"""eval-field.py の振る舞いテスト(#70)。バイナリ不要・標準ライブラリのみ。

jaio の実行(run_dump_corpus)は含めず、設定生成・出力検証・状態抽出・
ベースライン管理・照合の純ロジックを検証する。
"""

from __future__ import annotations

import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path

# ハイフン付きスクリプト名のため importlib で読み込む(test_eval_detection.py 流儀)
_spec = importlib.util.spec_from_file_location(
    "eval_field", Path(__file__).resolve().parent / "eval-field.py"
)
assert _spec is not None and _spec.loader is not None
ef = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(ef)


class BuildConfigTest(unittest.TestCase):
    def test_merges_default_and_sets(self) -> None:
        text = ef.build_config(["analysis.dictionary=/tmp/dict"])
        self.assertIn("[analysis]", text)
        self.assertIn('dictionary = "/tmp/dict"', text)
        # 既定の上書きも入る
        self.assertIn("[paths]", text)
        self.assertIn("include_hidden = true", text)

    def test_rejects_bare_key(self) -> None:
        with self.assertRaises(ef.EvalError):
            ef.build_config(["nosection"])
        with self.assertRaises(ef.EvalError):
            ef.build_config(["only_section."])


class RepoRelativeTest(unittest.TestCase):
    def test_normalizes_relative_and_absolute(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp) / "proj"
            (repo / "docs").mkdir(parents=True)
            f = repo / "docs" / "a.md"
            f.write_text("x", encoding="utf-8")
            by_abs = ef.repo_relative(str(f), repo)
            by_rel = ef.repo_relative("docs/a.md", repo)
            self.assertEqual(by_abs, "docs/a.md")
            self.assertEqual(by_rel, "docs/a.md")

    def test_outside_repo_is_absolute(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp) / "proj"
            repo.mkdir()
            outside = Path(tmp) / "other.md"
            outside.write_text("x", encoding="utf-8")
            self.assertTrue(ef.repo_relative(str(outside), repo).startswith("/"))


def minimal_report() -> dict:
    return {
        "kind": "jaio/dump-corpus",
        "version": 2,
        "docs": [
            {"file": "/r/docs/a.md", "tokens": 1, "body_chars": 1, "sections": []}
        ],
        "duplicates": [
            {
                "a": "/r/docs/a.md",
                "b": "/r/docs/b.md",
                "score": 0.9,
                "intersection": 5,
                "union": 6,
            }
        ],
        "context": {
            "targets": {
                "analyzed": 2,
                "discovered": 2,
                "skipped": 0,
                "discovery_complete": True,
            },
            "candidate_search": {"method": "all_pairs", "cutoff": 0.65},
            "operational_errors": [],
            "rules": [
                {"rule_id": "relation/cross-doc-duplication", "status": "executed"}
            ],
            "config_fingerprint": "sha256:x",
            "algorithms": {
                "morphology": "lindera",
                "tokenizer": "tiktoken o200k_base",
                "similarity": "simhash64+bigram-jaccard",
                "build": "jaio 0.7.2",
            },
            "sources": [
                {"path": "/r/docs/a.md", "sha256": "a" * 64},
                {"path": "/r/docs/b.md", "sha256": "b" * 64},
            ],
        },
    }


class ValidateReportTest(unittest.TestCase):
    def test_accepts_minimal_report(self) -> None:
        ef.validate_report(minimal_report())

    def test_rejects_wrong_kind_and_version(self) -> None:
        r = minimal_report()
        r["kind"] = "jaio/stats"
        with self.assertRaises(ef.EvalError):
            ef.validate_report(r)
        r = minimal_report()
        r["version"] = 99
        with self.assertRaises(ef.EvalError):
            ef.validate_report(r)

    def test_rejects_incomplete_scan_and_errors(self) -> None:
        r = minimal_report()
        r["context"]["targets"]["discovery_complete"] = False
        with self.assertRaises(ef.EvalError):
            ef.validate_report(r)
        r = minimal_report()
        r["context"]["targets"]["skipped"] = 1
        with self.assertRaises(ef.EvalError):
            ef.validate_report(r)
        r = minimal_report()
        r["context"]["operational_errors"] = [
            {"path": "x", "stage": "read", "reason": "boom"}
        ]
        with self.assertRaises(ef.EvalError):
            ef.validate_report(r)
        r = minimal_report()
        r["context"]["rules"][0]["status"] = "failed"
        with self.assertRaises(ef.EvalError):
            ef.validate_report(r)


class ExtractStateTest(unittest.TestCase):
    def test_extracts_docs_pairs_and_conditions(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            (repo / "docs").mkdir()
            for name in ("a.md", "b.md"):
                (repo / "docs" / name).write_text("x", encoding="utf-8")
            r = minimal_report()
            # 出力の file は実際の解析対象パス(repo 配下の絶対表記)に合わせる
            r["docs"][0]["file"] = str(repo / "docs" / "a.md")
            r["docs"].append(
                {
                    "file": str(repo / "docs" / "b.md"),
                    "tokens": 1,
                    "body_chars": 1,
                    "sections": [],
                }
            )
            r["duplicates"][0]["a"] = str(repo / "docs" / "a.md")
            r["duplicates"][0]["b"] = str(repo / "docs" / "b.md")
            # sources の path も repo 配下の絶対表記に合わせる(#70 残件 1)
            r["context"]["sources"][0]["path"] = str(repo / "docs" / "a.md")
            r["context"]["sources"][1]["path"] = str(repo / "docs" / "b.md")
            state = ef.extract_state(r, repo)
            self.assertEqual(state["fixed"]["docs"], ["docs/a.md", "docs/b.md"])
            self.assertEqual(state["pairs"], [["docs/a.md", "docs/b.md"]])
            self.assertEqual(
                state["fixed"]["candidate_search"],
                {"method": "all_pairs", "cutoff": 0.65},
            )
            self.assertEqual(state["variables"]["config_fingerprint"], "sha256:x")

    def test_rejects_duplicate_pairs(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            r = minimal_report()
            r["duplicates"].append(dict(r["duplicates"][0]))
            with self.assertRaises(ef.EvalError):
                ef.extract_state(r, Path(tmp))


class BaselineAndCompareTest(unittest.TestCase):
    def test_missing_baseline_is_error(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(ef.EvalError):
                ef.load_baseline(Path(tmp) / "none.json")

    def test_broken_or_incomplete_baseline_is_error(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp) / "b.json"
            p.write_text("{", encoding="utf-8")
            with self.assertRaises(ef.EvalError):
                ef.load_baseline(p)
            p.write_text(json.dumps({"fixed": {}}), encoding="utf-8")
            with self.assertRaises(ef.EvalError):
                ef.load_baseline(p)

    def baseline_state(self) -> dict:
        return {
            "fixed": {
                "docs": ["docs/a.md", "docs/b.md"],
                "doc_hashes": [
                    ["docs/a.md", "a" * 64],
                    ["docs/b.md", "b" * 64],
                ],
                "targets": {
                    "analyzed": 2,
                    "discovered": 2,
                    "skipped": 0,
                    "discovery_complete": True,
                },
                "candidate_search": {"method": "all_pairs", "cutoff": 0.65},
            },
            "variables": {
                "config_fingerprint": "sha256:x",
                "algorithms": {"morphology": "lindera"},
            },
            "pairs": [["docs/a.md", "docs/b.md"]],
        }

    def test_identical_state_passes(self) -> None:
        messages, failed = ef.compare(self.baseline_state(), self.baseline_state())
        self.assertFalse(failed)
        self.assertEqual(messages, [])

    def test_docs_change_fails(self) -> None:
        actual = self.baseline_state()
        actual["fixed"]["docs"] = ["docs/a.md"]
        messages, failed = ef.compare(self.baseline_state(), actual)
        self.assertTrue(failed)
        self.assertTrue(any("入力変化" in m for m in messages))

    def test_candidate_search_change_fails(self) -> None:
        actual = self.baseline_state()
        actual["fixed"]["candidate_search"]["cutoff"] = 0.5
        _, failed = ef.compare(self.baseline_state(), actual)
        self.assertTrue(failed)

    def test_missing_and_added_pairs_fail(self) -> None:
        actual = self.baseline_state()
        actual["pairs"] = [["docs/a.md", "docs/c.md"]]
        messages, failed = ef.compare(self.baseline_state(), actual)
        self.assertTrue(failed)
        self.assertTrue(any("消失ペア" in m for m in messages))
        self.assertTrue(any("新規ペア" in m for m in messages))

    def test_implementation_variables_do_not_fail(self) -> None:
        actual = self.baseline_state()
        actual["variables"]["config_fingerprint"] = "sha256:y"
        actual["variables"]["algorithms"]["morphology"] = "lindera-v7"
        messages, failed = ef.compare(self.baseline_state(), actual)
        self.assertFalse(failed)
        self.assertTrue(any("参考" in m for m in messages))


if __name__ == "__main__":
    unittest.main()
