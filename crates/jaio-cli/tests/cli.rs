//! CLI レベルの振る舞いテスト(実バイナリを実行する)。

use std::process::Command;

/// `--fix` は廃止された(#33、設計書 §7.4)。未知の引数として clap が
/// 終了コード 2 で拒否し、対象ファイルは一切変更されないことを確認する。
#[test]
fn fix_flag_is_rejected_without_modifying_files() {
    let dir = std::env::temp_dir().join(format!("jaio-fixflag-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("a.md");
    let original = "# 題名\n\n使用することができます。\n";
    std::fs::write(&file, original).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_jaio"))
        .args(["check", "--fix"])
        .arg(&dir)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(2),
        "--fix は終了コード 2 で拒否されるはず: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("--fix"),
        "エラーが --fix に言及するはず: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        original,
        "対象ファイルは変更されないはず"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// 読めないファイルが混ざると targets.skipped が増え、analysis_complete が
/// false、終了コード 2 になる(#36)。error 診断がなくてもクリーン扱いしない。
#[test]
fn unreadable_file_marks_analysis_incomplete() {
    let dir = std::env::temp_dir().join(format!("jaio-incomplete-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("ok.md"), "# 題名\n\n本文です。\n").unwrap();
    // 無効な UTF-8 で読み込み失敗を起こす
    std::fs::write(dir.join("bad.md"), b"\xff\xfe\x00bad").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_jaio"))
        .args(["check", "--format", "json"])
        .arg(&dir)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(2),
        "解析未完了は終了コード 2 のはず: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("JSON が出力されるはず");
    assert_eq!(report["summary"]["analysis_complete"], false);
    assert_eq!(report["context"]["targets"]["discovered"], 2);
    assert_eq!(report["context"]["targets"]["analyzed"], 1);
    assert_eq!(report["context"]["targets"]["skipped"], 1);
    assert_eq!(report["context"]["targets"]["discovery_complete"], true);
    let errors = report["context"]["operational_errors"]
        .as_array()
        .expect("operational_errors は配列のはず");
    assert!(
        errors.iter().any(|e| e["stage"] == "read"),
        "read 段階の失敗が記録されるはず: {errors:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// stats も check と同じ終了コード契約に乗る(#93)。読み込み失敗が混ざると
/// 解析未完了として exit 2 になり、JSON の operational_errors に記録される。
#[test]
fn stats_reports_incomplete_analysis_as_exit_2() {
    let dir = std::env::temp_dir().join(format!("jaio-statscode-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("ok.md"), "# 題名\n\n本文です。\n").unwrap();
    std::fs::write(dir.join("bad.md"), b"\xff\xfe\x00bad").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_jaio"))
        .args(["stats", "--format", "json"])
        .arg(&dir)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(2),
        "解析未完了の stats は終了コード 2 のはず: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("JSON が出力されるはず");
    assert_eq!(report["context"]["targets"]["skipped"], 1);
    let errors = report["context"]["operational_errors"]
        .as_array()
        .expect("operational_errors は配列のはず");
    assert!(
        errors.iter().any(|e| e["stage"] == "read"),
        "read 段階の失敗が記録されるはず: {errors:?}"
    );

    // 正常系(失敗なし)は exit 0 のままであることも確認する
    let ok = Command::new(env!("CARGO_BIN_EXE_jaio"))
        .args(["stats", "--format", "json"])
        .arg(dir.join("ok.md"))
        .output()
        .unwrap();
    assert_eq!(ok.status.code(), Some(0));
    std::fs::remove_dir_all(&dir).ok();
}

/// 正常系: analysis_complete=true、rules に全ルールの状態、
/// candidate_search が出る(#36)。少文書では全ペア採点になる。
#[test]
fn complete_run_reports_rule_statuses_and_candidate_search() {
    let dir = std::env::temp_dir().join(format!("jaio-complete-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.md"), "# 題名\n\n本文です。\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_jaio"))
        .args(["check", "--format", "json"])
        .arg(&dir)
        .output()
        .unwrap();
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("JSON が出力されるはず");
    assert_eq!(report["summary"]["analysis_complete"], true);
    assert_eq!(report["context"]["targets"]["analyzed"], 1);
    assert_eq!(report["context"]["targets"]["skipped"], 0);
    let rules = report["context"]["rules"]
        .as_array()
        .expect("rules は配列のはず");
    assert!(!rules.is_empty(), "全ルールの状態が出るはず");
    // 文書が 1 件だけなら全ドキュメントルールは executed
    assert!(
        rules
            .iter()
            .any(|r| r["rule_id"] == "efficiency/redundant-expression"
                && r["status"] == "executed"),
        "ドキュメントルールは executed のはず: {rules:?}"
    );
    // 比較可能な文書が 2 未満なら重複系コーパスルールは insufficient_inputs
    assert!(
        rules
            .iter()
            .any(|r| r["rule_id"] == "relation/cross-doc-duplication"
                && r["status"] == "insufficient_inputs"),
        "ペア前提ルールは insufficient_inputs のはず: {rules:?}"
    );
    let search = &report["context"]["candidate_search"];
    assert_eq!(search["method"], "all_pairs");
    assert_eq!(search["all_pairs"], true);
    assert!(search["cutoff"].is_number());
    std::fs::remove_dir_all(&dir).ok();
}

/// coverage の区別(#36): 「正常に 0 件」と「判定不能」を rules の
/// status・targets・measured で区別できる。
#[test]
fn rule_status_distinguishes_clean_zero_from_insufficient() {
    let dir = std::env::temp_dir().join(format!("jaio-coverage-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // 箇条書きだけの文書: 段落がなく文体判定は標本不足になるが、
    // 冗長表現や句読点の走査は完走する
    std::fs::write(dir.join("a.md"), "- 項目一\n- 項目二\n- 項目三\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_jaio"))
        .args(["check", "--format", "json"])
        .arg(&dir)
        .output()
        .unwrap();
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("JSON が出力されるはず");
    let rules = report["context"]["rules"]
        .as_array()
        .expect("rules は配列のはず");
    let find = |id: &str| {
        rules
            .iter()
            .find(|r| r["rule_id"] == id)
            .unwrap_or_else(|| panic!("{id} の状態があるはず: {rules:?}"))
    };
    // 判定不能: 文体判定可能な文がないので insufficient_inputs + 理由
    let style = find("consistency/style-deviation");
    assert_eq!(
        style["status"], "insufficient_inputs",
        "文体判定不能は insufficient_inputs のはず: {style}"
    );
    assert!(style["reason"].is_string(), "理由があるはず: {style}");
    assert_eq!(style["measured"], 0);
    // 正常に 0 件: リスト項目の走査は完走したので executed + targets>0
    let redundant = find("efficiency/redundant-expression");
    assert_eq!(
        redundant["status"], "executed",
        "走査完走なら executed のはず: {redundant}"
    );
    assert!(
        redundant["targets"].as_u64().unwrap_or(0) > 0,
        "対象リーフが記録されるはず: {redundant}"
    );
    // 適用対象なし: 見出しがない文書で targets=0 の executed
    let heading_skip = find("structure/heading-skip");
    assert_eq!(heading_skip["status"], "executed");
    assert_eq!(heading_skip["targets"], 0);
    assert_eq!(heading_skip["measured"], 0);
    std::fs::remove_dir_all(&dir).ok();
}

/// 抑制されたルールは実行せず、専用入力の取得もしない(#36)。
/// doc-comment-drift を抑制すればソース走査自体が起きないので、
/// 読めないソースがあっても operational error にならない。
#[test]
fn disabled_rules_skip_execution_and_input_acquisition() {
    let dir = std::env::temp_dir().join(format!("jaio-disabled-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.md"), "# 題名\n\n本文です。\n").unwrap();
    // 読めない(無効な UTF-8 の)rust ソース: コメント収集が走れば失敗する入力
    std::fs::write(dir.join("broken.rs"), b"// \xff\xfe\x00bad").unwrap();
    let config = dir.join("jaio.toml");
    std::fs::write(
        &config,
        "[lint.rules]\n\"relation/doc-comment-drift\" = \"allow\"\n\"efficiency/redundant-expression\" = \"allow\"\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_jaio"))
        .args(["check", "--format", "json", "--config"])
        .arg(&config)
        .arg(&dir)
        .output()
        .unwrap();
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("JSON が出力されるはず");
    let rules = report["context"]["rules"]
        .as_array()
        .expect("rules は配列のはず");
    for id in [
        "relation/doc-comment-drift",
        "efficiency/redundant-expression",
    ] {
        let status = rules
            .iter()
            .find(|r| r["rule_id"] == id)
            .unwrap_or_else(|| panic!("{id} の状態があるはず"));
        assert_eq!(status["status"], "disabled", "{id} は disabled のはず");
        // 実行していないので coverage は出さない
        assert!(status.get("targets").is_none(), "{id}: {status}");
        assert!(status.get("measured").is_none(), "{id}: {status}");
    }
    // コメント収集をしていないので .rs の読み込み失敗は記録されない
    let errors = report["context"]["operational_errors"]
        .as_array()
        .expect("operational_errors は配列のはず");
    assert!(
        !errors.iter().any(|e| e["stage"]
            .as_str()
            .is_some_and(|s| s.starts_with("comment"))),
        "コメント収集の失敗がないはず: {errors:?}"
    );
    let sources = report["context"]["sources"]
        .as_array()
        .expect("sources は配列のはず");
    assert!(
        !sources
            .iter()
            .any(|s| s["path"].as_str().is_some_and(|p| p.ends_with(".rs"))),
        "コメント収集をスキップしたので .rs は入力に含まれないはず: {sources:?}"
    );
    assert_eq!(report["summary"]["analysis_complete"], true);
    std::fs::remove_dir_all(&dir).ok();
}

/// lint.rules の不明なルール ID は起動時エラーにする(#36)。
#[test]
fn unknown_lint_rule_id_is_rejected() {
    let dir = std::env::temp_dir().join(format!("jaio-badrule-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.md"), "# 題名\n\n本文です。\n").unwrap();
    let config = dir.join("jaio.toml");
    std::fs::write(&config, "[lint.rules]\n\"no/such-rule\" = \"allow\"\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_jaio"))
        .args(["check", "--config"])
        .arg(&config)
        .arg(&dir)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no/such-rule"),
        "不明なルール ID に言及するはず: {stderr}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// comments.languages の未対応言語は起動時エラーにする(#36)。
/// typo を黙殺するとコメント 0 件として解析が進み、doc-comment-drift が
/// 「入力不足」の偽装をするため、設定エラーとして拒否する。
#[test]
fn unknown_comment_language_is_rejected() {
    let dir = std::env::temp_dir().join(format!("jaio-badlang-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.md"), "# 題名\n\n本文です。\n").unwrap();
    let config = dir.join("jaio.toml");
    std::fs::write(&config, "[comments]\nlanguages = [\"rusty\"]\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_jaio"))
        .args(["check", "--config"])
        .arg(&config)
        .arg(&dir)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("rusty"),
        "未対応言語名に言及するはず: {stderr}"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// ディレクトリ走査結果と別表記(`..` 経由)の明示指定が同一ファイルを指す
/// 場合でも、実体同一性で重複排除され、文書が二重解析・自己との重複ペアが
/// 報告されない(#44。文字列 dedup では `./docs/a.md` 相当の表記違いが残り、
/// a.md <-> a.md の 1.0 類似ペアが報告されていた)
#[test]
fn overlapping_inputs_do_not_create_self_duplicate() {
    let dir = std::env::temp_dir().join(format!("jaio-dedup-{}", std::process::id()));
    let docs = dir.join("docs");
    std::fs::create_dir_all(&docs).unwrap();
    // 互いに類似しない 2 文書(重複診断が出るなら自己ペア由来だけのはず)
    let body_a =
        "この文書は設定ファイルの読み込み処理を説明します。起動時に一度だけ読み込まれます。";
    let body_b = "この文書はデプロイ手順の運用記録です。日次の確認作業と週次の棚卸しを記載します。";
    std::fs::write(docs.join("a.md"), format!("# A\n\n{body_a}\n")).unwrap();
    std::fs::write(docs.join("b.md"), format!("# B\n\n{body_b}\n")).unwrap();
    std::fs::write(dir.join("jaio.toml"), "").unwrap();

    // `dir/docs/../docs/a.md` — 語彙上は別表記だが同一実体
    let alias = dir.join("docs").join("..").join("docs").join("a.md");
    let output = Command::new(env!("CARGO_BIN_EXE_jaio"))
        .args(["check", "--format", "json", "--config"])
        .arg(dir.join("jaio.toml"))
        .arg(&dir)
        .arg(&alias)
        .output()
        .unwrap();
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "JSON として解釈できるはず: {e}\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    assert_eq!(
        report["context"]["targets"]["analyzed"],
        2,
        "同一ファイルは重複排除され 2 文書になるはず: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let self_pairs: Vec<_> = report["diagnostics"]
        .as_array()
        .map(|d| {
            d.iter()
                .filter(|d| {
                    d["rule_id"].as_str().is_some_and(|id| {
                        id.contains("duplication") || id.contains("consolidation")
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    assert!(
        self_pairs.is_empty(),
        "自己との重複ペアは報告されないはず: {self_pairs:?}"
    );
    std::fs::remove_dir_all(&dir).ok();
}
