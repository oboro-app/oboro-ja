//! 入口 crate の SCHEMA と JSON 出力を照合する(#9)。
//!
//! ## 入口の SCHEMA でレポート JSON を照合できる
//!
//! - 利用者: 入口 crate を組み込む側
//! - 目的: 出力が契約どおりか機械的に確かめる
//!
//! ```gherkin
//! Given 最小の JsonReport を入口の render_json で直列化した
//! When 入口の SCHEMA で照合する
//! Then スキーマ違反はない
//! ```

use oboro_ja::json::{
    JsonAlgorithms, JsonCandidateSearch, JsonContext, JsonReport, JsonRuleStatus, JsonSource,
    JsonSummary, JsonTargets, render_json,
};
use oboro_ja::{Diagnostic, SCHEMA, Span};

fn assert_valid(validator: &jsonschema::Validator, instance: &serde_json::Value) {
    let errors: Vec<String> = validator
        .iter_errors(instance)
        .map(|e| format!("{e} at {}", e.instance_path()))
        .collect();
    assert!(errors.is_empty(), "スキーマ違反:\n{}", errors.join("\n"));
}

fn sample_summary() -> JsonSummary {
    JsonSummary {
        files: 1,
        tokens: 42,
        errors: 0,
        warnings: 1,
        infos: 0,
        top_rules: Vec::new(),
        timings: None,
        slow_rules: None,
        analysis_complete: true,
        hints: Vec::new(),
    }
}

fn sample_context() -> JsonContext {
    JsonContext {
        roots: vec!["docs/".to_string()],
        config_fingerprint: format!("sha256:{}", "a".repeat(64)),
        sources: vec![JsonSource {
            path: "docs/a.md".to_string(),
            sha256: "b".repeat(64),
        }],
        algorithms: JsonAlgorithms {
            morphology: "lindera-sudachidict(mode=normal)".to_string(),
            tokenizer: "tiktoken o200k_base".to_string(),
            similarity: "simhash64+bigram-jaccard",
            build: "jaio 0.1.0 deps-sha256:abc".to_string(),
        },
        targets: JsonTargets {
            discovered: 1,
            analyzed: 1,
            skipped: 0,
            discovery_complete: true,
        },
        operational_errors: Vec::new(),
        rules: vec![JsonRuleStatus {
            suppressed_by_path: None,
            rule_id: "efficiency/redundant-expression",
            status: "executed",
            reason: None,
            targets: Some(4),
            measured: Some(4),
        }],
        candidate_search: JsonCandidateSearch {
            method: "all_pairs",
            all_pairs: true,
            comparable_docs: 1,
            candidates: 0,
            scored_pairs: 0,
            cutoff: 0.65,
        },
    }
}

/// 入口の render_json 出力が SCHEMA に適合する(#9)
#[test]
fn entry_render_json_validates_against_schema() {
    let summary = sample_summary();
    let context = sample_context();
    let diagnostics = [Diagnostic::new(
        "docs/a.md".to_string(),
        Span { line: 3, col: 5 },
        "efficiency/redundant-expression",
        "冗長表現: 「することができます」→「できます」".to_string(),
    )];
    let report = JsonReport::new(&summary, &diagnostics, &context, "0.1.0");
    let mut buf = Vec::new();
    render_json(&mut buf, &report).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&buf).expect("入口の JSON が読めるはず");

    let schema: serde_json::Value =
        serde_json::from_str(SCHEMA).expect("SCHEMA が JSON として正しいはず");
    let validator = jsonschema::validator_for(&schema).expect("SCHEMA がコンパイルできるはず");
    assert_valid(&validator, &value);
}

/// SCHEMA 定数が docs/schema/report.schema.json と同じ本文である(#9)
#[test]
fn schema_constant_matches_docs_file() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/schema/report.schema.json"
    );
    let from_file = std::fs::read_to_string(path).expect("正本スキーマが読めるはず");
    assert_eq!(SCHEMA, from_file);
}
