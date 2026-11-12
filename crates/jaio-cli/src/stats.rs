use super::*;

/// トークン統計を表示する。`similarity` が真のとき、続けて文書ペアの
/// bigram Jaccard 類似度分布を表示する(閾値チューニング用、Issue #17)。
/// `jaio stats` のファイル別計測結果(run_stats の内部で使う)
pub(crate) struct StatsEntry {
    /// 表示パス
    pub(crate) display: String,
    /// 全体の文字数
    pub(crate) chars: usize,
    /// 全体のトークン数
    pub(crate) tokens: usize,
    /// 100 文字あたりトークン数
    pub(crate) density: f64,
    /// frontmatter の (chars, tokens)。無い場合は None(#53)
    pub(crate) frontmatter: Option<(usize, usize)>,
    /// 本文の (chars, tokens)
    pub(crate) body: (usize, usize),
}

/// ソースを frontmatter と本文に分けて (chars, tokens) を計測する(#53)。
/// frontmatter は Doc::parse(comrak)が検出したものを受け取る(#103)。
/// ここで手実装の検出を書かないのは setext 見出し等のエッジケースで
/// comrak と規則がずれるため。frontmatter が無い場合は None
pub(crate) fn split_frontmatter(
    source: &str,
    frontmatter: Option<&str>,
    counter: &TokenCounter,
) -> (Option<(usize, usize)>, (usize, usize)) {
    match frontmatter {
        // literal はソース先頭のバイト列と一致する(comrak が切り出す)。
        // 長さ判定と char boundary は念のための防御
        Some(fm) if source.len() >= fm.len() && source.is_char_boundary(fm.len()) => {
            let body = &source[fm.len()..];
            (
                Some((fm.chars().count(), counter.count(fm))),
                (body.chars().count(), counter.count(body)),
            )
        }
        _ => (None, (source.chars().count(), counter.count(source))),
    }
}

/// `jaio stats` の出力形式(#50)。
pub enum StatsFormat {
    /// 人間向けのターミナル出力(表形式)
    Terminal,
    /// 層別集計の自動化向け JSON 出力(kind: jaio/stats、§7.2)
    Json,
}

/// 戻り値は解析が完全に完了したか(読み込み・走査失敗が無い)。check と同じ
/// 終了コード契約(解析未完了 = exit 2)に stats も乗せる(#93、§7.2)
pub fn run_stats(
    paths: &[PathBuf],
    out: &mut impl Write,
    config: &Config,
    similarity: bool,
    format: StatsFormat,
) -> Result<bool> {
    // check と同じ走査で、致命的でない走査失敗も記録する(§7.2 の context 契約)
    let (files, mut operational_errors) = collect_files_reporting(paths, config, is_markdown)?;
    let discovery_complete = operational_errors.is_empty();
    let counter = TokenCounter::with_encoding(config.analysis.encoding)?;

    let mut readable: Vec<(PathBuf, String)> = Vec::new();
    for file in &files {
        match fs::read_to_string(file) {
            Ok(source) => readable.push((file.clone(), source)),
            Err(err) => {
                // 読めないファイルは check と同じく警告してスキップする(#44)。
                // JSON 出力では operational_errors に記録し、terminal では
                // 従来どおり stderr に警告を出す
                if matches!(format, StatsFormat::Terminal) {
                    eprintln!(
                        "warning: 読み込みをスキップしました({err}): {} — 対象外にするには jaio.toml の [paths] exclude に追加してください",
                        file.display()
                    );
                }
                operational_errors.push(JsonOperationalError {
                    path: Some(normalize_display(file)),
                    stage: "read".to_string(),
                    reason: err.to_string(),
                });
            }
        }
    }

    // ファイルごとの計測。frontmatter と本文の分離計測も行う
    // (#53: ハーネス指示の description は「常時コンテキストに載る層」で、
    // 本文(発火時)と分けて評価する)
    let mut entries: Vec<StatsEntry> = Vec::new();
    // 入力識別(§7.3 の sources と同じ形式。コスト評価の再現性管理用、#50)
    let mut sources: Vec<JsonSource> = Vec::new();
    let mut total_chars = 0usize;
    let mut total_tokens = 0usize;
    let mut parsed: Vec<(PathBuf, Doc, Vec<TextLeaf>)> = Vec::new();
    for (file, source) in &readable {
        let display = normalize_display(file);
        let chars = source.chars().count();
        let tokens = counter.count(source);
        let density = if chars > 0 {
            tokens as f64 * 100.0 / chars as f64
        } else {
            0.0
        };
        // parse は 1 回に統一し、frontmatter・類似度(--similarity 時)で
        // 結果を使い回す(#103。従来はファイルごとに 2〜3 回 parse していた)
        let (doc, leaves, _) = Doc::parse(file.clone(), source.clone());
        let (frontmatter, body) = split_frontmatter(source, doc.frontmatter.as_deref(), &counter);
        sources.push(JsonSource {
            sha256: format!("{:x}", sha2::Sha256::digest(source.as_bytes())),
            path: display.clone(),
        });
        entries.push(StatsEntry {
            display,
            chars,
            tokens,
            density,
            frontmatter,
            body,
        });
        total_chars += chars;
        total_tokens += tokens;
        parsed.push((file.clone(), doc, leaves));
    }

    let dist = if similarity {
        Some(similarity_distribution(&parsed, config)?)
    } else {
        None
    };

    match format {
        StatsFormat::Terminal => {
            writeln!(
                out,
                "{:<44} {:>8} {:>8} {:>11}",
                "file", "chars", "tokens", "tok/100ch"
            )?;
            for entry in &entries {
                writeln!(
                    out,
                    "{:<44} {:>8} {:>8} {:>11.1}",
                    entry.display, entry.chars, entry.tokens, entry.density
                )?;
            }
            writeln!(
                out,
                "{:<44} {:>8} {:>8}",
                "total", total_chars, total_tokens
            )?;
            if let Some(dist) = &dist {
                write_similarity_distribution(dist, out, config)?;
            }
        }
        StatsFormat::Json => {
            // algorithms は stats が実際に使う前処理・算法だけを出す。
            // 形態素解析は使わないため morphology は含めない(#50)
            let mut algorithms = serde_json::Map::new();
            let encoding = match config.analysis.encoding {
                TokenEncoding::O200kBase => "o200k_base",
                TokenEncoding::Cl100kBase => "cl100k_base",
            };
            algorithms.insert(
                "tokenizer".to_string(),
                serde_json::json!(format!("tiktoken {encoding}")),
            );
            if dist.is_some() {
                algorithms.insert(
                    "similarity".to_string(),
                    serde_json::json!("bigram-jaccard"),
                );
            }
            algorithms.insert("build".to_string(), serde_json::json!(build_manifest_id()));

            let value = serde_json::json!({
                "kind": "jaio/stats",
                "version": 1,
                "tool": { "name": "jaio", "version": env!("CARGO_PKG_VERSION") },
                "context": {
                    "roots": paths.iter().map(|p| normalize_display(p)).collect::<Vec<_>>(),
                    "config_fingerprint": config_fingerprint(config),
                    "algorithms": algorithms,
                    "sources": sources,
                    "targets": {
                        "discovered": files.len(),
                        "analyzed": readable.len(),
                        "skipped": files.len() - readable.len(),
                        "discovery_complete": discovery_complete,
                    },
                    "operational_errors": operational_errors,
                },
                "files": entries.iter().map(|e| {
                    serde_json::json!({
                        "file": e.display, "chars": e.chars, "tokens": e.tokens,
                        "tok_per_100ch": (e.density * 10.0).round() / 10.0,
                        // frontmatter(常時層)と本文(発火層)の分離計測(#53)。
                        // frontmatter が無いファイルは null
                        "frontmatter": e.frontmatter.map(|(c, t)| serde_json::json!({ "chars": c, "tokens": t })),
                        "body": { "chars": e.body.0, "tokens": e.body.1 },
                    })
                }).collect::<Vec<_>>(),
                "total": {
                    "files": entries.len(),
                    "chars": total_chars,
                    "tokens": total_tokens,
                },
                "similarity": dist.as_ref().map(|d| serde_json::json!({
                    "pairs": d.pairs,
                    "docs": d.docs,
                    "min_chars": d.min_chars,
                    "histogram": d.histogram.iter().map(|(lower, count)| {
                        serde_json::json!({ "lower": lower, "count": count })
                    }).collect::<Vec<_>>(),
                    "top_pairs": d.top_pairs.iter().map(|(sim, a, b)| {
                        serde_json::json!({ "similarity": sim, "a": a, "b": b })
                    }).collect::<Vec<_>>(),
                })),
            });
            writeln!(
                out,
                "{}",
                serde_json::to_string_pretty(&value).map_err(|e| anyhow::anyhow!(e))?
            )?;
        }
    }
    // terminal 形式では失敗の内訳が JSON に載らないため、解析未完了を明示する
    if !operational_errors.is_empty() && matches!(format, StatsFormat::Terminal) {
        eprintln!(
            "warning: 一部の解析が完了していません(失敗 {} 件)",
            operational_errors.len()
        );
    }
    let complete = operational_errors.is_empty();
    Ok(complete)
}

/// `--similarity` の分布データ(#50)。terminal 表示と JSON 出力の両方で使う。
pub(crate) struct SimilarityDistribution {
    pub(crate) pairs: usize,
    pub(crate) docs: usize,
    pub(crate) min_chars: usize,
    /// (bin 下限, 件数)。0.05 刻み
    pub(crate) histogram: Vec<(f64, usize)>,
    /// (類似度, パスA, パスB)の降順上位
    pub(crate) top_pairs: Vec<(f64, String, String)>,
}

/// `jaio stats --similarity` の分布を計算する。
/// Corpus と同じ正文抽出(本文リーフ(段落+リスト)の連結 → `normalize_text`)で
/// `[detection] doc_body_min_chars` 以上の文書の全ペアについて bigram Jaccard を
/// 計算する。行番号やコードに触れずに閾値(merge.min_similarity)チューニングの
/// 根拠を得るのが目的。
pub(crate) fn similarity_distribution(
    parsed: &[(PathBuf, Doc, Vec<TextLeaf>)],
    config: &Config,
) -> Result<SimilarityDistribution> {
    let min_chars = config.detection.doc_body_min_chars;
    let mut bodies: Vec<(String, Vec<u64>)> = Vec::new();
    for (file, _, leaves) in parsed {
        let _ = file;
        let body: String = leaves
            .iter()
            .filter(|l| DocContext::is_body_leaf(l))
            .map(|l| l.text.as_str())
            .collect();
        let normalized = normalize_text(&body);
        if normalized.chars().count() >= min_chars {
            bodies.push((normalize_display(file), bigram_hashes(&normalized)));
        }
    }
    // 全ペアの類似度(事前計算のソート済み bigram ハッシュでマージ比較する)。
    // ペアの参照はインデックスで渡し、パス文字列をペアごとに clone しない
    // (#44: 2048 文書で約 200 万ペア × String clone が数百 MB 規模になる)
    let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
    for i in 0..bodies.len() {
        for j in (i + 1)..bodies.len() {
            pairs.push((jaccard_sorted(&bodies[i].1, &bodies[j].1), i, j));
        }
    }
    let histogram = similarity_histogram(&pairs.iter().map(|p| p.0).collect::<Vec<_>>());
    let pair_count = pairs.len();
    let top_pairs = top_similar_pairs(pairs, 10)
        .into_iter()
        .map(|(sim, i, j)| (sim, bodies[i].0.clone(), bodies[j].0.clone()))
        .collect();
    Ok(SimilarityDistribution {
        pairs: pair_count,
        docs: bodies.len(),
        min_chars,
        histogram,
        top_pairs,
    })
}

/// `jaio stats --similarity` の分布部分を terminal に出力する。
pub(crate) fn write_similarity_distribution(
    dist: &SimilarityDistribution,
    out: &mut impl Write,
    config: &Config,
) -> Result<()> {
    writeln!(
        out,
        "similarity distribution ({} pairs, {} docs >= {} chars):",
        dist.pairs, dist.docs, dist.min_chars
    )?;
    // min_similarity が属す bin に注釈を付ける(それ以降の bin が統合候補になる)
    let threshold_bin = similarity_bin_index(config.merge.min_similarity);
    for (idx, (lower, count)) in dist.histogram.iter().enumerate() {
        let upper = lower + SIMILARITY_BIN_WIDTH;
        let marker = if idx == threshold_bin {
            "   <- min_similarity"
        } else {
            ""
        };
        writeln!(out, "  {lower:.2}-{upper:.2}: {count:>6}{marker}")?;
    }
    writeln!(out, "top pairs:")?;
    for (sim, a, b) in &dist.top_pairs {
        writeln!(out, "  {sim:.2}  {a} <-> {b}")?;
    }
    Ok(())
}
