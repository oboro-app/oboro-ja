use super::*;

/// 辞書実体の識別結果(#36)。出力用文字列と、WorkerKey の再ロード判定に
/// 使う内容 fingerprint を分けて持つ(表示とキャッシュ一致の保証)
pub(crate) struct DictIdentity {
    /// `algorithms.morphology` の `dict=` 部分に入る識別子文字列
    pub(crate) display: String,
    /// 外部辞書の内容 fingerprint(埋め込み・識別不能時は None)
    pub(crate) fingerprint: Option<String>,
}

/// 外部辞書ディレクトリの内容 fingerprint を返す。
/// lindera が実際にロードするファイル集合(`DICTIONARY_FILES`)の
/// 「名前・バイト長・SHA-256」のマニフェストをさらに SHA-256 した
/// 決定的な識別子。大きい辞書に備えてファイル内容はストリーミングで読む
pub(crate) fn dictionary_manifest_sha256(dir: &Path) -> Result<String> {
    let mut manifest = String::new();
    for name in jaio_parse::morph::DICTIONARY_FILES {
        let file_path = dir.join(name);
        let mut file = fs::File::open(&file_path)
            .with_context(|| format!("辞書ファイルを開けません: {}", file_path.display()))?;
        let mut hasher = sha2::Sha256::new();
        let len = std::io::copy(&mut file, &mut hasher)
            .with_context(|| format!("辞書ファイルを読めません: {}", file_path.display()))?;
        manifest.push_str(&format!("{name}\t{len}\t{:x}\n", hasher.finalize()));
    }
    Ok(format!("{:x}", sha2::Sha256::digest(manifest.as_bytes())))
}

/// 埋め込み辞書の表示識別子(#64)。crate 版が取れるときは
/// `embedded:<name>@<crate-version>`。上流 SudachiDict 版は build 時に
/// vendored されるため実行時には取れず、同梱データを一意に決める
/// lindera-sudachidict crate 版を版表示に使う。版が取れないビルド
/// (埋め込みなし等)や sudachidict 以外の名前では従来どおり `embedded:<name>`
pub(crate) fn embedded_dict_display(name: &str) -> String {
    match jaio_parse::morph::embedded_dict_version() {
        Some(version) if name == "sudachidict" => format!("embedded:{name}@{version}"),
        _ => format!("embedded:{name}"),
    }
}

/// 設定された辞書の識別子を解決する。外部辞書は実際にロードされる
/// ファイル集合の内容 fingerprint で同定する。取得できない場合は
/// パスだけの強い識別を装わず「識別不能」と理由を出す(#36)。
/// fingerprint の算出は実行ごと 1 回に抑え、run 中の辞書差し替えは
/// サポートしない(次の実行で新しい fingerprint として検出される)
pub(crate) fn dictionary_identity(spec: Option<&str>) -> DictIdentity {
    match spec {
        // lindera の embedded:// URI も config から指定し得るため、
        // 外部辞書として誤識別しない。内容はビルドに固定され不変のため
        // fingerprint も spec 自身を再利用マーカーとして使う
        Some(spec) if spec.starts_with("embedded://") => {
            let name = spec.strip_prefix("embedded://").unwrap_or(spec);
            DictIdentity {
                display: embedded_dict_display(name),
                fingerprint: Some(format!("embedded:{name}")),
            }
        }
        None => DictIdentity {
            display: embedded_dict_display("sudachidict"),
            fingerprint: None,
        },
        Some(spec) => {
            let fs_path = spec.strip_prefix("file://").unwrap_or(spec);
            match dictionary_manifest_sha256(Path::new(fs_path)) {
                Ok(hash) => DictIdentity {
                    display: format!("external:{spec}:manifest-sha256:{hash}"),
                    fingerprint: Some(hash),
                },
                Err(err) => DictIdentity {
                    display: format!("external:{spec}:unidentified({err:#})"),
                    fingerprint: None,
                },
            }
        }
    }
}

/// ビルドの識別情報(#36)。ワークスペースの Cargo.lock が全依存 crate の版を
/// 固定するため、ビルド時点のその SHA-256 が埋め込み辞書データ・算法・
/// トークナイザーの版を追跡する共通 manifest になる
/// (作業ツリーの未コミット変更も反映される点に注意)
pub(crate) fn build_manifest_id() -> String {
    const LOCK: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock"));
    format!(
        "jaio {} deps-sha256:{:x}",
        env!("CARGO_PKG_VERSION"),
        sha2::Sha256::digest(LOCK.as_bytes())
    )
}

/// 前処理・算法の識別情報(§7.3、#36)。check と dump で共通に使い、
/// 同じ設定・辞書実体なら同じ識別子が出ることを保証する(#39)
pub(crate) fn algorithms(config: &Config, dict_id: &DictIdentity) -> JsonAlgorithms {
    let mode = match config.analysis.mode {
        AnalysisMode::Normal => "normal",
        AnalysisMode::Decompose => "decompose",
    };
    let encoding = match config.analysis.encoding {
        TokenEncoding::O200kBase => "o200k_base",
        TokenEncoding::Cl100kBase => "cl100k_base",
    };
    let dict = &dict_id.display;
    JsonAlgorithms {
        morphology: format!("lindera-sudachidict(mode={mode}, dict={dict})"),
        tokenizer: format!("tiktoken {encoding}"),
        similarity: "simhash64+bigram-jaccard",
        build: build_manifest_id(),
    }
}

/// `build_context` に渡す、実行ごとの結果まとめ
pub(crate) struct ContextResults {
    /// 解析できた入力ファイルの content hash 列(表示パス昇順=収集順で決定的)
    pub(crate) sources: Vec<JsonSource>,
    pub(crate) targets: JsonTargets,
    pub(crate) operational_errors: Vec<JsonOperationalError>,
    pub(crate) rules: Vec<JsonRuleStatus>,
    pub(crate) candidate_search: JsonCandidateSearch,
}

/// 実行コンテキスト(§7.3)を組み立てる。
/// `dict_id` は実行冒頭で 1 回だけ解決したものを渡す(辞書の再読みを避け、
/// ワーカーの再ロード判定と同じ実体識別を共有する、#36)
pub(crate) fn build_context(
    paths: &[PathBuf],
    config: &Config,
    dict_id: &DictIdentity,
    results: ContextResults,
) -> JsonContext {
    JsonContext {
        roots: paths.iter().map(|p| normalize_display(p)).collect(),
        config_fingerprint: config_fingerprint(config),
        sources: results.sources,
        algorithms: algorithms(config, dict_id),
        targets: results.targets,
        operational_errors: results.operational_errors,
        rules: results.rules,
        candidate_search: results.candidate_search,
    }
}

/// `corpus.pair_search` を JSON 用の候補列挙記録に変換する(#36)。
/// 列挙した候補は全て採点されるため scored_pairs = candidates。
pub(crate) fn candidate_search_json(pair_search: &jaio_rules::PairSearch) -> JsonCandidateSearch {
    let all_pairs = pair_search.method == PairSearchMethod::AllPairs;
    JsonCandidateSearch {
        method: if all_pairs {
            "all_pairs"
        } else {
            "simhash_banding"
        },
        all_pairs,
        comparable_docs: pair_search.comparable_docs,
        candidates: pair_search.candidates,
        scored_pairs: pair_search.candidates,
        cutoff: pair_search.cutoff,
    }
}

/// コーパス未構築(対象ゼロ)のときの候補列挙記録(#36)。
/// 対象ゼロでは近似は起きないので all_pairs は真として記録する
pub(crate) fn empty_candidate_search(config: &Config) -> JsonCandidateSearch {
    JsonCandidateSearch {
        method: "all_pairs",
        all_pairs: true,
        comparable_docs: 0,
        candidates: 0,
        scored_pairs: 0,
        // Corpus::new と同じく 2 閾値の低い方(§5.4、#39)
        cutoff: config
            .detection
            .doc_similarity
            .min(config.merge.min_similarity),
    }
}

/// lint 設定でルールが抑制されているか(#36)。lint.rules 個別指定が優先し、
/// なければ lint.default が効く(`Config::apply` と同じ解決順)。
/// 抑制されたルールは実行自体を省くため、状態記録と入力取得の判断にも使う
pub(crate) fn rule_suppressed(config: &Config, id: &str) -> bool {
    matches!(
        config.lint.rules.get(id).or(config.lint.default.as_ref()),
        Some(SeveritySetting::Allow)
    )
}

/// ルール実行結果の集計(`rule_statuses` の入力、#36)。
/// ルールごとの失敗・適用範囲・コメント入力の状況を束ねて渡す
pub(crate) struct RuleRunSummary<'a> {
    /// 解析できた文書数
    pub(crate) analyzed: usize,
    /// コメント抽出できたソースファイル数
    pub(crate) comment_files: usize,
    /// コメント抽出に失敗があったか
    pub(crate) comment_input_failed: bool,
    /// DocumentRule の失敗件数(rule_id → 失敗ファイル数)
    pub(crate) doc_failures: &'a HashMap<String, usize>,
    /// CorpusRule の失敗ルール ID 集合
    pub(crate) corpus_failures: &'a HashSet<String>,
    /// DocumentRule の適用範囲の全ファイル合算
    pub(crate) doc_coverage: &'a HashMap<&'static str, Coverage>,
    /// CorpusRule の適用範囲
    pub(crate) corpus_coverage: &'a HashMap<&'static str, Coverage>,
    /// `[lint.rule_excludes]` のパス照合で抑制した診断数(rule_id → 件数、#82)
    pub(crate) path_suppressed: &'a HashMap<&'static str, usize>,
}

/// 全ルールの実行状態を求める(#36)。「実行完了」「設定で抑制」「入力不足」
/// 「失敗」を区別し、結果の見た目だけで健全さを誤認させない。
/// `coverage` はルール自身が報告した適用範囲で、診断数とは独立に
/// 「対象がなかった」「計測できなかった」を表す
pub(crate) fn rule_statuses(config: &Config, run: &RuleRunSummary<'_>) -> Vec<JsonRuleStatus> {
    // coverage から状態を引く。判定対象があったのに 1 件も計測できなかった
    // 場合(measured == 0)と、ルールが自ら対象外・不足を宣言した場合
    // (reason あり)を insufficient_inputs とする
    fn coverage_status(cov: Option<&Coverage>) -> (&'static str, Option<String>) {
        match cov {
            Some(cov) if cov.measured == 0 && (cov.targets > 0 || cov.reason.is_some()) => (
                "insufficient_inputs",
                Some(
                    cov.reason
                        .clone()
                        .unwrap_or_else(|| "判定可能な対象を計測できませんでした".to_string()),
                ),
            ),
            _ => ("executed", None),
        }
    }
    let mut out = Vec::new();
    for rule in default_rules(config) {
        let id = rule.id();
        let cov = run.doc_coverage.get(id);
        let (status, reason) = if rule_suppressed(config, id) {
            (
                "disabled",
                Some("lint 設定で診断を抑制されています".to_string()),
            )
        } else if run.analyzed == 0 {
            (
                "insufficient_inputs",
                Some("解析対象の文書がありません".to_string()),
            )
        } else if let Some(&n) = run.doc_failures.get(id) {
            ("failed", Some(format!("{n} ファイルで実行に失敗しました")))
        } else {
            coverage_status(cov)
        };
        out.push(JsonRuleStatus {
            rule_id: id,
            status,
            reason,
            targets: cov.map(|c| c.targets),
            measured: cov.map(|c| c.measured),
            suppressed_by_path: run.path_suppressed.get(id).copied(),
        });
    }
    for rule in default_corpus_rules(config, &[]) {
        let id = rule.id();
        let cov = run.corpus_coverage.get(id);
        let (status, reason) = if rule_suppressed(config, id) {
            (
                "disabled",
                Some("lint 設定で診断を抑制されています".to_string()),
            )
        } else if run.corpus_failures.contains(id) {
            (
                "failed",
                Some("実行に失敗しました(operational_errors 参照)".to_string()),
            )
        } else if run.analyzed == 0 {
            // 文書ゼロではコーパスルールも実行したことにならない
            (
                "insufficient_inputs",
                Some("解析対象の文書がありません".to_string()),
            )
        } else if id == "relation/doc-comment-drift" && run.comment_files == 0 {
            (
                "insufficient_inputs",
                Some(
                    if run.comment_input_failed {
                        "コメント抽出に失敗し、比較入力がありません"
                    } else {
                        "コメント抽出対象のソースファイルがありません"
                    }
                    .to_string(),
                ),
            )
        } else {
            coverage_status(cov)
        };
        out.push(JsonRuleStatus {
            rule_id: id,
            status,
            reason,
            targets: cov.map(|c| c.targets),
            measured: cov.map(|c| c.measured),
            suppressed_by_path: run.path_suppressed.get(id).copied(),
        });
    }
    out
}

/// 実効設定の正規化ハッシュ(§7.3)。Config のマップ型フィールドは BTreeMap
/// (キー順が決定的)なので、直列化結果は実行・環境に依らず同じになる。
/// serde_json の preserve_order 有効時(IndexMap)でも同じ結果を保つ。
pub(crate) fn config_fingerprint(config: &Config) -> String {
    // 直列化は Config の形から実行時失敗しないはずだが、失敗時に「全設定が
    // 同じ」fingerprint を静かに流通させない(#110)。呼び出し元で
    // operational error に変換される
    let canonical = serde_json::to_value(config)
        .and_then(|v| serde_json::to_string(&v))
        .expect("Config の直列化は常に成功するはず");
    format!("sha256:{:x}", sha2::Sha256::digest(canonical.as_bytes()))
}
