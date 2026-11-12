//! 実出力と指標オブジェクトを docs/schema/report.schema.json で検証する(#32)。
//! 手書きのキー検査ではなく、正本スキーマそのものをコンパイルして使う。

use std::process::Command;

fn load_schema() -> serde_json::Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/schema/report.schema.json"
    );
    let text = std::fs::read_to_string(path).expect("スキーマが読めるはず");
    serde_json::from_str(&text).expect("スキーマ自体が JSON として正しいはず")
}

/// 名前を指定して docs/schema/ のスキーマを読む(#61: stats / verify-move /
/// dump 系の契約も正本スキーマで機械保証する)
fn load_named_schema(name: &str) -> serde_json::Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/schema/");
    let text = std::fs::read_to_string(format!("{path}{name}"))
        .unwrap_or_else(|e| panic!("スキーマ {name} が読めるはず: {e}"));
    serde_json::from_str(&text).expect("スキーマ自体が JSON として正しいはず")
}

fn assert_valid(validator: &jsonschema::Validator, instance: &serde_json::Value) {
    let errors: Vec<String> = validator
        .iter_errors(instance)
        .map(|e| format!("{e} at {}", e.instance_path()))
        .collect();
    assert!(errors.is_empty(), "スキーマ違反:\n{}", errors.join("\n"));
}

/// `check --format json` の実出力全体がスキーマに適合する。
/// 3 件の近似複製文書で、measured(bigram_jaccard / token_delta)と
/// unavailable(estimated_token_reduction)の両状態を実際に含ませる。
#[test]
fn real_report_output_validates_against_schema() {
    let dir = std::env::temp_dir().join(format!("jaio-schema-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let body = "# 題名\n\n検査手順は最初に対象を洗い出し、\
                次に重複を計測してから統合方針を決める。\
                判断の記録は後で参照できるように残す。\n";
    // 3 件の近似文書で consolidation-plan(unavailable 計測)も発火させる
    std::fs::write(dir.join("a.md"), body).unwrap();
    std::fs::write(dir.join("b.md"), body.replace("検査手順", "確認手順")).unwrap();
    std::fs::write(dir.join("c.md"), body.replace("統合方針", "統合案")).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_jaio"))
        .args(["check", "--format", "json"])
        .arg(&dir)
        .output()
        .unwrap();
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("JSON が出力されるはず");

    let schema = load_schema();
    let validator = jsonschema::validator_for(&schema).expect("スキーマがコンパイルできるはず");
    assert_valid(&validator, &report);

    // 実出力が両状態を含んでいたことも確認する(空振り検証を防ぐ)
    let statuses: std::collections::BTreeSet<String> = report["diagnostics"]
        .as_array()
        .expect("diagnostics は配列のはず")
        .iter()
        .flat_map(|d| d["measurements"].as_array().cloned().unwrap_or_default())
        .filter_map(|m| m["status"].as_str().map(String::from))
        .collect();
    assert!(
        statuses.contains("measured"),
        "実出力に measured 計測があるはず: {statuses:?}"
    );
    assert!(
        statuses.contains("unavailable"),
        "実出力に unavailable 計測があるはず: {statuses:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// 利用ガイド(#37)の完全 JSON 例がスキーマに適合する。
/// ドキュメントと契約の乖離を機械的に検出する
#[test]
fn usage_guide_example_validates_against_schema() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/llm-usage.md");
    let text = std::fs::read_to_string(path).expect("利用ガイドが読めるはず");

    // ```json フェンスのうち schema_version を持つ完全なレポート例を拾う。
    // 複数ある場合は「最後の1つだけ」ではなく全て検証する(例が増えても
    // 検証対象が静かに入れ替わらないように)
    let mut found = Vec::new();
    let mut rest = text.as_str();
    while let Some(start) = rest.find("```json") {
        rest = &rest[start + "```json".len()..];
        let end = rest.find("```").expect("フェンスが閉じているはず");
        let block = &rest[..end];
        rest = &rest[end..];
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(block)
            && v.get("schema_version").is_some()
        {
            found.push(v);
        }
    }
    assert!(!found.is_empty(), "ガイドに完全なレポート例があるはず");

    let schema = load_schema();
    let validator = jsonschema::validator_for(&schema).expect("スキーマがコンパイルできるはず");
    for example in &found {
        assert_valid(&validator, example);
    }
}

/// measurement の状態別制約(#32): measured は有限 value 必須、
/// それ以外は value 禁止 + 空でない reason 必須。
#[test]
fn measurement_state_contract() {
    let schema = load_schema();
    let measurement_schema = &schema["$defs"]["measurement"];
    let validator =
        jsonschema::validator_for(measurement_schema).expect("計測スキーマがコンパイルできるはず");

    let measured_ok = serde_json::json!({
        "metric": "bigram_jaccard", "status": "measured", "value": 0.0
    });
    assert_valid(&validator, &measured_ok);

    let measured_missing_value = serde_json::json!({
        "metric": "bigram_jaccard", "status": "measured"
    });
    assert!(!validator.is_valid(&measured_missing_value));

    for status in ["not_applicable", "insufficient_sample", "unavailable"] {
        let ok = serde_json::json!({
            "metric": "token_delta", "status": status, "reason": "理由"
        });
        assert_valid(&validator, &ok);

        let with_value = serde_json::json!({
            "metric": "token_delta", "status": status, "reason": "理由", "value": 0.0
        });
        assert!(
            !validator.is_valid(&with_value),
            "{status} が value を持つのは違反のはず"
        );

        let missing_reason = serde_json::json!({
            "metric": "token_delta", "status": status
        });
        assert!(
            !validator.is_valid(&missing_reason),
            "{status} で reason なしは違反のはず"
        );

        let empty_reason = serde_json::json!({
            "metric": "token_delta", "status": status, "reason": ""
        });
        assert!(
            !validator.is_valid(&empty_reason),
            "{status} で空の reason は違反のはず"
        );
    }
}

/// `stats --format json --similarity` の実出力が stats.schema.json に適合する。
/// frontmatter の有無両方のファイルと分布データを実際に含ませる(空振り検証の防止)
#[test]
fn stats_json_output_validates_against_schema() {
    let dir = std::env::temp_dir().join(format!("jaio-stats-schema-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let body = "このセクションでは設定ファイルの読み込み処理について説明します。設定ファイルは起動時に一度だけ読み込まれ、変更は再起動後に反映されます。";
    // a.md は frontmatter 付き(SKILL.md 型)、b.md は無し(通常文書型)
    std::fs::write(
        dir.join("a.md"),
        format!("---\ndescription: 設定ファイルの読み込み処理を説明するスキルです。\n---\n\n# 題名A\n\n{body}\n"),
    )
    .unwrap();
    std::fs::write(dir.join("b.md"), format!("# 題名B\n\n{body}\n")).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_jaio"))
        .args(["stats", "--format", "json", "--similarity"])
        .arg(&dir)
        .output()
        .unwrap();
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("JSON が出力されるはず");

    let schema = load_named_schema("stats.schema.json");
    let validator = jsonschema::validator_for(&schema).expect("スキーマがコンパイルできるはず");
    assert_valid(&validator, &report);

    // frontmatter の null / 非 null の両方を含むこと(#53 の分岐を実際に通す)
    let nulls: Vec<bool> = report["files"]
        .as_array()
        .expect("files は配列のはず")
        .iter()
        .map(|f| f["frontmatter"].is_null())
        .collect();
    assert!(
        nulls.contains(&true) && nulls.contains(&false),
        "frontmatter 有無両方のファイルを含むはず: {nulls:?}"
    );
    assert!(
        !report["similarity"].is_null(),
        "--similarity 時は分布が出るはず"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// `verify-move --format json` の実出力が verify-move.schema.json に適合する。
/// 未保全行を実際に含ませる(空振り検証の防止)
#[test]
fn verify_move_json_output_validates_against_schema() {
    let dir = std::env::temp_dir().join(format!("jaio-vm-schema-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("before.md"),
        "# 手順\n\n一行目の説明です。\n二行目の説明です。\n",
    )
    .unwrap();
    std::fs::write(dir.join("after.md"), "### 手順\n\n一行目の説明です。\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_jaio"))
        .args(["verify-move", "--format", "json"])
        .arg(dir.join("before.md"))
        .arg(dir.join("after.md"))
        .output()
        .unwrap();
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("JSON が出力されるはず");

    let schema = load_named_schema("verify-move.schema.json");
    let validator = jsonschema::validator_for(&schema).expect("スキーマがコンパイルできるはず");
    assert_valid(&validator, &report);
    assert!(
        !report["lost_lines"].as_array().unwrap().is_empty(),
        "未保全行を含むはず"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// `dump doc` の実出力が dump-doc.schema.json に適合する(frontmatter 付き)
#[test]
fn dump_doc_output_validates_against_schema() {
    let dir = std::env::temp_dir().join(format!("jaio-dd-schema-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("skill.md"),
        "---\ndescription: 設定ファイルの読み込み処理を説明するスキルです。\n---\n\n# 題名\n\n本文です。[設定](./config.md) を参照します。\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_jaio"))
        .args(["dump", "doc"])
        .arg(dir.join("skill.md"))
        .output()
        .unwrap();
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("JSON が出力されるはず");

    let schema = load_named_schema("dump-doc.schema.json");
    let validator = jsonschema::validator_for(&schema).expect("スキーマがコンパイルできるはず");
    assert_valid(&validator, &report);
    assert!(!report["frontmatter"].is_null(), "frontmatter があるはず");
    assert!(
        !report["links"].as_array().unwrap().is_empty(),
        "リンクを含むはず"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// `dump morph` の実出力が dump-morph.schema.json に適合する
#[test]
fn dump_morph_output_validates_against_schema() {
    let dir = std::env::temp_dir().join(format!("jaio-dm-schema-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.md"), "# 題名\n\n設定ファイルを読み込みます。\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_jaio"))
        .args(["dump", "morph"])
        .arg(dir.join("a.md"))
        .output()
        .unwrap();
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("JSON が出力されるはず");

    let schema = load_named_schema("dump-morph.schema.json");
    let validator = jsonschema::validator_for(&schema).expect("スキーマがコンパイルできるはず");
    assert_valid(&validator, &report);
    assert!(
        !report["leaves"]
            .as_array()
            .unwrap()
            .iter()
            .all(|l| l["morphs"].as_array().unwrap().is_empty()),
        "形態素を含むはず"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// `dump corpus` の実出力が dump-corpus.schema.json に適合する。
/// 重複ペアを実際に含ませる(空振り検証の防止)。#61 の追加対応(レビューで
/// 対象からの漏れを発見)
#[test]
fn dump_corpus_output_validates_against_schema() {
    let dir = std::env::temp_dir().join(format!("jaio-dc-schema-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let body = "# 題名\n\nこのセクションでは設定ファイルの読み込み処理について説明します。設定ファイルは起動時に一度だけ読み込まれ、変更は再起動後に反映されます。運用時の注意も同じ場所にまとめて記載します。\n";
    std::fs::write(dir.join("a.md"), body).unwrap();
    std::fs::write(
        dir.join("b.md"),
        body.replace("設定ファイル", "構成ファイル"),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_jaio"))
        .args(["dump", "corpus"])
        .arg(&dir)
        .output()
        .unwrap();
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("JSON が出力されるはず");

    let schema = load_named_schema("dump-corpus.schema.json");
    let validator = jsonschema::validator_for(&schema).expect("スキーマがコンパイルできるはず");
    assert_valid(&validator, &report);
    assert!(
        !report["duplicates"].as_array().unwrap().is_empty(),
        "近似複製で重複ペアを含むはず"
    );
    assert!(
        report["context"]["rules"].as_array().unwrap().is_empty(),
        "dump はルールを実行しないため rules は空のはず"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// `stats --format json`(--similarity なし)の similarity が null になる
/// パスもスキーマに適合する
#[test]
fn stats_json_without_similarity_validates_against_schema() {
    let dir = std::env::temp_dir().join(format!("jaio-stats-ns-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.md"), "# 題名\n\n本文です。\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_jaio"))
        .args(["stats", "--format", "json"])
        .arg(&dir)
        .output()
        .unwrap();
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("JSON が出力されるはず");
    assert!(
        report["similarity"].is_null(),
        "--similarity なし時は null のはず"
    );

    let schema = load_named_schema("stats.schema.json");
    let validator = jsonschema::validator_for(&schema).expect("スキーマがコンパイルできるはず");
    assert_valid(&validator, &report);
    std::fs::remove_dir_all(&dir).ok();
}

/// orphan が「20 件超かつ解析成功文書数の 30% 超」になると summary.hints に
/// 設定案内が出る(#91)。実出力の summary 全体がスキーマにも適合することを
/// 合わせて確認する。
#[test]
fn orphan_dominant_corpus_emits_setting_hint() {
    let dir = std::env::temp_dir().join(format!("jaio-hint-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // 入口(README)1 + どこからも参照されない文書 21。orphan 21 / 22 文書 = 95%
    std::fs::write(dir.join("README.md"), "# README\n\n入口です。\n").unwrap();
    for i in 0..21 {
        let name = format!("orphan{i}.md");
        std::fs::write(dir.join(&name), format!("# {name}\n\n孤立した文書です。\n")).unwrap();
    }

    let output = Command::new(env!("CARGO_BIN_EXE_jaio"))
        .args(["check", "--format", "json"])
        .arg(&dir)
        .output()
        .unwrap();
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("JSON が出力されるはず");

    let hints = report["summary"]["hints"].as_array().expect("hints 配列");
    assert!(
        hints
            .iter()
            .any(|h| h.as_str().unwrap().contains("orphan_entry_points")),
        "orphan 大量検出時は設定案内が出るはず: {hints:?}"
    );

    let schema = load_schema();
    let validator = jsonschema::validator_for(&schema).expect("スキーマがコンパイルできるはず");
    assert_valid(&validator, &report);
    std::fs::remove_dir_all(&dir).ok();
}
