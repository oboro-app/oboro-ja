use super::*;

/// スレッドローカルに持つワーカー状態の設定キー(#36)。
/// 同一プロセスで設定 A→B を順次実行した場合に A の Analyzer・TokenCounter を
/// 誤って再利用しないため、設定が変われば作り直す。
/// 外部辞書はパスだけでなく内容 fingerprint もキーに含め、同一パスの
/// 実体差し替えで古い辞書のワーカーを使い回さない。
/// 比較は `is_reusable_with` で行う(= ではない): 識別不能な辞書は自身とも
/// 再利用不可になるため、等値の反射性を満たさない判定を PartialEq にしない
#[derive(Clone)]
pub(crate) struct WorkerKey {
    /// lindera の解析モード
    pub(crate) mode: AnalysisMode,
    /// 外部辞書ディレクトリ(未指定なら埋め込み)
    pub(crate) dictionary: Option<String>,
    /// 外部辞書の内容 fingerprint。埋め込み・識別不能時は None
    pub(crate) dict_fingerprint: Option<String>,
    /// トークン計測エンコーディング
    pub(crate) encoding: TokenEncoding,
}

impl WorkerKey {
    /// `self` で構築したワーカーを `other` の要求に再利用できるか。
    /// 外部辞書は両者とも同じ fingerprint を持つときだけ再利用できる。
    /// 識別不能(None)なら実体の同一性を証明できないため必ず作り直す
    pub(crate) fn is_reusable_with(&self, other: &Self) -> bool {
        if self.dictionary.is_some()
            && !matches!(
                (&self.dict_fingerprint, &other.dict_fingerprint),
                (Some(a), Some(b)) if a == b
            )
        {
            return false;
        }
        self.mode == other.mode
            && self.dictionary == other.dictionary
            && self.encoding == other.encoding
    }

    /// 設定値が完全に同一か。失敗結果の再利用判定に使う(#44)。
    /// `is_reusable_with` は識別不能な辞書を自身とも再利用不可とするが、
    /// 失敗のメモ化は「同一入力への再試行を省く」だけなのでフィールド一致でよい
    fn is_same_key(&self, other: &Self) -> bool {
        self.mode == other.mode
            && self.dictionary == other.dictionary
            && self.dict_fingerprint == other.dict_fingerprint
            && self.encoding == other.encoding
    }
}

/// スレッドローカルのワーカースロット(#44)。
/// 初期化失敗(Failed)もキーつきで保持し、同一キーでの辞書ロード再試行
/// (ファイルごとの無駄な I/O)を防ぐ
pub(crate) enum WorkerSlot {
    /// 構築済みのワーカー状態
    Ready(Box<WorkerState>),
    /// 直近の初期化失敗(キー・エラーメッセージ)
    Failed(WorkerKey, String),
}

/// スレッドローカルのワーカー状態(Analyzer・TokenCounter・形態素キャッシュ)
pub(crate) struct WorkerState {
    /// この状態を構築した設定のキー
    pub(crate) key: WorkerKey,
    /// 形態素解析器
    pub(crate) analyzer: Analyzer,
    /// トークン計測器
    pub(crate) counter: TokenCounter,
}

/// ファイル 1 つ分の解析結果(ファイル単位並列化のワーカー戻り値、§10)。
pub(crate) struct PerFile {
    pub(crate) doc: Doc,
    pub(crate) leaves: Vec<TextLeaf>,
    pub(crate) headings: Vec<Heading>,
    /// 正規化済み本文(重複検出の入力、§5.4)
    pub(crate) body: String,
    pub(crate) tokens: usize,
    pub(crate) mtime: SystemTime,
    pub(crate) diagnostics: Vec<Diagnostic>,
    /// (rule_id, 所要時間, 診断数) — マージ側で累計する
    pub(crate) rule_stats: Vec<(&'static str, Duration, usize)>,
    /// ルールごとの適用範囲(rule_id → coverage、#36)。マージ側で全ファイル合算する
    pub(crate) coverages: Vec<(&'static str, Coverage)>,
    /// パス除外で抑制した診断数(rule_id → 件数、#82)。マージ側で合算する
    pub(crate) path_suppressed: Vec<(&'static str, usize)>,
    /// 入力ソースの SHA-256(実行コンテキストの再現性追跡用、§7.3)
    pub(crate) sha256: String,
    /// このファイル処理で起きた致命的でない失敗(operational error、#36)
    pub(crate) errors: Vec<JsonOperationalError>,
}

/// ファイル 1 つ分の解析の成否(#36)。失敗は解析を止めず
/// operational error として context に記録する
pub(crate) enum FileOutcome {
    /// 解析できた(PerFile は大きいので Box 化する)
    Analyzed(Box<PerFile>),
    /// 読み込み失敗等でスキップした
    Skipped(JsonOperationalError),
}

/// パース・DocumentRule 実行までのファイル単位の処理(並列単位、§10)。
/// 読み込みに失敗したファイルは警告を出して Skipped を返す(Issue #21)。
/// ルール失敗も解析全体は止めず、PerFile.errors に記録する(#36)
pub(crate) fn analyze_file(
    file: &PathBuf,
    rules: &[Box<dyn DocumentRule>],
    config: &Config,
    rule_excludes: &RuleExcludes,
    analyzer: &mut Analyzer,
    counter: &TokenCounter,
    cache: &mut HashMap<String, Arc<[Morph]>>,
) -> Result<FileOutcome> {
    // 読めないファイル(無効な UTF-8 等)は解析全体を中止せず、警告して
    // スキップする。壊れたファイルが混在するリポジトリでも他の文書解析を継続できる
    let source = match fs::read_to_string(file) {
        Ok(source) => source,
        Err(err) => {
            eprintln!(
                "warning: 読み込みをスキップしました({err}): {} — 対象外にするには jaio.toml の [paths] exclude に追加してください",
                file.display()
            );
            return Ok(FileOutcome::Skipped(JsonOperationalError {
                path: Some(normalize_display(file)),
                stage: "read".to_string(),
                reason: err.to_string(),
            }));
        }
    };
    // 読み取り専用: ファイルは変更しない(§7.4)。内容の SHA-256 を取り、
    // 実行コンテキスト(context.sources)で再現性を追跡できるようにする
    let sha256 = format!("{:x}", sha2::Sha256::digest(source.as_bytes()));
    let tokens = counter.count(&source);

    let (mut doc, leaves, headings) = Doc::parse(file.clone(), source);
    // 照合・表示とも display(normalize_display)を正とする。パターン照合系の
    // パス基準の統一は jaio_core::relativize_for_match が担う(#76/#87/#89)
    doc.display = normalize_display(file);
    let mut diagnostics = Vec::new();
    let mut rule_stats = Vec::with_capacity(rules.len());
    let mut coverages = Vec::with_capacity(rules.len());
    let mut path_suppressed: Vec<(&'static str, usize)> = Vec::new();
    // 致命的でないファイル単位の失敗(operational error、#36)
    let mut errors = Vec::new();
    for rule in rules {
        let mut ctx = DocContext {
            doc: &doc,
            leaves: &leaves,
            headings: &headings,
            analyzer,
            counter,
            cache,
            detection: &config.detection,
        };
        let rule_started = Instant::now();
        // ルール失敗は解析全体を止めず、ファイル単位の operational error にする
        let run = match rule.run(&mut ctx) {
            Ok(run) => run,
            Err(err) => {
                errors.push(JsonOperationalError {
                    path: Some(normalize_display(file)),
                    stage: format!("rule:{}", rule.id()),
                    reason: format!("{err:#}"),
                });
                continue;
            }
        };
        coverages.push((rule.id(), run.coverage));
        rule_stats.push((rule.id(), rule_started.elapsed(), run.diagnostics.len()));
        // ルール別の適用除外パス(#75)→ 設定の重大度上書きと allow 抑制の順に適用。
        // パス照合で抑えた件数は context.rules に反映する(#82)
        let mut suppressed = 0usize;
        diagnostics.extend(
            run.diagnostics
                .into_iter()
                .filter(|d| {
                    if rule_excludes.is_suppressed(d.rule_id, Path::new(&d.file)) {
                        suppressed += 1;
                        false
                    } else {
                        true
                    }
                })
                .filter_map(|d| config.apply(d)),
        );
        if suppressed > 0 {
            path_suppressed.push((rule.id(), suppressed));
        }
    }
    // body 連結・mtime の条件付き取得は dump と共通の変換を使う(#96)。
    // mtime 取得失敗は operational error として記録しつつ解析済みとして
    // 残す(#36)
    let corpus_input = corpus::corpus_doc_input(file, &leaves, config.merge.weights.recency > 0.0);
    let body = corpus_input.body;
    if let Some(err) = corpus_input.metadata_error {
        errors.push(err);
    }
    Ok(FileOutcome::Analyzed(Box::new(PerFile {
        doc,
        leaves,
        headings,
        body,
        tokens,
        mtime: corpus_input.mtime,
        diagnostics,
        rule_stats,
        coverages,
        path_suppressed,
        sha256,
        errors,
    })))
}

/// ドキュメントを解析して診断を報告する。
/// `quiet` が真のとき error 重大度の診断のみ報告・集計する(§7.2)。
/// 解析対象のファイルは変更しない(読み取り専用、§7.4)。
/// `verbose` が真のときフェーズ別・ルール別の計測をサマリに載せる(Issue #18)。
pub fn run_check(
    paths: &[PathBuf],
    out: &mut impl Write,
    config: &Config,
    format: &OutputFormat,
    quiet: bool,
    verbose: bool,
) -> Result<JsonSummary> {
    // フェーズ別計測: collect = ファイル収集(Issue #18)
    let collect_started = Instant::now();
    // 走査エラーは致命的でない限り operational error として記録する(#36)
    let (files, mut scan_errors) = collect_files_reporting(paths, config, is_markdown)?;
    // 走査が失敗なく完走したか(false なら発見数は下限)
    let discovery_complete = scan_errors.is_empty();
    let mut operational_errors: Vec<JsonOperationalError> = Vec::new();
    operational_errors.append(&mut scan_errors);
    let collect_ms = collect_started.elapsed().as_millis() as u64;
    // 辞書実体の識別は実行ごと 1 回だけ行い、ワーカーの再ロード判定と
    // context 出力で同じ結果を共有する(#36)
    let dict_id = dictionary_identity(config.analysis.dictionary.as_deref());
    if files.is_empty() {
        // 対象がなければ形態素解析器(辞書ロード)もソース走査も不要。
        // 走査の成否とルール実行状態は context に残し、空でも v2 形を保つ(#36)
        let rules = rule_statuses(
            config,
            &RuleRunSummary {
                analyzed: 0,
                comment_files: 0,
                comment_input_failed: false,
                doc_failures: &HashMap::new(),
                corpus_failures: &HashSet::new(),
                doc_coverage: &HashMap::new(),
                corpus_coverage: &HashMap::new(),
                path_suppressed: &HashMap::new(),
            },
        );
        let targets = JsonTargets {
            discovered: 0,
            analyzed: 0,
            skipped: 0,
            discovery_complete,
        };
        let analysis_complete = discovery_complete && operational_errors.is_empty();
        let context = build_context(
            paths,
            config,
            &dict_id,
            ContextResults {
                sources: Vec::new(),
                targets,
                operational_errors,
                rules,
                candidate_search: empty_candidate_search(config),
            },
        );
        let summary = summarize(0, 0, &[], None, analysis_complete);
        render_result(format, out, &[], &summary, &context)?;
        return Ok(summary);
    }
    // lint 設定で抑制されたルールは実行しない(#36)。診断は apply で捨てられる
    // ため結果は同じだが、形態素解析やコメント抽出などの入力取得ごと省ける
    let rules: Vec<_> = default_rules(config)
        .into_iter()
        .filter(|rule| !rule_suppressed(config, rule.id()))
        .collect();
    let corpus_rules: Vec<_> = default_corpus_rules(config, paths)
        .into_iter()
        .filter(|rule| !rule_suppressed(config, rule.id()))
        .collect();
    // [lint.rule_excludes] のコンパイル済みマッチャ(#75)。診断適用の直前で
    // 使い回す(glob コンパイルは重いため、StyleOverrides と同じ方針)。
    // 絶対パス診断の相対化基準には jaio.toml の配置ディレクトリを使う(#76)
    let rule_excludes = RuleExcludes::build(&config.lint.rule_excludes, config.toml_base())?;

    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let mut total_tokens = 0usize;
    let mut docs: Vec<Doc> = Vec::new();
    // 実行コンテキストの content hash 列(§7.3)
    let mut sources: Vec<JsonSource> = Vec::new();
    // Corpus 用の付帯情報(§5.4/§5.5 の重複検出・統合提案で使う)
    let mut bodies: Vec<String> = Vec::new();
    let mut doc_tokens: Vec<usize> = Vec::new();
    let mut mtimes: Vec<SystemTime> = Vec::new();
    // セクション抽出の入力(ファイルごとの leaves/headings、§5.4)
    let mut section_inputs: Vec<(Vec<TextLeaf>, Vec<Heading>)> = Vec::new();
    // ルール別の累計計測(rule_id → (所要時間, 診断数)、Issue #18)。
    // 時間は Duration で蓄積し、サマリ化のときにミリ秒へ変換する
    let mut rule_timings: HashMap<&'static str, (Duration, usize)> = HashMap::new();

    // フェーズ別計測: parse = ドキュメントパース + DocumentRule 実行。
    // ファイル単位で rayon 並列化する(§10、Issue #14)。Analyzer・
    // TokenCounter・形態素キャッシュはスレッドローカルに持ち、collect は
    // インデックス順を保持するため結果の並びは逐次実行と同じ
    let parse_started = Instant::now();
    // スレッドローカル状態は設定キーつきで管理し、同一プロセスで設定を変えて
    // run_check を呼んだ場合に旧設定の Analyzer・TokenCounter を誤用しない(#36)
    let worker_key = WorkerKey {
        mode: config.analysis.mode,
        dictionary: config.analysis.dictionary.clone(),
        dict_fingerprint: dict_id.fingerprint.clone(),
        encoding: config.analysis.encoding,
    };
    // map_init はワークスティールのたびに初期化を呼び直すことがあるため、
    // 重いワーカー状態は thread_local に遅延初期化してスレッドごと 1 度に抑える。
    // 失敗(Err)もキャッシュして同一キーでの再試行を防ぐ(#44: 外部辞書の
    // パス誤り等でファイルごとに辞書ロードを再試行する無駄な I/O を防ぐ)
    thread_local! {
        static WORKER_STATE: RefCell<Option<WorkerSlot>> = const { RefCell::new(None) };
    }
    let results: Vec<Result<FileOutcome>> = files
        .par_iter()
        .map(|file| {
            WORKER_STATE.with(|cell| {
                let mut slot = cell.borrow_mut();
                let need_init = match &*slot {
                    Some(WorkerSlot::Ready(state)) => !state.key.is_reusable_with(&worker_key),
                    Some(WorkerSlot::Failed(key, _)) => !key.is_same_key(&worker_key),
                    None => true,
                };
                if need_init {
                    *slot = Some(
                        match Analyzer::with_options(
                            worker_key.mode,
                            worker_key.dictionary.as_deref(),
                        )
                        .and_then(|analyzer| {
                            TokenCounter::with_encoding(worker_key.encoding).map(|counter| {
                                WorkerState {
                                    key: worker_key.clone(),
                                    analyzer,
                                    counter,
                                }
                            })
                        }) {
                            Ok(state) => WorkerSlot::Ready(Box::new(state)),
                            Err(err) => WorkerSlot::Failed(worker_key.clone(), err.to_string()),
                        },
                    );
                }
                match slot.as_mut().expect("直前に初期化") {
                    WorkerSlot::Ready(state) => {
                        // 形態素キャッシュはファイル単位に作る(#40)。
                        // スレッド永続のキャッシュは一意文の多いコーパスで
                        // メモリが入力の約 170 倍に膨らむ実測があり、設計 §10 の
                        // 趣旨(同一見出し・頻出句の再解析回避)はファイル内の
                        // 再利用でほぼ達成できる
                        let mut file_cache: HashMap<String, Arc<[Morph]>> = HashMap::new();
                        analyze_file(
                            file,
                            &rules,
                            config,
                            &rule_excludes,
                            &mut state.analyzer,
                            &state.counter,
                            &mut file_cache,
                        )
                    }
                    WorkerSlot::Failed(_, message) => anyhow::bail!("{message}"),
                }
            })
        })
        .collect();
    let mut skipped_files = 0usize;
    // ルール単位の失敗件数(rule_id → 失敗ファイル数、#36 の rules.status 用)
    let mut doc_failures: HashMap<String, usize> = HashMap::new();
    // DocumentRule の適用範囲の合算(rule_id → 全ファイル合計の coverage、#36)
    let mut doc_coverage: HashMap<&'static str, Coverage> = HashMap::new();
    // パス除外(lint.rule_excludes)で抑制した診断数の合算(Doc/Corpus 両方、#82)
    let mut path_suppressed: HashMap<&'static str, usize> = HashMap::new();
    for result in results {
        // 読み込みに失敗したファイルは analyze_file 内で警告済み(Skipped として返る)
        let file_result = match result? {
            FileOutcome::Analyzed(file_result) => file_result,
            FileOutcome::Skipped(error) => {
                skipped_files += 1;
                operational_errors.push(error);
                continue;
            }
        };
        for error in &file_result.errors {
            if let Some(id) = error.stage.strip_prefix("rule:") {
                *doc_failures.entry(id.to_string()).or_default() += 1;
            }
        }
        operational_errors.extend(file_result.errors);
        total_tokens += file_result.tokens;
        sources.push(JsonSource {
            path: file_result.doc.display.clone(),
            sha256: file_result.sha256,
        });
        diagnostics.extend(file_result.diagnostics);
        for (rule_id, elapsed, count) in file_result.rule_stats {
            let entry = rule_timings.entry(rule_id).or_default();
            entry.0 += elapsed;
            entry.1 += count;
        }
        // ルールの適用範囲を全ファイルで合算する(#36)。不足理由は
        // 最初に観測したものを代表として残す
        for (rule_id, coverage) in file_result.coverages {
            let entry = doc_coverage.entry(rule_id).or_default();
            entry.targets += coverage.targets;
            entry.measured += coverage.measured;
            if entry.reason.is_none() {
                entry.reason = coverage.reason;
            }
        }
        // パス除外で抑制した診断数を合算する(#82)
        for (rule_id, count) in file_result.path_suppressed {
            *path_suppressed.entry(rule_id).or_default() += count;
        }
        bodies.push(file_result.body);
        doc_tokens.push(file_result.tokens);
        mtimes.push(file_result.mtime);
        section_inputs.push((file_result.leaves, file_result.headings));
        docs.push(file_result.doc);
    }
    let parse_ms = parse_started.elapsed().as_millis() as u64;

    // フェーズ別計測: corpus = Corpus 構築(前処理込み)+ CorpusRule 実行
    let corpus_started = Instant::now();
    // リポジトリ横断ルールは全ドキュメントの解析後に実行する(設計書 §6.1)
    // comment 収集・sources 追加・セクション構築・Corpus 構築は dump と
    // 共通の変換を使う(#96)。抽出失敗があったか(doc-comment-drift の
    // 「入力なし」と「収集失敗」を区別する、#36)
    let (comment_files, mut comment_errors) =
        corpus::collect_comment_inputs(paths, config, &mut sources)?;
    let comment_input_failed = !comment_errors.is_empty();
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
    // コーパスルールの失敗は解析全体を止めず operational error にする(#36)
    let mut corpus_failures: HashSet<String> = HashSet::new();
    let mut corpus_coverage: HashMap<&'static str, Coverage> = HashMap::new();
    for rule in &corpus_rules {
        let rule_started = Instant::now();
        let run = match rule.run(&corpus) {
            Ok(run) => run,
            Err(err) => {
                corpus_failures.insert(rule.id().to_string());
                operational_errors.push(JsonOperationalError {
                    path: None,
                    stage: format!("rule:{}", rule.id()),
                    reason: format!("{err:#}"),
                });
                continue;
            }
        };
        corpus_coverage.insert(rule.id(), run.coverage);
        let entry = rule_timings.entry(rule.id()).or_default();
        entry.0 += rule_started.elapsed();
        entry.1 += run.diagnostics.len();
        // ルール別の適用除外パス(#75)→ 重大度上書きと allow 抑制の順に適用。
        // パス照合で抑えた件数は context.rules に反映する(#82)
        let rule_id = rule.id();
        let mut suppressed = 0usize;
        diagnostics.extend(
            run.diagnostics
                .into_iter()
                .filter(|d| {
                    if rule_excludes.is_suppressed(d.rule_id, Path::new(&d.file)) {
                        suppressed += 1;
                        false
                    } else {
                        true
                    }
                })
                .filter_map(|d| config.apply(d)),
        );
        if suppressed > 0 {
            *path_suppressed.entry(rule_id).or_default() += suppressed;
        }
    }
    let corpus_ms = corpus_started.elapsed().as_millis() as u64;

    diagnostics.sort_by(|a, b| {
        // 最後に観察文順を足し、同一位置・同一ルールの診断も決定的に並べる(§7.3)
        (&a.file, a.span.line, a.span.col, a.rule_id, &a.observation).cmp(&(
            &b.file,
            b.span.line,
            b.span.col,
            b.rule_id,
            &b.observation,
        ))
    });

    if quiet {
        diagnostics.retain(|d| d.severity == Severity::Error);
    }
    // --verbose 時のみ計測をサマリに載せる(未指定時の出力は従来と完全に同じ)
    let timings = RunTimings {
        collect_ms,
        parse_ms,
        corpus_ms,
        rules: rule_timings
            .into_iter()
            .map(|(id, (elapsed, diags))| (id, (elapsed.as_millis() as u64, diags)))
            .collect(),
    };
    let analyzed = files.len().saturating_sub(skipped_files);
    // 走査・読み込み・ルール実行の全てが完了したか(#36)。operational_errors は
    // 致命的でない失敗の総勘定なので、空であることが完全性の条件になる
    let analysis_complete = operational_errors.is_empty();
    let summary = summarize(
        analyzed,
        total_tokens,
        &diagnostics,
        verbose.then_some(timings).as_ref(),
        analysis_complete,
    );
    let targets = JsonTargets {
        discovered: files.len(),
        analyzed,
        skipped: skipped_files,
        discovery_complete,
    };
    let rules_status = rule_statuses(
        config,
        &RuleRunSummary {
            analyzed,
            comment_files: comment_files.len(),
            comment_input_failed,
            doc_failures: &doc_failures,
            corpus_failures: &corpus_failures,
            doc_coverage: &doc_coverage,
            corpus_coverage: &corpus_coverage,
            path_suppressed: &path_suppressed,
        },
    );
    let candidate_search = candidate_search_json(&corpus.pair_search);
    let context = build_context(
        paths,
        config,
        &dict_id,
        ContextResults {
            sources,
            targets,
            operational_errors,
            rules: rules_status,
            candidate_search,
        },
    );
    render_result(format, out, &diagnostics, &summary, &context)?;
    Ok(summary)
}

/// 診断列とサマリを指定形式で出力する(§7.2)。
/// `context` は JSON 形式でのみ出力に含まれる(§7.3)。
pub(crate) fn render_result(
    format: &OutputFormat,
    out: &mut impl Write,
    diagnostics: &[Diagnostic],
    summary: &JsonSummary,
    context: &JsonContext,
) -> Result<()> {
    match format {
        OutputFormat::Terminal { color } => {
            jaio_report::render(out, diagnostics, *color)?;
            write_check_summary(out, summary)?;
        }
        OutputFormat::Json => {
            let report = jaio_report::JsonReport::new(
                summary,
                diagnostics,
                context,
                env!("CARGO_PKG_VERSION"),
            );
            jaio_report::render_json(out, &report)?;
        }
        OutputFormat::Markdown => {
            jaio_report::render_markdown(out, diagnostics, summary)?;
        }
        OutputFormat::Sarif => {
            jaio_report::render_sarif(out, diagnostics, env!("CARGO_PKG_VERSION"))?;
        }
    }
    Ok(())
}

/// `--verbose` 用の実行計測(Issue #18)。
/// フェーズ別の所要時間とルール別の累計(所要時間・診断数)を保持し、
/// `summarize` で `JsonSummary` に注入する。
pub(crate) struct RunTimings {
    /// ファイル収集(collect)の所要時間(ミリ秒)
    pub(crate) collect_ms: u64,
    /// ドキュメントパース + DocumentRule 実行(ファイルループ全体)の所要時間
    pub(crate) parse_ms: u64,
    /// Corpus 構築 + CorpusRule 実行の所要時間
    pub(crate) corpus_ms: u64,
    /// rule_id → (累計所要時間ミリ秒, 累計診断数)
    pub(crate) rules: HashMap<&'static str, (u64, usize)>,
}

pub(crate) fn summarize(
    files: usize,
    tokens: usize,
    diagnostics: &[Diagnostic],
    timings: Option<&RunTimings>,
    analysis_complete: bool,
) -> JsonSummary {
    let mut summary = JsonSummary {
        files,
        tokens,
        errors: 0,
        warnings: 0,
        infos: 0,
        analysis_complete,
        top_rules: top_rules(diagnostics),
        timings: None,
        slow_rules: None,
        hints: Vec::new(),
    };
    for diagnostic in diagnostics {
        match diagnostic.severity {
            Severity::Error => summary.errors += 1,
            Severity::Warn => summary.warnings += 1,
            Severity::Info => summary.infos += 1,
        }
    }
    // --verbose の計測があればフェーズ別時間と上位スロールールを載せる(Issue #18)
    if let Some(timings) = timings {
        summary.timings = Some(PhaseTimings {
            collect_ms: timings.collect_ms,
            parse_ms: timings.parse_ms,
            corpus_ms: timings.corpus_ms,
        });
        let entries: Vec<(String, u64, usize)> = timings
            .rules
            .iter()
            .map(|(id, (ms, diags))| ((*id).to_string(), *ms, *diags))
            .collect();
        summary.slow_rules = Some(top_slow_rules(&entries));
    }
    // 設定による緩和が見込める大量検出への案内(#91)
    summary.hints = build_hints(files, diagnostics);
    summary
}

/// 設定による緩和が見込める大量検出への案内を組み立てる(#91)。
/// orphan は入口未指定のフラットな文書群で大量に検出される。量感と比率の
/// 両方を見て「orphan が 20 件超かつ解析成功文書数の 30% 超」のときに
/// 設定案内を返す。絶対値の下限で小規模コーパス(10 文書中 3 件等)の
/// 過剰案内を防ぎ、比率で相互リンクの少ない大規模文書群を拾う(#91)。
/// 誤検出ではなく設定不足を促す実行側の助言。重大度は持たせない
/// しきい値: orphan が「20 件超」かつ「解析成功文書数の 30% 超」の両方を
/// 満たすときに案内する(#91)。由来は v0.9.1 実装時の実運用観察
/// (265 文書の実リポジトリで orphan 101 件 = 38% となり案内が必要と判断)+ 小規模コーパス
/// (10 文書中 3 件等)で過剰案内にならない量感の下限。実運用の観察から
/// 再調整する場合は #91 の 3 案を参照
const ORPHAN_HINT_MIN_COUNT: usize = 20;
const ORPHAN_HINT_MIN_RATIO_NUM: usize = 3;
const ORPHAN_HINT_MIN_RATIO_DEN: usize = 10;

fn build_hints(files: usize, diagnostics: &[Diagnostic]) -> Vec<String> {
    if files == 0 {
        return Vec::new();
    }
    let orphans = diagnostics
        .iter()
        .filter(|d| d.rule_id == "relation/orphan")
        .count();
    if orphans > ORPHAN_HINT_MIN_COUNT
        && orphans * ORPHAN_HINT_MIN_RATIO_DEN > files * ORPHAN_HINT_MIN_RATIO_NUM
    {
        vec![
            "orphan が多数検出されています。detection.orphan_entry_points の設定を検討すると検出を絞れます"
                .to_string(),
        ]
    } else {
        Vec::new()
    }
}

pub(crate) fn write_check_summary(
    out: &mut impl Write,
    summary: &JsonSummary,
) -> std::io::Result<()> {
    // 解析が未完了なら「問題なし」に見せない(#36)。失敗の内訳は
    // JSON の context.operational_errors にある
    if !summary.analysis_complete {
        // warning は stdout をパイプする利用者が見落とさないよう stderr へ(#110)
        eprintln!(
            "warning: 一部の解析が完了していません(--format json の context.operational_errors を確認してください)"
        );
    }
    if summary.files == 0 {
        writeln!(out, "対象の Markdown ファイルが見つかりません")?;
        return Ok(());
    }
    writeln!(
        out,
        "checked {} documents ({} tokens)",
        summary.files, summary.tokens
    )?;
    // 設定案内(#91)。診断ではないため重大度集計の後、ルール内訳の前に出す
    for hint in &summary.hints {
        writeln!(out, "note: {hint}")?;
    }
    let total = summary.errors + summary.warnings + summary.infos;
    if total == 0 {
        writeln!(out, "no issues found")?;
    } else {
        writeln!(
            out,
            "{} errors, {} warnings, {} infos",
            summary.errors, summary.warnings, summary.infos
        )?;
    }
    // ルール別の内訳は診断があるときだけ出す(0 件なら出さない)
    if !summary.top_rules.is_empty() {
        let entries: Vec<String> = summary
            .top_rules
            .iter()
            .map(|(rule_id, count)| format!("{rule_id} {count}"))
            .collect();
        writeln!(out, "top rules: {}", entries.join(", "))?;
    }
    // --verbose 時のみフェーズ別・ルール別の計測行を出す(Issue #18)
    if let Some(timings) = &summary.timings {
        writeln!(
            out,
            "timings: collect {}ms, parse {}ms, corpus {}ms",
            timings.collect_ms, timings.parse_ms, timings.corpus_ms
        )?;
    }
    if let Some(slow_rules) = &summary.slow_rules
        && !slow_rules.is_empty()
    {
        let entries: Vec<String> = slow_rules
            .iter()
            .map(|(rule_id, ms, diags)| format!("{rule_id} {ms}ms ({diags} diags)"))
            .collect();
        writeln!(out, "slow rules: {}", entries.join(", "))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use jaio_core::Span;

    fn orphan(file: &str) -> Diagnostic {
        Diagnostic::new(
            file.to_string(),
            Span { line: 1, col: 1 },
            "relation/orphan",
            "解析対象内で参照されていない文書です".to_string(),
        )
    }

    fn plain(file: &str) -> Diagnostic {
        Diagnostic::new(
            file.to_string(),
            Span { line: 1, col: 1 },
            "efficiency/long-sentence",
            "長い文です".to_string(),
        )
    }

    #[test]
    fn hints_suggest_entry_points_when_orphans_dominate() {
        // orphan が 20 件超かつ解析成功文書数の 30% 超のとき、設定案内を出す(#91)
        let diags: Vec<Diagnostic> = (0..25).map(|i| orphan(&format!("a{i}.md"))).collect();
        let hints = build_hints(50, &diags);
        assert_eq!(hints.len(), 1);
        assert!(hints[0].contains("orphan_entry_points"));
        // 比率が閾値(30%)直上でも量感があれば案内する(ちょうど 30% は「超」でない)
        let diags: Vec<Diagnostic> = (0..31).map(|i| orphan(&format!("b{i}.md"))).collect();
        assert_eq!(build_hints(100, &diags).len(), 1);
    }

    #[test]
    fn no_hint_when_orphans_are_minor() {
        // 量感が足りなければ案内は出さない(小規模コーパスの過剰案内を防ぐ)
        let diags: Vec<Diagnostic> = (0..5).map(|i| orphan(&format!("a{i}.md"))).collect();
        assert!(build_hints(10, &diags).is_empty());
        // 比率が 50% でも 20 件未満なら出ない
        let diags: Vec<Diagnostic> = (0..15).map(|i| orphan(&format!("c{i}.md"))).collect();
        assert!(build_hints(30, &diags).is_empty());
        // orphan 以外の診断が大量でも案内は出ない
        let diags: Vec<Diagnostic> = (0..20).map(|i| plain(&format!("b{i}.md"))).collect();
        assert!(build_hints(10, &diags).is_empty());
    }

    #[test]
    fn no_hint_without_documents() {
        // 解析対象が 0 のときは案内を出さない(0 除算の代わりに早期リターン)
        let hints = build_hints(0, &[orphan("a.md")]);
        assert!(hints.is_empty());
    }
}
