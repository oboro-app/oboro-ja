//! jaio-report: 診断の出力。terminal 形式(§7.1)と JSON 形式(§7.3)、
//! Markdown 形式(§7.2)を提供する。

use std::collections::HashMap;
use std::io::Write;

use jaio_core::{Diagnostic, Measurement, MeasurementStatus, Severity};
use serde::Serialize;

/// terminal 出力で occurrences 行に表示する最大行数。
/// 表示のみの上限で根拠データ(JSON の locations)は全件保持する。
/// 値は初期ヒューリスティック(1 画面に収まる量、#110 で明記)
const MAX_SHOWN_OCCURRENCES: usize = 8;

/// サマリのルール別集計で表示する上位ルール数。
/// 初期ヒューリスティック(terminal のサマリ 1 行ブロックに収まる量、#110)
const MAX_TOP_RULES: usize = 5;

/// `--verbose` 計測で表示する上位スロールール数(Issue #18)。
/// 初期ヒューリスティック(MAX_TOP_RULES と同じ画面量、#110)
const MAX_SLOW_RULES: usize = 5;

/// 診断列を terminal 形式で出力する。
pub fn render(out: &mut impl Write, diags: &[Diagnostic], color: bool) -> std::io::Result<()> {
    for d in diags {
        let severity = paint(color, severity_color(d.severity), &d.severity.to_string());
        writeln!(
            out,
            "{}:{}:{}  {}[{}]  {}",
            d.file, d.span.line, d.span.col, severity, d.rule_id, d.observation
        )?;
        for line in detail_lines(d) {
            writeln!(out, "  {line}")?;
        }
    }
    Ok(())
}

/// 診断列とサマリを Markdown 形式で出力する(§7.2)。
pub fn render_markdown(
    out: &mut impl Write,
    diags: &[Diagnostic],
    summary: &JsonSummary,
) -> std::io::Result<()> {
    writeln!(out, "# jaio report")?;
    writeln!(out)?;
    if !summary.analysis_complete {
        // 読み込み失敗等があっても正常時と同じ見た目になるのを防ぐ(#99)。
        // 失敗の内訳は JSON の context.operational_errors を参照
        writeln!(
            out,
            "> **warning: 一部の解析が完了していません。** 欠落の内訳は `--format json` の `context.operational_errors` を確認してください。"
        )?;
        writeln!(out)?;
    }
    for d in diags {
        writeln!(
            out,
            "- `{}:{}:{}` **{}** `[{}]` {}",
            d.file, d.span.line, d.span.col, d.severity, d.rule_id, d.observation
        )?;
        for line in detail_lines(d) {
            writeln!(out, "  - {line}")?;
        }
    }
    writeln!(out)?;
    writeln!(
        out,
        "checked {} documents ({} tokens) / {} errors, {} warnings, {} infos",
        summary.files, summary.tokens, summary.errors, summary.warnings, summary.infos
    )?;
    Ok(())
}

/// review_questions・measurements・locations・limitations の補足行
/// (terminal・Markdown で共有)。
fn detail_lines(d: &Diagnostic) -> Vec<String> {
    let mut lines = Vec::new();
    for q in &d.review_questions {
        lines.push(format!("question: {q}"));
    }
    for m in &d.measurements {
        lines.push(measurement_line(m));
    }
    // 出現箇所(role=occurrence)は同一行の重複を除いて昇順に並べ、表示は
    // 上位 MAX_SHOWN_OCCURRENCES 行までとする。各出現には該当テキストの断片を
    // 併記し、行番号だけでは分からない箇所を直接伝える(Issue #19)。
    // 根拠データ(locations)自体は JSON 出力で完全な形を保つ
    let mut shown_items: Vec<(usize, String, bool)> = d
        .locations
        .iter()
        .filter(|l| l.role == "occurrence")
        .map(|l| {
            (
                l.span.line,
                l.snippet.clone().unwrap_or_default(),
                l.approximate,
            )
        })
        .collect();
    shown_items.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    shown_items.dedup();
    if !shown_items.is_empty() {
        let shown: Vec<String> = shown_items
            .iter()
            .take(MAX_SHOWN_OCCURRENCES)
            .map(|(line, snippet, approximate)| {
                let approx = if *approximate { "(approx)" } else { "" };
                format!("L{line}{approx}「{snippet}」")
            })
            .collect();
        let overflow = if shown_items.len() > MAX_SHOWN_OCCURRENCES {
            format!(" ほか{}行", shown_items.len() - MAX_SHOWN_OCCURRENCES)
        } else {
            String::new()
        };
        lines.push(format!("occurrences: {}{}", shown.join(", "), overflow));
    }
    for l in d.locations.iter().filter(|l| l.role == "counterpart") {
        lines.push(format!("counterpart: {}:{}", l.file, l.span.line));
    }
    for l in &d.limitations {
        lines.push(format!("limitation: {l}"));
    }
    lines
}

/// 計測値 1 件の表示行。`measured` 以外は値の代わりに状態と理由を出す。
fn measurement_line(m: &Measurement) -> String {
    if m.status != MeasurementStatus::Measured {
        let mut line = format!("measurement: {} ({})", m.metric, status_name(m.status));
        if let Some(reason) = &m.reason {
            line.push_str(&format!(": {reason}"));
        }
        return line;
    }
    let mut line = match m.value {
        Some(value) => format!("measurement: {}={value}", m.metric),
        None => format!("measurement: {}", m.metric),
    };
    if let Some(unit) = m.unit {
        line.push_str(&format!(" {unit}"));
    }
    let mut meta: Vec<String> = Vec::new();
    if let (Some(threshold), Some(comparison)) = (m.threshold, m.comparison) {
        meta.push(format!("{comparison}{threshold}"));
    }
    if let Some(method) = m.method {
        meta.push(method.to_string());
    }
    if let Some(scope) = &m.scope {
        meta.push(scope.clone());
    }
    if !meta.is_empty() {
        line.push_str(&format!(" ({})", meta.join(", ")));
    }
    line
}

/// 計測状態の表示名(serde の snake_case 名と同じ)。
fn status_name(status: MeasurementStatus) -> &'static str {
    match status {
        MeasurementStatus::Measured => "measured",
        MeasurementStatus::NotApplicable => "not_applicable",
        MeasurementStatus::InsufficientSample => "insufficient_sample",
        MeasurementStatus::Unavailable => "unavailable",
    }
}

/// 出力構造のバージョン(§7.3)。互換性のない変更で増やす
pub const SCHEMA_VERSION: u32 = 2;

/// JSON 出力のトップレベル構造(設計書 §7.3)。
#[derive(Serialize)]
pub struct JsonReport<'a> {
    /// 出力構造のバージョン
    pub schema_version: u32,
    /// ツール情報
    pub tool: ToolInfo<'a>,
    /// 実行コンテキスト(再現性の追跡用)
    pub context: &'a JsonContext,
    /// 実行サマリ
    pub summary: &'a JsonSummary,
    /// 診断列
    pub diagnostics: Vec<JsonDiagnostic<'a>>,
}

/// ツール情報(JSON 出力の `tool`)。
#[derive(Serialize)]
pub struct ToolInfo<'a> {
    /// ツール名(常に "jaio")
    pub name: &'static str,
    /// jaio のバージョン(Cargo.toml)
    pub version: &'a str,
}

/// 実行コンテキスト(JSON 出力の `context`、§7.3)。
/// 同一入力・設定で出力が再現できることを追跡可能にする。
/// 「問題がなかった」と「検査できなかった」の区別のため、
/// 対象の完全性・実行上の失敗・検索方式もここに記録する(#36)。
#[derive(Serialize)]
pub struct JsonContext {
    /// 解析対象のルート(コマンドラインで指定した表示パス)
    pub roots: Vec<String>,
    /// 実効設定の正規化ハッシュ("sha256:<hex>")
    pub config_fingerprint: String,
    /// 解析した入力ソースの content hash 列(パス昇順)
    pub sources: Vec<JsonSource>,
    /// 前処理・算法の識別情報
    pub algorithms: JsonAlgorithms,
    /// 対象ファイルの発見・解析・スキップ件数(#36)
    pub targets: JsonTargets,
    /// 致命的でない実行上の失敗(読み込み・走査・コメント抽出等)
    pub operational_errors: Vec<JsonOperationalError>,
    /// 有効ルールごとの実行状態(#36)
    pub rules: Vec<JsonRuleStatus>,
    /// 重複候補の列挙に使った方式と件数(#36)
    pub candidate_search: JsonCandidateSearch,
}

/// 対象ファイルの件数(JSON 出力の `context.targets`、#36)。
#[derive(Serialize)]
pub struct JsonTargets {
    /// 走査で発見した対象ファイル数
    pub discovered: usize,
    /// 実際に解析したファイル数
    pub analyzed: usize,
    /// 読み込み失敗等で解析をスキップしたファイル数
    pub skipped: usize,
    /// 走査が失敗なく完了したか。false のとき discovered は下限で
    /// 未発見ファイル数は不明(0 で埋めない)
    pub discovery_complete: bool,
}

/// 致命的でない実行上の失敗 1 件(JSON 出力の `context.operational_errors`、#36)。
#[derive(Serialize)]
pub struct JsonOperationalError {
    /// 失敗したファイル・ディレクトリ(特定できない場合は出力しない)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// 失敗した段階("scan" / "read" / "comment_extract" / "rule:<id>" 等)
    pub stage: String,
    /// 失敗の理由
    pub reason: String,
}

/// ルール 1 件の実行状態(JSON 出力の `context.rules`、#36)。
#[derive(Serialize)]
pub struct JsonRuleStatus {
    /// ルール ID
    pub rule_id: &'static str,
    /// 実行状態: "executed" / "disabled"(allow 抑制) /
    /// "insufficient_inputs"(判定に必要な入力がない) / "failed"(一部入力で失敗)
    pub status: &'static str,
    /// 状態の理由(disabled / insufficient_inputs / failed のとき)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// ルールが適用対象として見た単位の総数(リーフ・文・ペア等、ルール固有)。
    /// 実行していない(disabled / failed / 入力なし)場合は出力しない
    #[serde(skip_serializing_if = "Option::is_none")]
    pub targets: Option<usize>,
    /// そのうち実際に計測・判定できた単位数。targets > measured なら一部のみ判定
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measured: Option<usize>,
    /// `[lint.rule_excludes]` のパス照合で抑制された診断数(#82)。
    /// ルールは実行済み(executed)だが、診断の一部が設定で抑えられたことを
    /// 文脈から読み取れるようにする。抑制がなければ出力しない
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suppressed_by_path: Option<usize>,
}

/// 重複候補列挙の実行記録(JSON 出力の `context.candidate_search`、#36)。
#[derive(Serialize)]
pub struct JsonCandidateSearch {
    /// 実際に使った方式: "all_pairs" / "simhash_banding"
    pub method: &'static str,
    /// 全ペアを採点したか(近似なら false)
    pub all_pairs: bool,
    /// 比較対象となった文書数(本文が最小文字数を満たすもの)
    pub comparable_docs: usize,
    /// 列挙された候補ペア数
    pub candidates: usize,
    /// bigram Jaccard を採点したペア数
    pub scored_pairs: usize,
    /// 採点に使った類似度の下限(報告・統合閾値の低い方)
    pub cutoff: f64,
}

/// 入力ソース 1 件の識別情報(JSON 出力の `context.sources[]`)。
#[derive(Serialize)]
pub struct JsonSource {
    /// 表示パス
    pub path: String,
    /// 内容の SHA-256(hex)
    pub sha256: String,
}

/// 前処理・算法の識別情報(JSON 出力の `context.algorithms`、§7.3)。
#[derive(Serialize)]
pub struct JsonAlgorithms {
    /// 形態素解析器と分割モード・辞書実体(例:
    /// "lindera-sudachidict(mode=normal, dict=embedded:sudachidict)")
    pub morphology: String,
    /// トークン計測エンコーディング(例: "tiktoken o200k_base")
    pub tokenizer: String,
    /// 類似度計算の算法識別子
    pub similarity: &'static str,
    /// バイナリと依存群のビルド識別(jaio 版 + 依存 manifest の hash、#36)。
    /// 埋め込み辞書データ・算法・トークナイザーの版追跡はこの
    /// 共通 manifest への参照で担う
    pub build: String,
}

/// 診断の JSON 表現。Diagnostic のフィールドに category を加える(§7.3)
#[derive(Serialize)]
pub struct JsonDiagnostic<'a> {
    #[serde(flatten)]
    diagnostic: &'a Diagnostic,
    /// ルール ID のカテゴリ(rule_id の `/` の前)
    category: &'static str,
}

impl<'a> JsonReport<'a> {
    /// サマリ・診断列・実行コンテキストからレポートを組み立てる
    pub fn new(
        summary: &'a JsonSummary,
        diagnostics: &'a [Diagnostic],
        context: &'a JsonContext,
        version: &'a str,
    ) -> Self {
        for d in diagnostics {
            for m in &d.measurements {
                m.validate();
            }
        }
        Self {
            schema_version: SCHEMA_VERSION,
            tool: ToolInfo {
                name: "jaio",
                version,
            },
            context,
            summary,
            diagnostics: diagnostics
                .iter()
                .map(|d| JsonDiagnostic {
                    diagnostic: d,
                    category: d.category(),
                })
                .collect(),
        }
    }
}

/// サマリの JSON 表現。
#[derive(Serialize)]
pub struct JsonSummary {
    /// 解析したドキュメント数
    pub files: usize,
    /// 対象全体のトークン総数
    pub tokens: usize,
    /// error 重大度の診断数
    pub errors: usize,
    /// warn 重大度の診断数
    pub warnings: usize,
    /// info 重大度の診断数
    pub infos: usize,
    /// ルール別の診断数の上位(件数降順、同数は rule_id 辞書順)。診断 0 件なら空
    pub top_rules: Vec<(String, usize)>,
    /// フェーズ別の処理時間。`--verbose` 時のみ出力(Issue #18)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timings: Option<PhaseTimings>,
    /// 所要時間降順の上位ルール(rule_id, ミリ秒, 診断数)。`--verbose` 時のみ出力(Issue #18)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slow_rules: Option<Vec<(String, u64, usize)>>,
    /// 計画した対象・ルールの処理が失敗なく完了したか(#36)。
    /// false なら operational_errors / insufficient_inputs の内容を見ること
    pub analysis_complete: bool,
    /// 設定による緩和が見込める大量検出への案内(#91)。orphan が解析成功
    /// 文書数の半分を超える等、設定不足の可能性が高いときに 1 行案内を入れる。
    /// 該当なしのときは空。診断ではないため重大度を持たせない
    pub hints: Vec<String>,
}

/// フェーズ別の処理時間(ミリ秒)。`--verbose` 計測用(Issue #18)。
/// 再現性の比較対象外(実行環境依存、#36)。
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct PhaseTimings {
    /// ファイル収集(collect)
    pub collect_ms: u64,
    /// ドキュメントパースと DocumentRule 実行(ファイルループ全体)
    pub parse_ms: u64,
    /// Corpus 構築と CorpusRule 実行
    pub corpus_ms: u64,
}

impl JsonSummary {
    /// 既定の終了コード(#36、§7.2)。運用エラー・解析不完全は 2、
    /// 完全な解析で規約上の error 診断ありは 1、その他は 0(2 を優先)
    pub fn exit_code(&self) -> u8 {
        if !self.analysis_complete {
            2
        } else if self.errors > 0 {
            1
        } else {
            0
        }
    }
}

/// 診断列をルール別に集計し、件数の多い順(同数は rule_id の辞書順)に
/// 上位 `MAX_TOP_RULES` 件を返す。サマリ行と JSON の `top_rules` の両方で使う。
pub fn top_rules(diags: &[Diagnostic]) -> Vec<(String, usize)> {
    let mut counts: HashMap<&'static str, usize> = HashMap::new();
    for d in diags {
        *counts.entry(d.rule_id).or_default() += 1;
    }
    let mut entries: Vec<(String, usize)> = counts
        .into_iter()
        .map(|(id, count)| (id.to_string(), count))
        .collect();
    // 件数降順・同数は rule_id 辞書順で並べ、上位への切り詰めでも結果が決定的になるようにする
    entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    entries.truncate(MAX_TOP_RULES);
    entries
}

/// ルール別の累計計測(rule_id, ミリ秒, 診断数)を所要時間降順(同時間は rule_id 辞書順)に
/// 上位 `MAX_SLOW_RULES` 件まで返す。`--verbose` の slow_rules とサマリ行で使う(Issue #18)。
pub fn top_slow_rules(entries: &[(String, u64, usize)]) -> Vec<(String, u64, usize)> {
    let mut sorted = entries.to_vec();
    sorted.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    sorted.truncate(MAX_SLOW_RULES);
    sorted
}

/// 診断列を JSON 形式で出力する(LLM 委譲ワークフロー向け、§7.3)。
pub fn render_json(out: &mut impl Write, report: &JsonReport<'_>) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(report)
        .map_err(|e| std::io::Error::other(format!("JSON への直列化に失敗しました: {e}")))?;
    writeln!(out, "{json}")
}

/// SARIF の artifactLocation.uri 用にパスをパーセントエンコードする(#44)。
/// SARIF 仕様は URI を要求するため、空白・非 ASCII・`../` 等を安全に表現する
fn percent_encode_path(path: &str) -> String {
    /// URI で予約されていない文字 + 区切りに使う `/`
    fn is_unreserved(c: u8) -> bool {
        c.is_ascii_alphanumeric() || matches!(c, b'-' | b'.' | b'_' | b'~' | b'/')
    }
    let mut out = String::with_capacity(path.len());
    for byte in path.as_bytes() {
        if is_unreserved(*byte) {
            out.push(*byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// SARIF 2.1.0 の level。jaio の Severity に対応する。
fn sarif_level(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warn => "warning",
        Severity::Info => "note",
    }
}

/// 診断列を SARIF 2.1.0 で出力する(§7.2)。他ツールの CI 連携向け。
/// `rules` には検出に使われたルール ID を一意に列挙する。
/// message には観察(observation)をそのまま載せる。
pub fn render_sarif(
    out: &mut impl Write,
    diags: &[Diagnostic],
    version: &str,
) -> std::io::Result<()> {
    let mut rule_ids: Vec<&str> = diags.iter().map(|d| d.rule_id).collect();
    rule_ids.sort_unstable();
    rule_ids.dedup();
    let rules: Vec<serde_json::Value> = rule_ids
        .iter()
        .map(|id| serde_json::json!({ "id": id }))
        .collect();
    let results: Vec<serde_json::Value> = diags
        .iter()
        .map(|d| {
            serde_json::json!({
                "ruleId": d.rule_id,
                "level": sarif_level(d.severity),
                "message": { "text": d.observation },
                "locations": [{
                    "physicalLocation": {
                        "artifactLocation": { "uri": percent_encode_path(&d.file) },
                        "region": {
                            "startLine": d.span.line,
                            "startColumn": d.span.col,
                        },
                    },
                }],
            })
        })
        .collect();
    let sarif = serde_json::json!({
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            "tool": {
                "driver": {
                    "name": "jaio",
                    "version": version,
                    "informationUri": "https://github.com/oboro-app/oboro-ja",
                    "rules": rules,
                },
            },
            "results": results,
        }],
    });
    let json = serde_json::to_string_pretty(&sarif)
        .map_err(|e| std::io::Error::other(format!("SARIF への直列化に失敗しました: {e}")))?;
    writeln!(out, "{json}")
}

fn severity_color(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "31",
        Severity::Warn => "33",
        Severity::Info => "36",
    }
}

fn paint(color: bool, code: &str, text: &str) -> String {
    if color {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jaio_core::{Location, Span};

    fn sample_diagnostic() -> Diagnostic {
        Diagnostic::new(
            "docs/a.md".to_string(),
            Span { line: 3, col: 5 },
            "efficiency/redundant-expression",
            "冗長表現: 「することができます」→「できます」".to_string(),
        )
        .severity(Severity::Warn)
        .review_question("「できます」に短縮しても意味・敬意が変わらないか確認してください")
        .measurement(
            Measurement::measured("token_count", 8.0)
                .unit("tokens")
                .scope("occurrence"),
        )
        .measurement(
            Measurement::measured("token_count", 3.0)
                .unit("tokens")
                .scope("replacement"),
        )
        .measurement(Measurement::measured("token_delta", -5.0).unit("tokens"))
        .location(Location::occurrence(
            "docs/a.md".to_string(),
            Span { line: 3, col: 5 },
            "することができます".to_string(),
        ))
        .limitation("トークン差分は置き換え候補を適用した場合の局所的な概算です")
    }

    fn sample_context() -> JsonContext {
        JsonContext {
            roots: vec!["docs/".to_string()],
            config_fingerprint: "sha256:abc".to_string(),
            sources: vec![JsonSource {
                path: "docs/a.md".to_string(),
                sha256: "def".to_string(),
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

    #[test]
    fn renders_diagnostic_lines() {
        let mut buf = Vec::new();
        render(&mut buf, &[sample_diagnostic()], false).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("docs/a.md:3:5  warning[efficiency/redundant-expression]"));
        assert!(out.contains("question: 「できます」に短縮しても"));
        assert!(out.contains("measurement: token_delta=-5 tokens"), "{out}");
        assert!(out.contains("limitation: トークン差分は"));
    }

    #[test]
    fn renders_unmeasured_status_without_value() {
        let diagnostic = sample_diagnostic().measurement(Measurement::unmeasured(
            "token_delta",
            MeasurementStatus::InsufficientSample,
            "比較対象がない",
        ));
        let mut buf = Vec::new();
        render(&mut buf, &[diagnostic], false).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(
            out.contains("measurement: token_delta (insufficient_sample): 比較対象がない"),
            "{out}"
        );
    }

    #[test]
    fn renders_occurrence_locations() {
        let mut diagnostic = Diagnostic::new(
            "docs/a.md".to_string(),
            Span { line: 3, col: 1 },
            "consistency/style-deviation",
            "文体の逸脱".to_string(),
        )
        .severity(Severity::Warn);
        diagnostic.locations = vec![
            Location::occurrence(
                "docs/a.md".to_string(),
                Span { line: 3, col: 1 },
                "例示の断片".to_string(),
            ),
            Location::occurrence(
                "docs/a.md".to_string(),
                Span { line: 10, col: 1 },
                "二つ目".to_string(),
            ),
            Location::occurrence(
                "docs/a.md".to_string(),
                Span { line: 26, col: 1 },
                "三つ目".to_string(),
            ),
        ];
        let mut buf = Vec::new();
        render(&mut buf, &[diagnostic], false).unwrap();
        let out = String::from_utf8(buf).unwrap();
        // 出現ごとにスニペットが「」付きで併記される(Issue #19)
        assert!(out.contains("occurrences: L3「例示の断片」, L10「二つ目」, L26「三つ目」"));
    }

    #[test]
    fn occurrences_rendering_dedups_lines_and_caps_display() {
        // 同一行の重複は 1 つにまとめ、昇順に並べる。表示は 8 行まで
        let mut diagnostic = Diagnostic::new(
            "docs/a.md".to_string(),
            Span { line: 3, col: 1 },
            "consistency/style-deviation",
            "文体の逸脱".to_string(),
        )
        .severity(Severity::Info);
        let mut items = vec![
            Location::occurrence(
                "docs/a.md".to_string(),
                Span { line: 12, col: 1 },
                "行12の断片".to_string(),
            ),
            Location::occurrence(
                "docs/a.md".to_string(),
                Span { line: 3, col: 1 },
                "行3の断片".to_string(),
            ),
            Location::occurrence(
                "docs/a.md".to_string(),
                Span { line: 3, col: 1 },
                "行3の断片".to_string(),
            ),
        ];
        for line in 20..30 {
            items.push(Location::occurrence(
                "docs/a.md".to_string(),
                Span { line, col: 1 },
                format!("行{line}の断片"),
            ));
        }
        diagnostic.locations = items;
        let mut buf = Vec::new();
        render(&mut buf, &[diagnostic], false).unwrap();
        let out = String::from_utf8(buf).unwrap();
        // 重複除去済み(3 は 1 回だけ)+ 昇順 + 8 行まで表示 + 残り行数の注記。
        // 各出現にはスニペットが「」付きで併記される(Issue #19)
        assert!(
            out.contains(
                "occurrences: L3「行3の断片」, L12「行12の断片」, L20「行20の断片」, L21「行21の断片」, L22「行22の断片」, L23「行23の断片」, L24「行24の断片」, L25「行25の断片」 ほか4行"
            ),
            "{out}"
        );
        assert!(!out.contains("L26「"));
    }

    #[test]
    fn renders_counterpart_location() {
        let diagnostic = Diagnostic::new(
            "docs/quickstart.md".to_string(),
            Span { line: 5, col: 1 },
            "relation/cross-doc-duplication",
            "docs/setup.md と本文が 78% 類似しています".to_string(),
        )
        .severity(Severity::Warn)
        .measurement(
            Measurement::measured("similarity", 0.78)
                .method("simhash64+bigram-jaccard")
                .unit("ratio")
                .threshold(0.65, ">=")
                .scope("document_pair"),
        )
        .location(Location::counterpart(
            "docs/setup.md".to_string(),
            Span { line: 5, col: 1 },
        ));
        let mut buf = Vec::new();
        render(&mut buf, &[diagnostic], false).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("counterpart: docs/setup.md:5"), "{out}");
        assert!(
            out.contains(
                "measurement: similarity=0.78 ratio (>=0.65, simhash64+bigram-jaccard, document_pair)"
            ),
            "{out}"
        );
    }

    #[test]
    fn renders_markdown_report() {
        let summary = JsonSummary {
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
        };
        let mut buf = Vec::new();
        render_markdown(&mut buf, &[sample_diagnostic()], &summary).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("# jaio report"));
        assert!(
            out.contains("- `docs/a.md:3:5` **warning** `[efficiency/redundant-expression]`"),
            "{out}"
        );
        assert!(out.contains("  - question: "));
        assert!(out.contains("checked 1 documents (42 tokens) / 0 errors, 1 warnings, 0 infos"));
    }

    #[test]
    fn renders_sarif_report() {
        let mut buf = Vec::new();
        render_sarif(&mut buf, &[sample_diagnostic()], "0.1.0").unwrap();
        let out = String::from_utf8(buf).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["version"], "2.1.0");
        assert_eq!(parsed["runs"][0]["tool"]["driver"]["name"], "jaio");
        let result = &parsed["runs"][0]["results"][0];
        assert_eq!(result["ruleId"], "efficiency/redundant-expression");
        assert_eq!(result["level"], "warning");
        let loc = &result["locations"][0]["physicalLocation"];
        assert_eq!(loc["artifactLocation"]["uri"], "docs/a.md");
        assert_eq!(loc["region"]["startLine"], 3);
        // info 診断は SARIF の note に対応する
        let info_diag = sample_diagnostic().severity(Severity::Info);
        let mut buf = Vec::new();
        render_sarif(&mut buf, &[info_diag], "0.1.0").unwrap();
        let parsed: serde_json::Value =
            serde_json::from_str(&String::from_utf8(buf).unwrap()).unwrap();
        assert_eq!(parsed["runs"][0]["results"][0]["level"], "note");
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

    #[test]
    fn renders_json_report() {
        let summary = sample_summary();
        let context = sample_context();
        let diagnostics = [sample_diagnostic()];
        let report = JsonReport::new(&summary, &diagnostics, &context, "0.1.0");
        let mut buf = Vec::new();
        render_json(&mut buf, &report).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("\"schema_version\": 2"));
        assert!(out.contains("\"name\": \"jaio\""));
        assert!(out.contains("\"version\": \"0.1.0\""));
        assert!(out.contains("\"config_fingerprint\": \"sha256:abc\""));
        assert!(out.contains("\"rule_id\": \"efficiency/redundant-expression\""));
        assert!(out.contains("\"category\": \"efficiency\""));
        assert!(out.contains("\"severity\": \"warn\""));
        assert!(out.contains("\"observation\""));
        assert!(out.contains("\"measurements\""));
        assert!(out.contains("\"review_questions\""));
        assert!(out.contains("\"limitations\""));
        // v1 のフィールドは出さない
        assert!(!out.contains("\"fixable\""));
        assert!(!out.contains("\"message\""));
        assert!(!out.contains("\"help\""));
        assert!(!out.contains("\"evidence\""));
        assert!(!out.contains("\"estimated_reduction\""));
        assert!(!out.contains("\"fixes\""));
    }

    #[test]
    fn json_report_keys_match_schema() {
        // docs/schema/report.schema.json(仕様の正本)と出力のキー集合が
        // ずれないことを確認する軽い同期テスト(Issue #7)。required の全キーが
        // 出力に存在し、出力のキーが properties に収まることだけを照合する
        let schema: serde_json::Value =
            serde_json::from_str(include_str!("../../../docs/schema/report.schema.json")).unwrap();
        let summary = sample_summary();
        let context = sample_context();
        let diagnostics = [sample_diagnostic()];
        let output =
            serde_json::to_value(JsonReport::new(&summary, &diagnostics, &context, "0.1.0"))
                .expect("JSON への直列化に失敗しました");

        let check_keys =
            |schema_obj: &serde_json::Value, output_obj: &serde_json::Value, what: &str| {
                let properties = schema_obj["properties"].as_object().unwrap();
                let object = output_obj.as_object().unwrap();
                for required in schema_obj["required"].as_array().unwrap() {
                    let key = required.as_str().unwrap();
                    assert!(
                        object.contains_key(key),
                        "{what}: required の {key} が出力にない"
                    );
                }
                for key in object.keys() {
                    assert!(
                        properties.contains_key(key),
                        "{what}: 出力の {key} が schema の properties にない"
                    );
                }
            };
        check_keys(&schema, &output, "top-level");
        check_keys(
            &schema["properties"]["summary"],
            &output["summary"],
            "summary",
        );
        check_keys(
            &schema["properties"]["context"],
            &output["context"],
            "context",
        );
        let diagnostic_schema = &schema["$defs"]["diagnostic"];
        check_keys(
            diagnostic_schema,
            &output["diagnostics"][0],
            "diagnostics[0]",
        );
        // category はスキーマの enum(§5 のカテゴリ一覧)に含まれる
        let categories = diagnostic_schema["properties"]["category"]["enum"]
            .as_array()
            .unwrap();
        assert!(
            categories.contains(&output["diagnostics"][0]["category"]),
            "category が schema の enum にない"
        );
        // measurement / location のキーもスキーマと一致する
        check_keys(
            &schema["$defs"]["measurement"],
            &output["diagnostics"][0]["measurements"][0],
            "measurement",
        );
        check_keys(
            &schema["$defs"]["location"],
            &output["diagnostics"][0]["locations"][0],
            "location",
        );
    }

    #[test]
    fn top_rules_aggregation_is_deterministic() {
        let diag = |rule_id: &'static str| {
            Diagnostic::new(
                "docs/a.md".to_string(),
                Span { line: 1, col: 1 },
                rule_id,
                "m".to_string(),
            )
            .severity(Severity::Info)
        };
        // 診断 0 件なら空
        assert!(top_rules(&[]).is_empty());
        // 7 ルール(3,3,2,1,1,1,1 件):同数は rule_id 辞書順、上位 5 件に切り詰め
        let diags: Vec<Diagnostic> = [
            "relation/orphan",
            "relation/orphan",
            "relation/orphan",
            "consistency/particle",
            "consistency/particle",
            "consistency/particle",
            "efficiency/redundant-expression",
            "efficiency/redundant-expression",
            "consistency/style-deviation",
            "relation/a",
            "relation/b",
            "relation/c",
        ]
        .into_iter()
        .map(diag)
        .collect();
        let top = top_rules(&diags);
        assert_eq!(
            top,
            vec![
                ("consistency/particle".to_string(), 3),
                ("relation/orphan".to_string(), 3),
                ("efficiency/redundant-expression".to_string(), 2),
                ("consistency/style-deviation".to_string(), 1),
                ("relation/a".to_string(), 1),
            ]
        );
        // JSON の summary.top_rules は [rule_id, 件数] の配列として直列化される
        let summary = JsonSummary {
            files: 1,
            tokens: 10,
            errors: 0,
            warnings: 6,
            infos: 6,
            top_rules: top.clone(),
            timings: None,
            slow_rules: None,
            analysis_complete: true,
            hints: Vec::new(),
        };
        let json = serde_json::to_value(&summary).unwrap();
        assert_eq!(
            json["top_rules"],
            serde_json::json!([
                ["consistency/particle", 3],
                ["relation/orphan", 3],
                ["efficiency/redundant-expression", 2],
                ["consistency/style-deviation", 1],
                ["relation/a", 1],
            ])
        );
    }

    #[test]
    fn verbose_timings_serialize_only_when_present() {
        // --verbose 未指定(None)ならキー自体を出さず、指定時は slow_rules が
        // [rule_id, ms, diags] の配列として直列化される(Issue #18)
        let plain = JsonSummary {
            files: 1,
            tokens: 10,
            errors: 0,
            warnings: 0,
            infos: 0,
            top_rules: Vec::new(),
            timings: None,
            slow_rules: None,
            analysis_complete: true,
            hints: Vec::new(),
        };
        let json = serde_json::to_value(&plain).unwrap();
        assert!(json.get("timings").is_none());
        assert!(json.get("slow_rules").is_none());

        let verbose = JsonSummary {
            timings: Some(PhaseTimings {
                collect_ms: 12,
                parse_ms: 6210,
                corpus_ms: 640,
            }),
            slow_rules: Some(vec![("relation/orphan".to_string(), 90, 101)]),
            ..plain
        };
        let json = serde_json::to_value(&verbose).unwrap();
        assert_eq!(
            json["timings"],
            serde_json::json!({ "collect_ms": 12, "parse_ms": 6210, "corpus_ms": 640 })
        );
        assert_eq!(
            json["slow_rules"],
            serde_json::json!([["relation/orphan", 90, 101]])
        );
    }
}
