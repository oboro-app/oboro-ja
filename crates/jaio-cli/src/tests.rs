use super::*;
use jaio_core::Span;

#[test]
fn normalizes_leading_dot_slash() {
    assert_eq!(normalize_display(Path::new("./docs/a.md")), "docs/a.md");
    assert_eq!(normalize_display(Path::new("docs/a.md")), "docs/a.md");
    assert_eq!(
        normalize_display(Path::new("crates/jaio-cli/../jaio-rules/a.md")),
        "crates/jaio-rules/a.md"
    );
    // 解析ルートを超える `..` は潜ったまま残す
    assert_eq!(normalize_display(Path::new("../x.md")), "../x.md");
    assert_eq!(normalize_display(Path::new("a/../../x.md")), "../x.md");
}

#[test]
fn summarize_counts_severities() {
    let diag = |severity: Severity| {
        Diagnostic::new(
            "a.md".to_string(),
            Span { line: 1, col: 1 },
            "r/x",
            "m".to_string(),
        )
        .severity(severity)
    };
    let summary = summarize(
        2,
        10,
        &[
            diag(Severity::Error),
            diag(Severity::Warn),
            diag(Severity::Warn),
            diag(Severity::Info),
        ],
        None,
        true,
    );
    assert_eq!((summary.errors, summary.warnings, summary.infos), (1, 2, 1));
}

#[test]
fn config_fingerprint_is_deterministic_and_tracks_changes() {
    // HashMap フィールドを含んでも正規化されている(実行間で同じ)
    let toml = "[lint.rules]\n\"b/x\" = \"info\"\n\"a/y\" = \"allow\"\n";
    let a = Config::from_toml(toml).unwrap();
    let b = Config::from_toml(toml).unwrap();
    assert_eq!(config_fingerprint(&a), config_fingerprint(&b));
    assert!(config_fingerprint(&a).starts_with("sha256:"));
    // 実効値が変わればハッシュも変わる
    let changed = Config::from_toml("[detection]\ndoc_similarity = 0.9\n").unwrap();
    assert_ne!(config_fingerprint(&a), config_fingerprint(&changed));
}

#[test]
fn summarize_verbose_timings_ranked_and_capped() {
    // --verbose 相当の計測を渡すと slow_rules は所要時間降順・上位 5 件になり、
    // ターミナルの summary にも計測行が出る(Issue #18)
    let mut rules: HashMap<&'static str, (u64, usize)> = HashMap::new();
    for (id, ms, diags) in [
        ("consistency/style-deviation", 3120u64, 255usize),
        ("relation/orphan", 90, 101),
        ("relation/a", 5000, 1),
        ("relation/b", 4000, 2),
        ("relation/c", 3000, 3),
        ("relation/d", 2000, 4),
        ("relation/e", 1000, 5),
    ] {
        rules.insert(id, (ms, diags));
    }
    let timings = RunTimings {
        collect_ms: 12,
        parse_ms: 6210,
        corpus_ms: 640,
        rules,
    };
    let summary = summarize(1, 100, &[], Some(&timings), true);
    assert_eq!(
        summary.timings,
        Some(PhaseTimings {
            collect_ms: 12,
            parse_ms: 6210,
            corpus_ms: 640
        })
    );
    // 7 ルール → 所要時間降順の上位 5 件に切り詰め(1000ms の relation/e は落ちる)
    assert_eq!(
        summary.slow_rules,
        Some(vec![
            ("relation/a".to_string(), 5000, 1),
            ("relation/b".to_string(), 4000, 2),
            ("consistency/style-deviation".to_string(), 3120, 255),
            ("relation/c".to_string(), 3000, 3),
            ("relation/d".to_string(), 2000, 4),
        ])
    );
    let mut buf = Vec::new();
    write_check_summary(&mut buf, &summary).unwrap();
    let out = String::from_utf8(buf).unwrap();
    assert!(
        out.contains("timings: collect 12ms, parse 6210ms, corpus 640ms"),
        "{out}"
    );
    assert!(
        out.contains("slow rules: relation/a 5000ms (1 diags), relation/b 4000ms (2 diags)"),
        "{out}"
    );

    // 未指定(None)なら計測行も JSON キーも出さない(従来出力と同じ)
    let plain = summarize(1, 100, &[], None, true);
    assert!(plain.timings.is_none());
    assert!(plain.slow_rules.is_none());
    let mut buf = Vec::new();
    write_check_summary(&mut buf, &plain).unwrap();
    let out = String::from_utf8(buf).unwrap();
    assert!(!out.contains("timings:"), "{out}");
    assert!(!out.contains("slow rules:"), "{out}");
    let json = serde_json::to_value(&plain).unwrap();
    assert!(json.get("timings").is_none());
    assert!(json.get("slow_rules").is_none());
}

#[test]
fn quiet_filters_non_error_diagnostics() -> Result<()> {
    // fixture の診断は warn/info 系のため quiet では報告・集計から外れる
    let config = Config::default();
    let mut buf = Vec::new();
    let summary = run_check(
        &[PathBuf::from("../jaio-rules/tests/fixtures/sample.md")],
        &mut buf,
        &config,
        &OutputFormat::Json,
        true,
        false,
    )?;
    assert_eq!((summary.errors, summary.warnings, summary.infos), (0, 0, 0));
    let value: serde_json::Value = serde_json::from_slice(&buf)?;
    assert_eq!(value["diagnostics"].as_array().unwrap().len(), 0);
    Ok(())
}

#[test]
fn collect_respects_exclude() -> Result<()> {
    // cwd 内に一時ツリーを作る(テストの cwd は crates/jaio-cli。
    // cwd 外は照合不能 = 許可の扱いになるため、解析ルート内で検証する)
    let cwd = env::current_dir()?;
    let root = cwd.join("target").join("jaio-collect-exclude-test");
    let docs = root.join("docs");
    let templates = root.join("templates");
    fs::create_dir_all(&docs)?;
    fs::create_dir_all(&templates)?;
    fs::write(docs.join("page.md"), "# ページ\n\n本文です。\n")?;
    fs::write(templates.join("draft.md"), "# 下書き\n\n本文です。\n")?;

    let config = Config::from_toml("[paths]\nexclude = [\"templates/\"]")?;
    // 解析ルートを走査: exclude が効く(従来経路)
    let files = collect_md_files(std::slice::from_ref(&root), &config)?;
    assert!(
        files.iter().all(|f| !f.ends_with("draft.md")),
        "exclude に一致するパスが混入している: {files:?}"
    );
    // サブディレクトリを直接指定しても解析ルート相対の exclude が効く(#49)
    let files = collect_md_files(std::slice::from_ref(&templates), &config)?;
    assert!(
        files.is_empty(),
        "サブディレクトリ指定でも除外されるはず: {files:?}"
    );
    // exclude 外のサブディレクトリは従来どおり収集される
    let files = collect_md_files(std::slice::from_ref(&docs), &config)?;
    assert_eq!(files.len(), 1, "除外対象外は収集されるはず: {files:?}");
    fs::remove_dir_all(&root).ok();
    Ok(())
}

#[test]
fn json_output_is_valid() -> Result<()> {
    let config = Config::default();
    let mut buf = Vec::new();
    let summary = run_check(
        &[PathBuf::from("../jaio-rules/tests/fixtures/sample.md")],
        &mut buf,
        &config,
        &OutputFormat::Json,
        false,
        false,
    )?;
    assert_eq!(summary.files, 1);
    let value: serde_json::Value = serde_json::from_slice(&buf)?;
    assert_eq!(value["schema_version"], 2);
    assert_eq!(value["tool"]["name"], "jaio");
    assert!(value["summary"].is_object());
    // 実行コンテキスト(§7.3): roots・config ハッシュ・ソース hash・算法識別
    let context = &value["context"];
    assert_eq!(
        context["roots"],
        serde_json::json!(["../jaio-rules/tests/fixtures/sample.md"])
    );
    assert!(
        context["config_fingerprint"]
            .as_str()
            .is_some_and(|f| f.starts_with("sha256:")),
        "config_fingerprint は sha256: プレフィックス付きのはず"
    );
    assert_eq!(
        context["algorithms"]["similarity"],
        "simhash64+bigram-jaccard"
    );
    let sources = context["sources"].as_array().expect("sources は配列");
    assert_eq!(sources.len(), 1);
    assert!(
        sources[0]["sha256"]
            .as_str()
            .is_some_and(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())),
        "sources[].sha256 は hex のはず: {}",
        sources[0]
    );
    assert!(
        value["diagnostics"]
            .as_array()
            .is_some_and(|a| !a.is_empty())
    );
    // 診断には rule_id のカテゴリが付く(§7.3)
    assert!(value["diagnostics"][0]["category"].is_string());
    // docs/schema/report.schema.json の主要制約を機械検証する(#25)。
    // 全診断が必須フィールドと列挙値を満たすこと
    let categories = [
        "structure",
        "efficiency",
        "consistency",
        "relation",
        "merge",
    ];
    // JSON の severity は serde 名(warn)。terminal 表示の warning とは異なる
    let severities = ["error", "warn", "info"];
    let measurement_statuses = [
        "measured",
        "not_applicable",
        "insufficient_sample",
        "unavailable",
    ];
    for d in value["diagnostics"].as_array().expect("diagnostics は配列") {
        for key in ["file", "rule_id", "observation"] {
            assert!(d[key].is_string(), "必須フィールド欠損: {key}");
        }
        let severity = d["severity"].as_str().expect("severity は文字列");
        assert!(
            severities.contains(&severity),
            "不正な severity: {severity}"
        );
        let category = d["category"].as_str().expect("category は文字列");
        assert!(
            categories.contains(&category),
            "不正な category: {category}"
        );
        let span = &d["span"];
        assert!(span["line"].is_u64() && span["col"].is_u64(), "span は数値");
        // v1 のフィールドは出さない(#32/#33)
        for removed in ["message", "help", "evidence", "fixable"] {
            assert!(d.get(removed).is_none(), "v1 フィールド残存: {removed}");
        }
        // measurements の status は列挙値内、measured 以外は value を出さない
        for m in d["measurements"].as_array().into_iter().flatten() {
            let status = m["status"].as_str().expect("status は文字列");
            assert!(
                measurement_statuses.contains(&status),
                "不正な status: {status}"
            );
            if status != "measured" {
                assert!(
                    m.get("value").is_none(),
                    "measured 以外が value を持っている: {m}"
                );
            }
        }
        // locations の role は既知の役割
        for l in d["locations"].as_array().into_iter().flatten() {
            assert!(
                ["primary", "counterpart", "occurrence"]
                    .contains(&l["role"].as_str().unwrap_or("")),
                "不正な role: {}",
                l["role"]
            );
        }
    }
    // summary は v1 の集計フィールドを持たない(#33/#34)
    let summary_json = &value["summary"];
    for removed in ["estimated_reduction", "fixes"] {
        assert!(
            summary_json.get(removed).is_none(),
            "v1 summary フィールド残存: {removed}"
        );
    }
    let text = String::from_utf8(buf)?;
    assert!(text.contains("\"severity\""));
    Ok(())
}

#[test]
fn rule_excludes_reports_suppressed_by_path() -> Result<()> {
    // #85: [lint.rule_excludes] で抑制した診断数が context.rules に載り、
    // 診断自体は消える。抑制していないルールにはフィールドを出さない
    let config = Config::from_toml(
        r#"[lint.rule_excludes]
"structure/empty-section" = ["**/sample.md"]"#,
    )?;
    let mut buf = Vec::new();
    run_check(
        &[PathBuf::from("../jaio-rules/tests/fixtures/sample.md")],
        &mut buf,
        &config,
        &OutputFormat::Json,
        false,
        false,
    )?;
    let value: serde_json::Value = serde_json::from_slice(&buf)?;
    // 診断自体は抑制される(sample.md の「#### インストール」= 空セクション)
    let diags = value["diagnostics"].as_array().expect("配列のはず");
    assert!(
        diags
            .iter()
            .all(|d| d["rule_id"] != "structure/empty-section"),
        "対象パスの empty-section は報告されないはず: {diags:?}"
    );
    // 抑制件数は context.rules に反映される
    let rules = value["context"]["rules"].as_array().expect("配列のはず");
    let es = rules
        .iter()
        .find(|r| r["rule_id"] == "structure/empty-section")
        .expect("empty-section の status があるはず");
    assert_eq!(es["suppressed_by_path"], serde_json::json!(1));
    // 抑制していないルールにはフィールドを出さない
    for r in rules {
        if r["rule_id"] != "structure/empty-section" {
            assert!(
                r.get("suppressed_by_path").is_none(),
                "抑制のないルールには出力しない: {r}"
            );
        }
    }
    Ok(())
}

#[test]
fn absolute_scan_root_does_not_panic() -> Result<()> {
    // cwd 外の絶対パスを走査ルートにしても panic しない(回帰試験)。
    // 旧実装は cwd 基準の表示パスを gitignore フィルタへ渡し、
    // ルート検査("path is expected to be under the root")で panic した
    let dir = std::env::temp_dir().join(format!("jaio-abs-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let kept = dir.join("a.md");
    let excluded = dir.join("b.gen.md");
    std::fs::write(&kept, "# 題名\n\n本文です。\n")?;
    std::fs::write(&excluded, "# 除外\n\n本文です。\n")?;
    let config = Config::from_toml("[paths]\nexclude = [\"*.gen.md\"]\n")?;
    let summary = run_check(
        std::slice::from_ref(&dir),
        &mut Vec::new(),
        &config,
        &OutputFormat::Json,
        false,
        false,
    )?;
    assert_eq!(summary.files, 1, "exclude が絶対ルートでも効くはず");
    std::fs::remove_file(&kept)?;
    std::fs::remove_file(&excluded)?;
    std::fs::remove_dir(&dir)?;
    Ok(())
}

#[test]
fn check_never_modifies_sources() -> Result<()> {
    // §7.4/#33: jaio は読み取り専用。診断が出るファイルでも内容は不変
    let dir = std::env::temp_dir().join(format!("jaio-readonly-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let file = dir.join("a.md");
    let original = "# 題名\n\n使用することができます。\n";
    std::fs::write(&file, original)?;
    let config = Config::default();
    let mut buf = Vec::new();
    let summary = run_check(
        std::slice::from_ref(&dir),
        &mut buf,
        &config,
        &OutputFormat::Json,
        false,
        false,
    )?;
    // 冗長表現の診断が出ている(変更対象があり得る状態)でも元ファイルは無変更
    assert!(summary.warnings + summary.infos > 0, "診断が出るはず");
    assert_eq!(std::fs::read_to_string(&file)?, original);
    // context.sources の hash が元ファイル内容と一致する(解析対象=元ファイル)
    let value: serde_json::Value = serde_json::from_slice(&buf)?;
    let digest = format!("{:x}", sha2::Sha256::digest(original.as_bytes()));
    assert_eq!(
        value["context"]["sources"][0]["sha256"],
        serde_json::json!(digest)
    );
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}

#[test]
fn explain_finds_known_rule() -> Result<()> {
    let mut buf = Vec::new();
    explain_rule("consistency/style-deviation", &mut buf)?;
    let out = String::from_utf8(buf)?;
    assert!(out.contains("文体の逸脱"));
    assert!(explain_rule("unknown/rule", &mut Vec::new()).is_err());
    Ok(())
}

#[test]
fn stats_terminal_keeps_table_format() -> Result<()> {
    // 既定(terminal): 表形式の出力を維持する(#50 で出力分岐を導入した際の回帰確認)
    let dir = std::env::temp_dir().join(format!("jaio-stats-terminal-{}", std::process::id()));
    fs::create_dir_all(&dir)?;
    fs::write(dir.join("a.md"), "# 題名\n\n本文です。\n")?;
    let config = Config::default();
    let mut buf = Vec::new();
    run_stats(
        std::slice::from_ref(&dir),
        &mut buf,
        &config,
        false,
        StatsFormat::Terminal,
    )?;
    let out = String::from_utf8(buf)?;
    assert!(out.contains("tok/100ch"), "{out}");
    assert!(out.contains("total"), "{out}");
    fs::remove_dir_all(&dir).ok();
    Ok(())
}

#[test]
fn stats_json_reports_files_total_and_context() -> Result<()> {
    // --format json: 層別集計の自動化に必要な files/total/context を出す(#50)。
    // --similarity 併用時はヒストグラムと上位ペアも構造化して含める
    let dir = std::env::temp_dir().join(format!("jaio-stats-json-{}", std::process::id()));
    fs::create_dir_all(&dir)?;
    // a.md は frontmatter 付き(SKILL.md 型)、b.md は無し(通常文書型)
    fs::write(
        dir.join("a.md"),
        "---\ndescription: このスキルは設定ファイルの読み込み処理について説明します。起動時に一度だけ読み込まれます。\n---\n\nこのセクションでは設定ファイルの読み込み処理について説明します。設定ファイルは起動時に一度だけ読み込まれます。\n",
    )?;
    fs::write(
        dir.join("b.md"),
        "# 題名B\n\nこのセクションでは設定ファイルの読み込み処理について説明します。設定ファイルは起動時に一度だけ読み込まれます。\n",
    )?;
    let config = Config::default();
    let mut buf = Vec::new();
    run_stats(
        std::slice::from_ref(&dir),
        &mut buf,
        &config,
        true,
        StatsFormat::Json,
    )?;
    let value: serde_json::Value = serde_json::from_slice(&buf)?;
    assert_eq!(value["kind"], "jaio/stats");
    assert_eq!(value["version"], 1);
    assert_eq!(value["total"]["files"], 2);
    let files = value["files"].as_array().expect("files は配列");
    assert_eq!(files.len(), 2);
    for f in files {
        for key in [
            "file",
            "chars",
            "tokens",
            "tok_per_100ch",
            "frontmatter",
            "body",
        ] {
            assert!(f.get(key).is_some(), "files[] に {key} がない: {f}");
        }
    }
    // frontmatter 分離計測(#53): a.md(frontmatter あり)は常時層と発火層に
    // 分かれ、b.md(なし)は frontmatter が null で本文が全体と一致する
    let a = &files[0];
    let fm = a["frontmatter"]
        .as_object()
        .expect("a.md は frontmatter がある");
    assert!(fm["chars"].as_u64().unwrap() > 0);
    assert!(fm["tokens"].as_u64().unwrap() > 0);
    let a_total = a["chars"].as_u64().unwrap();
    let a_fm = fm["chars"].as_u64().unwrap();
    let a_body = a["body"]["chars"].as_u64().unwrap();
    assert_eq!(a_total, a_fm + a_body, "全体 = frontmatter + 本文のはず");
    let b = &files[1];
    assert!(b["frontmatter"].is_null());
    assert_eq!(
        b["body"]["chars"], b["chars"],
        "frontmatter なしの本文は全体と一致するはず"
    );
    // check と同じ再現性識別の context(config_fingerprint・sources・targets)
    let context = &value["context"];
    for key in [
        "roots",
        "config_fingerprint",
        "sources",
        "algorithms",
        "targets",
        "operational_errors",
    ] {
        assert!(
            context.get(key).is_some(),
            "context に {key} がない: {context}"
        );
    }
    assert!(
        context["config_fingerprint"]
            .as_str()
            .is_some_and(|f| f.starts_with("sha256:"))
    );
    assert_eq!(context["targets"]["discovered"], 2);
    assert_eq!(context["targets"]["analyzed"], 2);
    assert_eq!(context["targets"]["discovery_complete"], true);
    assert!(
        context["algorithms"]["tokenizer"]
            .as_str()
            .is_some_and(|t| t.contains("tiktoken"))
    );
    // stats は形態素解析を使わないため morphology は出さない
    assert!(context["algorithms"].get("morphology").is_none());
    // 類似度分布は構造化されており、terminal 文字列を混ぜない
    assert!(value["similarity"]["histogram"].as_array().is_some());
    assert!(value["similarity"]["top_pairs"].as_array().is_some());
    let top = &value["similarity"]["top_pairs"][0];
    assert!(
        top["a"].as_str().is_some_and(|s| s.ends_with("a.md"))
            && top["b"].as_str().is_some_and(|s| s.ends_with("b.md")),
        "top_pairs の両端が a.md/b.md のはず: {top}"
    );
    fs::remove_dir_all(&dir).ok();
    Ok(())
}

#[test]
fn similarity_distribution_reports_pairs_and_threshold() -> Result<()> {
    // --similarity: 近重複ペアが上位ペアに現れ、min_similarity の bin に
    // 注釈が付く(閾値チューニングの根拠出力)
    let dir = std::env::temp_dir().join(format!("jaio-simstats-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    // 本文(doc_body_min_chars=50 を超える)は同一で見出しだけ異なる 2 文書
    std::fs::write(
        dir.join("a.md"),
        "# 題名A\n\nこのセクションでは設定ファイルの読み込み処理について説明します。設定ファイルは起動時に一度だけ読み込まれます。\n",
    )?;
    std::fs::write(
        dir.join("b.md"),
        "# 題名B\n\nこのセクションでは設定ファイルの読み込み処理について説明します。設定ファイルは起動時に一度だけ読み込まれます。\n",
    )?;
    let config = Config::default();
    let files = collect_md_files(std::slice::from_ref(&dir), &config)?;
    // 読み込み済みの parse 結果を渡す形式に合わせる(#103 と同じ構造)
    let parsed: Vec<(PathBuf, Doc, Vec<TextLeaf>)> = files
        .iter()
        .map(|f| {
            let source = std::fs::read_to_string(f).expect("テストファイルが読める");
            let (doc, leaves, _) = Doc::parse(f.clone(), source);
            (f.clone(), doc, leaves)
        })
        .collect();
    let dist = similarity_distribution(&parsed, &config)?;
    assert_eq!((dist.pairs, dist.docs, dist.min_chars), (1, 2, 50));
    let mut buf = Vec::new();
    write_similarity_distribution(&dist, &mut buf, &config)?;
    let out = String::from_utf8(buf)?;
    assert!(
        out.contains("similarity distribution (1 pairs, 2 docs >= 50 chars):"),
        "{out}"
    );
    assert!(out.contains("top pairs:"), "{out}");
    // 表示パスは絶対パスになるため、上位ペア行の両端だけを検証する
    let top_line = out.lines().find(|l| l.contains("<->")).unwrap_or_default();
    assert!(top_line.contains("a.md <-> "), "{out}");
    assert!(top_line.ends_with("b.md"), "{out}");
    // 類似度 1.00 のペアが上位に並ぶ(同点はパス名順)
    assert!(out.contains("1.00"), "{out}");
    // 既定の min_similarity(0.65)が属す 0.65-0.70 の bin に注釈が付く
    let threshold_line = out
        .lines()
        .find(|l| l.contains("<- min_similarity"))
        .unwrap_or_default();
    assert!(threshold_line.starts_with("  0.65-0.70:"), "{out}");
    std::fs::remove_file(dir.join("a.md"))?;
    std::fs::remove_file(dir.join("b.md"))?;
    std::fs::remove_dir(&dir)?;
    Ok(())
}

#[cfg(test)]
mod llm_stability_tests {
    use super::*;

    /// LLM 委譲ワークフロー(設計書 §7.5、Issue #7)は「JSON キーの追加互換」を
    /// 前提とする。トップレベルと diagnostics の必須キーが欠けていないことを検証し、
    /// 互換性の破壊をテストで捕捉する。
    #[test]
    fn json_output_keys_are_stable() -> Result<()> {
        let config = Config::default();
        let mut buf = Vec::new();
        run_check(
            &[PathBuf::from("../jaio-rules/tests/fixtures/sample.md")],
            &mut buf,
            &config,
            &OutputFormat::Json,
            false,
            false,
        )?;
        let value: serde_json::Value = serde_json::from_slice(&buf)?;

        // トップレベルの必須キー(schema v2、§7.3)
        for key in [
            "schema_version",
            "tool",
            "context",
            "summary",
            "diagnostics",
        ] {
            assert!(value.get(key).is_some(), "必須キー {key} が欠けている");
        }
        // context の必須キー
        for key in ["roots", "config_fingerprint", "sources", "algorithms"] {
            assert!(
                value["context"].get(key).is_some(),
                "context の必須キー {key} が欠けている"
            );
        }
        for key in ["morphology", "tokenizer", "similarity"] {
            assert!(
                value["context"]["algorithms"].get(key).is_some(),
                "algorithms の必須キー {key} が欠けている"
            );
        }
        // summary の必須キー
        for key in [
            "files",
            "tokens",
            "errors",
            "warnings",
            "infos",
            "top_rules",
        ] {
            assert!(
                value["summary"].get(key).is_some(),
                "summary の必須キー {key} が欠けている"
            );
        }
        // diagnostics 各要素の必須キー
        let diagnostics = value["diagnostics"].as_array().expect("diagnostics は配列");
        assert!(!diagnostics.is_empty(), "fixture は診断を出す");
        for d in diagnostics {
            for key in [
                "file",
                "span",
                "rule_id",
                "category",
                "severity",
                "observation",
            ] {
                assert!(
                    d.get(key).is_some(),
                    "diagnostics の必須キー {key} が欠けている: {d}"
                );
            }
            for key in ["line", "col"] {
                assert!(
                    d["span"].get(key).is_some(),
                    "span の必須キー {key} が欠けている"
                );
            }
        }
        Ok(())
    }
}
