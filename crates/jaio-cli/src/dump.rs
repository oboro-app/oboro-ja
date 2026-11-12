use super::*;

/// テキストリーフの種別を JSON 用の文字列にする。
pub(crate) fn leaf_kind_name(kind: jaio_parse::doc::LeafKind) -> &'static str {
    use jaio_parse::doc::LeafKind;
    match kind {
        LeafKind::Heading => "heading",
        LeafKind::Paragraph => "paragraph",
        LeafKind::ListItem => "list_item",
        LeafKind::TableCell => "table_cell",
    }
}

/// 文書構造 AST を JSON でダンプする(`jaio dump doc`、Issue #16)。
/// 誤検出の調査・ルール開発・LLM 委譲の素材に使う。
pub fn run_dump_doc(path: &Path, out: &mut impl Write, config: &Config) -> Result<()> {
    let source = fs::read_to_string(path)
        .with_context(|| format!("ファイルの読み込みに失敗しました: {}", path.display()))?;
    let (doc, leaves, headings) = Doc::parse(path.to_path_buf(), source);

    // frontmatter の領域情報(#53)。tokens は stats と同じ encoding で計測する(#57)
    let counter = TokenCounter::with_encoding(config.analysis.encoding)?;
    let frontmatter_json = doc.frontmatter.as_deref().map(|fm| {
        serde_json::json!({
            "chars": fm.chars().count(),
            "tokens": counter.count(fm),
        })
    });

    let value = serde_json::json!({
        "kind": "jaio/dump-doc",
        "version": jaio_report::SCHEMA_VERSION,
        "file": normalize_display(path),
        "frontmatter": frontmatter_json,
        "headings": headings.iter().map(|h| serde_json::json!({
            "level": h.level,
            "line": h.line,
            "title": h.title,
            "empty": h.empty,
        })).collect::<Vec<_>>(),
        "leaves": leaves.iter().map(|l| serde_json::json!({
            "line": l.line,
            "kind": leaf_kind_name(l.kind),
            "nest_depth": l.nest_depth,
            "text": l.text,
        })).collect::<Vec<_>>(),
        "links": doc.links.iter().map(|l| serde_json::json!({
            "url": l.url,
            "line": l.line,
            "kind": if l.kind == jaio_parse::doc::LinkKind::Image { "image" } else { "link" },
        })).collect::<Vec<_>>(),
    });
    writeln!(
        out,
        "{}",
        serde_json::to_string_pretty(&value).map_err(|e| anyhow::anyhow!(e))?
    )?;
    Ok(())
}

/// 形態素解析結果を JSON でダンプする(`jaio dump morph`、Issue #16)。
/// 「なぜこの判定になったか」を形態素単位で直接確認する。
/// check と同じ実効設定(解析モード・辞書)を使い、識別情報も出力する(#39)
pub fn run_dump_morph(path: &Path, out: &mut impl Write, config: &Config) -> Result<()> {
    let source = fs::read_to_string(path)
        .with_context(|| format!("ファイルの読み込みに失敗しました: {}", path.display()))?;
    let (_doc, leaves, _headings) = Doc::parse(path.to_path_buf(), source);
    // 実際にロードする辞書実体の識別を先に解決し、出力がロード対象と一致する
    // ことを保証する(#36)
    let dict_id = dictionary_identity(config.analysis.dictionary.as_deref());
    let mut analyzer =
        Analyzer::with_options(config.analysis.mode, config.analysis.dictionary.as_deref())?;

    let mut leaves_json = Vec::new();
    for leaf in &leaves {
        let morphs = analyzer.analyze(&leaf.text)?;
        leaves_json.push(serde_json::json!({
            "line": leaf.line,
            "kind": leaf_kind_name(leaf.kind),
            "morphs": morphs,
        }));
    }
    let value = serde_json::json!({
        "kind": "jaio/dump-morph",
        "version": jaio_report::SCHEMA_VERSION,
        "file": normalize_display(path),
        "config_fingerprint": config_fingerprint(config),
        "algorithms": algorithms(config, &dict_id),
        "leaves": leaves_json,
    });
    writeln!(
        out,
        "{}",
        serde_json::to_string_pretty(&value).map_err(|e| anyhow::anyhow!(e))?
    )?;
    Ok(())
}

/// Corpus の内容を JSON でダンプする(`jaio dump corpus`、Issue #16)。
/// 重複検出の調査と LLM 委譲の素材に使う。本文そのものは大きいため、
/// 文字数とセクション構造のみを出す(bodies 全文は含めない)。
/// check と同じ実効設定・収集・入力識別を使い(#39)、致命的でない失敗は
/// operational error として記録して継続する。戻り値の `JsonSummary` は
/// check と同じ終了コード契約(未完了なら 2)を与える
pub fn run_dump_corpus(
    paths: &[PathBuf],
    out: &mut impl Write,
    config: &Config,
) -> Result<JsonSummary> {
    // check と同じ走査で、致命的でない走査失敗も記録する(#39)
    let (files, mut operational_errors) = collect_files_reporting(paths, config, is_markdown)?;
    let discovery_complete = operational_errors.is_empty();
    // 辞書実体の識別も check と同じ規則で 1 回だけ解決する(#36/#39)
    let dict_id = dictionary_identity(config.analysis.dictionary.as_deref());
    let counter = TokenCounter::with_encoding(config.analysis.encoding)?;
    let mut docs: Vec<Doc> = Vec::new();
    let mut bodies: Vec<String> = Vec::new();
    let mut doc_tokens: Vec<usize> = Vec::new();
    let mut mtimes: Vec<SystemTime> = Vec::new();
    let mut total_tokens = 0usize;
    let mut skipped_files = 0usize;
    // check と同じ入力識別(#36/#39)
    let mut sources: Vec<JsonSource> = Vec::new();
    let mut section_inputs: Vec<(
        Vec<jaio_parse::doc::TextLeaf>,
        Vec<jaio_parse::doc::Heading>,
    )> = Vec::new();

    for file in &files {
        let source = match fs::read_to_string(file) {
            Ok(source) => source,
            Err(err) => {
                // 読み込み失敗は dump を止めず operational error として記録する(#39)
                skipped_files += 1;
                operational_errors.push(JsonOperationalError {
                    path: Some(normalize_display(file)),
                    stage: "read".to_string(),
                    reason: err.to_string(),
                });
                continue;
            }
        };
        sources.push(JsonSource {
            path: normalize_display(file),
            sha256: format!("{:x}", sha2::Sha256::digest(source.as_bytes())),
        });
        let tokens = counter.count(&source);
        total_tokens += tokens;
        doc_tokens.push(tokens);
        let (mut doc, leaves, headings) = Doc::parse(file.clone(), source);
        doc.display = normalize_display(file);
        // Corpus 入力の組み立ては check と共通の変換を使う(#96)
        let input = corpus::corpus_doc_input(file, &leaves, config.merge.weights.recency > 0.0);
        if let Some(err) = input.metadata_error {
            operational_errors.push(err);
        }
        bodies.push(input.body);
        mtimes.push(input.mtime);
        section_inputs.push((leaves, headings));
        docs.push(doc);
    }
    // comment 収集・Corpus 構築は check と共通の変換を使う(#96)。
    // sections / comment_files は後段の出力でも使うため所有をここに置く
    let (comment_files, mut comment_errors) =
        corpus::collect_comment_inputs(paths, config, &mut sources)?;
    operational_errors.append(&mut comment_errors);
    let sections = corpus::build_sections(&section_inputs, config.detection.section_body_min_chars);
    let corpus = corpus::build_corpus(
        paths,
        config,
        corpus::CorpusBorrowed {
            docs: &docs,
            bodies: &bodies,
            doc_tokens: &doc_tokens,
            mtimes: &mtimes,
            sections: &sections,
            comment_files: &comment_files,
        },
    )?;

    let docs_json: Vec<_> = docs
        .iter()
        .enumerate()
        .map(|(i, doc)| {
            serde_json::json!({
                "file": doc.display,
                "tokens": doc_tokens[i],
                "body_chars": bodies[i].chars().count(),
                "sections": sections[i].iter().map(|s| serde_json::json!({
                    "line": s.line,
                    "title": s.title,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    let duplicates_json: Vec<_> = corpus
        .duplicates
        .iter()
        .map(|p| {
            serde_json::json!({
                "a": docs[p.a].display,
                "b": docs[p.b].display,
                "score": p.score,
                "intersection": p.intersection,
                "union": p.union,
            })
        })
        .collect();
    let comments_json: Vec<_> = comment_files
        .iter()
        .map(|c| {
            serde_json::json!({
                "file": c.display,
                "body_chars": c.body.chars().count(),
            })
        })
        .collect();
    let analyzed = files.len().saturating_sub(skipped_files);
    // dump はルールを実行しないため rules は空。完全性・候補列挙・算法・
    // 入力識別は check と同じ context 形で出す(#39)
    let targets = JsonTargets {
        discovered: files.len(),
        analyzed,
        skipped: skipped_files,
        discovery_complete,
    };
    let analysis_complete = operational_errors.is_empty();
    let context = build_context(
        paths,
        config,
        &dict_id,
        ContextResults {
            sources,
            targets,
            operational_errors,
            rules: Vec::new(),
            candidate_search: candidate_search_json(&corpus.pair_search),
        },
    );
    let value = serde_json::json!({
        "kind": "jaio/dump-corpus",
        "version": jaio_report::SCHEMA_VERSION,
        "analysis_complete": analysis_complete,
        "context": context,
        "docs": docs_json,
        "duplicates": duplicates_json,
        "comments": comments_json,
    });
    writeln!(
        out,
        "{}",
        serde_json::to_string_pretty(&value).map_err(|e| anyhow::anyhow!(e))?
    )?;
    // check と同じ終了コード契約(未完了なら 2、診断を出さない dump に 1 はない)
    Ok(summarize(
        analyzed,
        total_tokens,
        &[],
        None,
        analysis_complete,
    ))
}

#[cfg(test)]
mod dump_tests {
    use super::*;
    use std::fs;

    #[test]
    fn dump_doc_outputs_structure_json() -> Result<()> {
        let dir = std::env::temp_dir().join("jaio-dump-doc-test");
        fs::create_dir_all(&dir)?;
        let file = dir.join("a.md");
        fs::write(&file, "# 題名\n\n本文です。\n\n- 箇条書き\n")?;
        let mut buf = Vec::new();
        run_dump_doc(&file, &mut buf, &Config::default())?;
        let value: serde_json::Value = serde_json::from_slice(&buf)?;
        assert_eq!(value["headings"][0]["title"], "題名");
        let kinds: Vec<&str> = value["leaves"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["kind"].as_str().unwrap())
            .collect();
        assert!(kinds.contains(&"paragraph"));
        assert!(kinds.contains(&"list_item"));
        fs::remove_dir_all(&dir).ok();
        Ok(())
    }

    #[test]
    fn dump_doc_frontmatter_includes_tokens() -> Result<()> {
        // frontmatter がある場合は chars と tokens の両方を出す(#53/#57)
        let dir = std::env::temp_dir().join("jaio-dump-doc-fm-test");
        fs::create_dir_all(&dir)?;
        let file = dir.join("skill.md");
        fs::write(
            &file,
            "---\ndescription: 設定ファイルの読み込み処理を説明するスキルです。\n---\n\n# 題名\n\n本文です。\n",
        )?;
        let mut buf = Vec::new();
        run_dump_doc(&file, &mut buf, &Config::default())?;
        let value: serde_json::Value = serde_json::from_slice(&buf)?;
        let fm = value["frontmatter"]
            .as_object()
            .expect("frontmatter があるはず");
        assert!(fm["chars"].as_u64().unwrap() > 0);
        assert!(
            fm["tokens"].as_u64().unwrap() > 0,
            "tokens も出すはず: {fm:?}"
        );
        fs::remove_dir_all(&dir).ok();
        Ok(())
    }

    #[test]
    fn dump_morph_outputs_morphs() -> Result<()> {
        let dir = std::env::temp_dir().join("jaio-dump-morph-test");
        fs::create_dir_all(&dir)?;
        let file = dir.join("a.md");
        fs::write(&file, "# 題名\n\n設定ファイルを読み込みます。\n")?;
        let mut buf = Vec::new();
        run_dump_morph(&file, &mut buf, &Config::default())?;
        let value: serde_json::Value = serde_json::from_slice(&buf)?;
        let text = serde_json::to_string(&value)?;
        assert!(
            text.contains("読み込み"),
            "形態素の表層形が含まれるはず: {text}"
        );
        assert!(text.contains("動詞"), "品詞情報が含まれるはず: {text}");
        fs::remove_dir_all(&dir).ok();
        Ok(())
    }

    #[test]
    fn dump_corpus_outputs_duplicates() -> Result<()> {
        let dir = std::env::temp_dir().join("jaio-dump-corpus-test");
        fs::create_dir_all(&dir)?;
        let body = "このセクションでは設定ファイルの読み込み処理について説明します。設定ファイルは起動時に一度だけ読み込まれ、変更された場合は再起動が必要です。読み込みに失敗した場合はエラーメッセージを表示して終了します。".repeat(3);
        fs::write(dir.join("a.md"), format!("# A\n\n{body}\n"))?;
        fs::write(dir.join("b.md"), format!("# B\n\n{body}\n"))?;
        let mut buf = Vec::new();
        run_dump_corpus(std::slice::from_ref(&dir), &mut buf, &Config::default())?;
        let value: serde_json::Value = serde_json::from_slice(&buf)?;
        assert_eq!(
            value["duplicates"].as_array().map(Vec::len),
            Some(1),
            "同一本文の 1 ペアが検出されるはず: {}",
            value["duplicates"]
        );
        assert_eq!(value["docs"].as_array().map(Vec::len), Some(2));
        fs::remove_dir_all(&dir).ok();
        Ok(())
    }

    /// #39: dump morph は check と同じ実効設定(解析モード・辞書)を使う。
    /// 設定がアルゴリズム識別に反映され、無効な外部辞書は埋め込みへ
    /// 黙って逃げずにエラーになることを検証する
    #[test]
    fn dump_morph_uses_configured_mode_and_dictionary() -> Result<()> {
        let dir = std::env::temp_dir().join("jaio-dump-morph-cfg-test");
        fs::create_dir_all(&dir)?;
        let file = dir.join("a.md");
        fs::write(&file, "# 題名\n\n設定ファイルを読み込みます。\n")?;

        let mut config = Config::default();
        config.analysis.mode = AnalysisMode::Decompose;
        let mut buf = Vec::new();
        run_dump_morph(&file, &mut buf, &config)?;
        let value: serde_json::Value = serde_json::from_slice(&buf)?;
        let morphology = value["algorithms"]["morphology"].as_str().unwrap();
        assert!(
            morphology.contains("mode=decompose"),
            "設定した mode が識別子に出るはず: {morphology}"
        );
        assert!(morphology.contains("dict=embedded:sudachidict"));

        // 無効な外部辞書パスは埋め込みへフォールバックせず設定エラーになる
        config.analysis.dictionary = Some("file:///nonexistent-dict-dir".to_string());
        assert!(
            run_dump_morph(&file, &mut Vec::new(), &config).is_err(),
            "存在しない辞書はエラーになるはず(埋め込みへの黙った退避をしない)"
        );
        fs::remove_dir_all(&dir).ok();
        Ok(())
    }

    /// #39: dump corpus の token 数は設定 encoding の直接計測と一致する
    #[test]
    fn dump_corpus_uses_configured_encoding() -> Result<()> {
        let dir = std::env::temp_dir().join("jaio-dump-corpus-enc-test");
        fs::create_dir_all(&dir)?;
        let source = "# 題名\n\n設定ファイルを読み込み、起動時に一度だけ解析します。\n";
        fs::write(dir.join("a.md"), source)?;

        let mut config = Config::default();
        config.analysis.encoding = TokenEncoding::Cl100kBase;
        let mut buf = Vec::new();
        run_dump_corpus(std::slice::from_ref(&dir), &mut buf, &config)?;
        let value: serde_json::Value = serde_json::from_slice(&buf)?;

        let expected = TokenCounter::with_encoding(TokenEncoding::Cl100kBase)?.count(source);
        assert_eq!(
            value["docs"][0]["tokens"].as_u64().unwrap() as usize,
            expected,
            "dump corpus の tokens は指定 encoding の直接計測と一致するはず"
        );
        assert_eq!(
            value["context"]["algorithms"]["tokenizer"]
                .as_str()
                .unwrap(),
            "tiktoken cl100k_base"
        );
        fs::remove_dir_all(&dir).ok();
        Ok(())
    }

    /// #39: 読み込み失敗を注入した dump corpus は部分結果を返しつつ
    /// 不完全性(complete=false・exit 2)を明示する
    #[test]
    fn dump_corpus_partial_failure_marks_incomplete() -> Result<()> {
        let dir = std::env::temp_dir().join("jaio-dump-corpus-inc-test");
        fs::create_dir_all(&dir)?;
        fs::write(dir.join("ok.md"), "# 題名\n\n読める文書です。\n")?;
        fs::write(dir.join("bad.md"), [0xffu8, 0xfe, 0xfd])?;

        let mut buf = Vec::new();
        let summary = run_dump_corpus(std::slice::from_ref(&dir), &mut buf, &Config::default())?;
        let value: serde_json::Value = serde_json::from_slice(&buf)?;

        // 部分結果は出力される
        assert_eq!(value["docs"].as_array().map(Vec::len), Some(1));
        // 不完全性が機械可読に出る
        assert_eq!(value["analysis_complete"], false);
        assert_eq!(value["context"]["targets"]["skipped"], 1);
        let stages: Vec<&str> = value["context"]["operational_errors"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e["stage"].as_str())
            .collect();
        assert!(
            stages.contains(&"read"),
            "read 失敗が記録されるはず: {stages:?}"
        );
        assert_eq!(summary.exit_code(), 2);
        fs::remove_dir_all(&dir).ok();
        Ok(())
    }

    /// #36: 同じ Markdown でも、使ったコメント入力や辞書設定が変われば
    /// 対応する入力・算法の識別が変わる(再現性の追跡)
    #[test]
    fn input_identification_tracks_config_and_comment_sources() -> Result<()> {
        let dir = std::env::temp_dir().join("jaio-ident-test");
        fs::create_dir_all(&dir)?;
        fs::write(dir.join("a.md"), "# 題名\n\n本文です。\n")?;

        // 辞書設定だけ変えると algorithms の識別と fingerprint が変わる
        let mut other = Config::default();
        other.analysis.dictionary = Some("file:///another-dict".to_string());
        let base = algorithms(
            &Config::default(),
            &dictionary_identity(Config::default().analysis.dictionary.as_deref()),
        );
        let changed = algorithms(
            &other,
            &dictionary_identity(other.analysis.dictionary.as_deref()),
        );
        assert_ne!(base.morphology, changed.morphology);
        assert_ne!(
            config_fingerprint(&Config::default()),
            config_fingerprint(&other)
        );

        // コメント抽出対象を足すと sources に抽出元の hash が現れる
        fs::write(
            dir.join("main.rs"),
            "// 設定ファイルを読み込む手順を説明するコメント\n",
        )?;
        let mut buf = Vec::new();
        run_dump_corpus(std::slice::from_ref(&dir), &mut buf, &Config::default())?;
        let value: serde_json::Value = serde_json::from_slice(&buf)?;
        let paths: Vec<&str> = value["context"]["sources"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|s| s["path"].as_str())
            .collect();
        assert!(
            paths.iter().any(|p| p.ends_with("main.rs")),
            "コメント抽出元も sources に含まれるはず: {paths:?}"
        );
        fs::remove_dir_all(&dir).ok();
        Ok(())
    }

    /// #36: 外部辞書の識別は lindera が実際にロードするファイル集合
    /// (DICTIONARY_FILES = package-dict.sh と同じ集合)の内容 fingerprint で
    /// 行い、同一パスの実体差し替えを検出する。sys.dic のない配布
    /// レイアウトでも識別できることを fixture で確認する。
    /// なお fixture の中身は有効な lindera 辞書ではなく、ロードの E2E は
    /// 実辞書が必要(識別ロジックの検証が目的)
    #[test]
    fn dictionary_identity_tracks_same_path_content_changes() -> Result<()> {
        let dir = std::env::temp_dir().join("jaio-dict-ident-test");
        let dict = dir.join("dict");
        fs::create_dir_all(&dict)?;
        for name in jaio_parse::morph::DICTIONARY_FILES {
            fs::write(dict.join(name), format!("seed-{name}"))?;
        }
        let spec = format!("file://{}", dict.display());
        let first = dictionary_identity(Some(&spec));
        assert!(
            first.display.contains("manifest-sha256:"),
            "実体の内容 fingerprint が出るはず: {}",
            first.display
        );
        assert!(first.fingerprint.is_some());
        // 同一内容の再実行では不変
        assert_eq!(dictionary_identity(Some(&spec)).display, first.display);

        // 同一パスで実体を変えると fingerprint が変わる
        fs::write(dict.join("dict.words"), "different-content")?;
        let second = dictionary_identity(Some(&spec));
        assert_ne!(second.fingerprint, first.fingerprint);

        // 必須ファイルの欠落は「識別不能」であって、パスだけの強い識別に見せない
        fs::remove_file(dict.join("unk.bin"))?;
        let missing = dictionary_identity(Some(&spec));
        assert!(
            missing.display.contains("unidentified"),
            "識別不能は理由つきで明示するはず: {}",
            missing.display
        );
        assert!(missing.fingerprint.is_none());
        fs::remove_dir_all(&dir).ok();
        Ok(())
    }

    /// lindera の embedded:// URI を config で指定した場合も、
    /// 外部辞書ではなく埋め込み辞書として識別し、実体が不変なので
    /// ワーカー再利用ができる(fingerprint を再利用マーカーとして持つ)。
    /// 埋め込みビルドでは同梱辞書を一意に決める crate 版が display に付く(#64)
    #[test]
    fn embedded_uri_spec_is_identified_as_embedded() {
        let id = dictionary_identity(Some("embedded://sudachidict"));
        match jaio_parse::morph::embedded_dict_version() {
            Some(version) => {
                assert_eq!(id.display, format!("embedded:sudachidict@{version}"));
            }
            None => assert_eq!(id.display, "embedded:sudachidict"),
        }
        assert!(
            id.fingerprint.is_some(),
            "埋め込み URI は実体不変なので再利用可能な識別を持つ"
        );
    }

    /// #36: WorkerKey は外部辞書の内容 fingerprint を含むため、
    /// 同一プロセスでの同一パス差し替えで再ロードされる。
    /// 識別不能な辞書は同一性を証明できないため再利用しない
    #[test]
    fn worker_key_reloads_on_same_path_dictionary_change() {
        let base = WorkerKey {
            mode: AnalysisMode::Normal,
            dictionary: Some("file:///d".to_string()),
            dict_fingerprint: Some("aaa".to_string()),
            encoding: TokenEncoding::O200kBase,
        };
        let same = WorkerKey {
            dict_fingerprint: Some("aaa".to_string()),
            ..base.clone()
        };
        let changed = WorkerKey {
            dict_fingerprint: Some("bbb".to_string()),
            ..base.clone()
        };
        let unidentified = WorkerKey {
            dict_fingerprint: None,
            ..base.clone()
        };
        let embedded_a = WorkerKey {
            dictionary: None,
            dict_fingerprint: None,
            ..base.clone()
        };
        let embedded_b = embedded_a.clone();

        assert!(base.is_reusable_with(&same), "同じ実体の辞書は再利用できる");
        assert!(
            !base.is_reusable_with(&changed),
            "実体が変われば再ロードする"
        );
        assert!(
            !base.is_reusable_with(&unidentified),
            "識別不能な辞書は再利用しない"
        );
        assert!(
            !unidentified.is_reusable_with(&unidentified),
            "識別不能同士でも再利用しない(必ず再ロード)"
        );
        assert!(
            embedded_a.is_reusable_with(&embedded_b),
            "埋め込み辞書は不変なので再利用できる"
        );
    }
}
