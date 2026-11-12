//! jaio-rules: ルール trait(設計書 §6.1)とルール実装群。

mod consistency;
mod corpus;
mod efficiency;
mod structure;

pub use corpus::{Corpus, PairSearch, comparison_key, normalize as normalize_path};
pub use structure::{SectionSlice, extract_sections};

use anyhow::Result;
use comrak::nodes::NodeValue;
use jaio_core::config::Config;
use jaio_core::config::StyleOverrides;
use jaio_core::{Diagnostic, Span};
use jaio_parse::doc::{Doc, Heading, LeafKind, TextLeaf};
use jaio_parse::morph::{Analyzer, Morph};
use jaio_parse::tokencount::TokenCounter;

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

/// ルール実行の戻り値(#36)。診断とは別に「何に適用して何を計測・判定
/// できたか」を返し、「呼ばれたが計測不能だった」と「正常に 0 件」を区別する。
pub struct RuleRun {
    /// 検出した診断列
    pub diagnostics: Vec<Diagnostic>,
    /// 適用範囲と計測量
    pub coverage: Coverage,
}

/// ルールが見た適用対象と実際に計測・判定できた量(#36)。
/// `targets` == 0 は「適用対象がなかった」、`measured` == 0(かつ
/// `targets` > 0)は「入力はあったが判定不能だった」を意味する。
#[derive(Debug, Clone, Default)]
pub struct Coverage {
    /// 適用対象として見た単位数(リーフ・見出し・文・ペア等、ルール固有)
    pub targets: usize,
    /// そのうち実際に計測・判定できた数
    pub measured: usize,
    /// 対象外・標本不足の補足理由(あれば)
    pub reason: Option<String>,
}

impl RuleRun {
    /// `targets` 件すべてを計測・判定できた場合の構築
    pub fn complete(diagnostics: Vec<Diagnostic>, targets: usize) -> Self {
        Self {
            diagnostics,
            coverage: Coverage {
                targets,
                measured: targets,
                reason: None,
            },
        }
    }

    /// 適用対象がなかった場合の構築(診断なし)
    pub fn not_applicable(reason: impl Into<String>) -> Self {
        Self {
            diagnostics: Vec::new(),
            coverage: Coverage {
                targets: 0,
                measured: 0,
                reason: Some(reason.into()),
            },
        }
    }
}

/// 単一ドキュメント内で完結するルール(設計書 §6.1)。
/// DocContext が jaio-parse の型を参照するため、この crate に trait を置く。
pub trait DocumentRule: Send + Sync {
    /// ルール ID(例: "efficiency/redundant-expression")
    fn id(&self) -> &'static str;
    /// ルールの説明
    fn description(&self) -> &'static str;
    /// ルールを適用する
    fn run(&self, ctx: &mut DocContext<'_>) -> Result<RuleRun>;
}

/// ルール実行コンテキスト。1 ドキュメント分の解析部品を束ねる。
pub struct DocContext<'a> {
    /// 解析済みドキュメント
    pub doc: &'a Doc,
    /// テキストリーフ列
    pub leaves: &'a [TextLeaf],
    /// 見出し構造
    pub headings: &'a [Heading],
    /// 形態素解析器
    pub analyzer: &'a mut Analyzer,
    /// トークンカウンタ
    pub counter: &'a TokenCounter,
    /// 検出感度の設定(jaio.toml の `[detection]`)
    pub detection: &'a jaio_core::config::DetectionConfig,
    /// 形態素解析のキャッシュ(テキスト → 解析結果)。ルール間で再解析を避ける
    pub cache: &'a mut HashMap<String, Arc<[Morph]>>,
}

impl DocContext<'_> {
    /// 本文(段落・リスト項目)かどうか。冗長表現などテキスト内容のルールで使う。
    /// フィールドの分割借用を活かすためヘルパーはイテレータを返さない。
    pub fn is_body_leaf(leaf: &TextLeaf) -> bool {
        matches!(leaf.kind, LeafKind::Paragraph | LeafKind::ListItem)
    }

    /// 本文(段落+リスト項目)に占める段落テキストの割合(0.0 - 1.0)。
    /// 散文前提ルール(token-density / repeated-quote)の適用判断に使う。
    /// リスト主導の文書(課題管理ファイル等)は値が低くなる
    pub fn prose_share(&self) -> f64 {
        let mut prose = 0usize;
        let mut total = 0usize;
        for leaf in self.leaves.iter().filter(|l| Self::is_body_leaf(l)) {
            let len = leaf.text.chars().count();
            total += len;
            if leaf.kind == LeafKind::Paragraph {
                prose += len;
            }
        }
        if total == 0 {
            return 1.0;
        }
        prose as f64 / total as f64
    }

    /// 散文前提ルールの適用対象か(段落割合が設定の下限以上)
    pub fn is_prose_document(&self) -> bool {
        self.prose_share() >= self.detection.prose_min_share
    }

    /// 形態素解析(キャッシュ付き、設計書 §10)。同一テキストの再解析を避ける。
    /// 戻り値は `Arc` のため呼び出し側の借用を縛らない。
    pub fn analyze(&mut self, text: &str) -> Result<Arc<[Morph]>> {
        if let Some(cached) = self.cache.get(text) {
            return Ok(Arc::clone(cached));
        }
        let morphs: Arc<[Morph]> = self.analyzer.analyze(text)?.into();
        self.cache.insert(text.to_string(), Arc::clone(&morphs));
        Ok(morphs)
    }

    /// リーフのソース行範囲(line..=end_line)から `needle` の `occurrence` 番目
    /// (0 基点)の実位置を返す。`mask` を指定すると、その文字列の出現は
    /// カウント対象外にする(推奨表記「サーバー」に含まれる別表記「サーバ」を
    /// 拾わないため)。Markdown 記法を挟むなどソース上で連続していない場合は None
    pub fn locate(
        &self,
        leaf: &TextLeaf,
        needle: &str,
        occurrence: usize,
        mask: Option<&str>,
    ) -> Option<Span> {
        self.locate_all(leaf, needle, mask).get(occurrence).copied()
    }

    /// `locate` の全出現版。同一リーフ・同一 needle への反復呼び出しは
    /// こちらを一度呼んで出現番号で引く(出現ごとの再走査を避ける、Issue #30)
    pub fn locate_all(&self, leaf: &TextLeaf, needle: &str, mask: Option<&str>) -> Vec<Span> {
        // リーフの行範囲をバイトスライスで取り出す。lines().skip() で文書
        // 先頭から走査するとリーフ数×行数の二乗計算になるため(Issue #30)
        let start = self
            .doc
            .line_offsets
            .get(leaf.line - 1)
            .copied()
            .unwrap_or(self.doc.source.len());
        let end = self
            .doc
            .line_offsets
            .get(leaf.end_line)
            .copied()
            .unwrap_or(self.doc.source.len());
        let block = &self.doc.source[start..end];
        let mut spans = Vec::new();
        let mut in_def_prefix = true;
        for (idx, raw) in block.lines().enumerate() {
            // leaf.text にはリンクテキストだけが入り URL(`](...)`)や
            // 参照定義は含まれない。生行側にだけ存在する出現で出現番号が
            // ずれないよう、それらの範囲はカウントから外す。
            // 参照定義行はリーフ行範囲の先頭からの連続ランにだけ現れる
            // (段落中の同名行は leaf.text に残る本文扱い)
            let is_def = in_def_prefix && is_reference_definition(raw);
            if !is_def {
                in_def_prefix = false;
            }
            let mut excluded = non_prose_ranges(raw);
            if is_def {
                excluded.push(0..raw.len());
            }
            if let Some(m) = mask {
                excluded.extend(raw.match_indices(m).map(|(b, _)| b..b + m.len()));
            }
            for (byte, _) in raw.match_indices(needle) {
                if excluded.iter().any(|r| r.contains(&byte)) {
                    continue;
                }
                // CRLF の \r は行末として列に数えない(#44)
                let col = raw[..byte].chars().filter(|&c| c != '\r').count() + 1;
                spans.push(Span {
                    line: leaf.line + idx,
                    col,
                });
            }
        }
        spans
    }
}

/// 行が参照リンク定義(`[label]: dest`)の形か。comrak は段落ブロック
/// 先頭の連続する定義行を leaf.text から取り除くため、呼び出し側は
/// リーフ行範囲の先頭からの連続ランにだけ適用する(段落中の同名行は
/// leaf.text に残る本文扱い)
pub(crate) fn is_reference_definition(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with('[') && t.contains("]:")
}

/// 生行のうち leaf.text に含まれない記法部分のバイト範囲を返す。
/// ソース位置の探索(locate)でこれらの内部を対象外にするために使う(§7.4)。
/// 対象: `[text](url)`・`![alt](url)` の URL 部。autolink は comrak が
/// リンクテキストとして URL を leaf.text に含めるため対象外。
/// 参照リンク定義行は呼び出し側が `is_reference_definition` で行単位に扱う
pub(crate) fn non_prose_ranges(line: &str) -> Vec<Range<usize>> {
    let bytes = line.as_bytes();
    let mut ranges = Vec::new();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b']' && bytes[i + 1] == b'(' {
            // インラインリンクの URL 部
            let url_start = i + 2;
            let mut j = url_start;
            while j < bytes.len() && bytes[j] != b')' {
                j += 1;
            }
            ranges.push(url_start..j);
            i = j;
        } else {
            i += 1;
        }
    }
    ranges
}

/// `haystack` 中の `needle` の出現数を数える。ただし `preferred` の出現に
/// 内包されるものは数えない(「サーバー」内の「サーバ」を別表記と誤数しない)
pub(crate) fn count_excluding_preferred(haystack: &str, preferred: &str, needle: &str) -> usize {
    let preferred_ranges: Vec<Range<usize>> = haystack
        .match_indices(preferred)
        .map(|(b, _)| b..b + preferred.len())
        .collect();
    haystack
        .match_indices(needle)
        .filter(|(b, _)| !preferred_ranges.iter().any(|r| r.contains(b)))
        .count()
}

/// verify-move / move-integrity の行正規化で保護する構文範囲(§7.2.1、#66)。
/// comrak の解析結果からコードブロック行・見出しマーカー範囲・実際のリンク範囲を
/// 集め、Markdown 構文ではない場所(コード・文字列)へ正規化を適用しない
struct MoveLineGuards {
    /// コードブロック(フェンス・インデント)に含まれる行(1 基点)。
    /// この行はインデントを含めて内容をそのまま保持する
    code_block_lines: HashSet<usize>,
    /// 行(1 基点)ごとの ATX 見出しマーカー除去範囲(行内のバイト範囲)。
    /// sourcepos の開始列は `#` の位置を指すため、引用内・字下げの
    /// 見出しでも正確にマーカーだけ除去できる
    heading_markers: HashMap<usize, Range<usize>>,
    /// setext 見出しの下線行(`===` / `---`、1 基点)。内容を持たない
    /// マーカー行なので実質行から除外する
    marker_only_lines: HashSet<usize>,
    /// 行(1 基点)ごとの実リンク範囲(行内のバイト範囲)。
    /// `[text](url)` → `text` の簡約はこの範囲にのみ適用する
    link_ranges: HashMap<usize, Vec<Range<usize>>>,
}

/// comrak の解析結果から保護範囲を集める
fn move_line_guards(source: &str) -> MoveLineGuards {
    let arena = comrak::Arena::new();
    let root = comrak::parse_document(&arena, source, &jaio_parse::doc::markdown_options());
    let lines: Vec<&str> = source.lines().collect();
    let mut code_block_lines = HashSet::new();
    let mut heading_markers = HashMap::new();
    let mut marker_only_lines = HashSet::new();
    let mut link_ranges: HashMap<usize, Vec<Range<usize>>> = HashMap::new();
    for node in root.descendants() {
        let data = node.data.borrow();
        let span = data.sourcepos;
        match &data.value {
            NodeValue::CodeBlock(_) => {
                code_block_lines.extend(span.start.line..=span.end.line);
            }
            NodeValue::Heading(_) => {
                // setext 見出しは複数行にまたがり、最終行は下線だけの
                // マーカー行(中間行は本文)。ATX は常に 1 行なので無関係
                if span.end.line > span.start.line {
                    marker_only_lines.insert(span.end.line);
                }
                // ATX 見出しの開始列は `#` の位置を指す。連続する `#` と
                // 後続空白までを除去範囲とする。`#` が無ければ setext の
                // 本文行なので何も記録しない(列が想定とずれる場合も
                // 無条件で剥がさない安全側の確認)
                let start = span.start.column - 1;
                let Some(bytes) = lines.get(span.start.line - 1).map(|l| l.as_bytes()) else {
                    continue;
                };
                if bytes.get(start) != Some(&b'#') {
                    continue;
                }
                let mut end = start;
                while bytes.get(end) == Some(&b'#') {
                    end += 1;
                }
                while bytes.get(end).is_some_and(|b| matches!(b, b' ' | b'\t')) {
                    end += 1;
                }
                heading_markers.insert(span.start.line, start..end);
            }
            // sourcepos の列はバイト基準・終端は末尾バイトの次の列(0 基点の
            // 排他端に一致)。複数行にまたがるリンクは行単位の置き換え対象に
            // しない(安全側に倒す)
            NodeValue::Link(_) if span.start.line == span.end.line => {
                link_ranges
                    .entry(span.start.line)
                    .or_default()
                    .push(span.start.column - 1..span.end.column);
            }
            _ => {}
        }
    }
    MoveLineGuards {
        code_block_lines,
        heading_markers,
        marker_only_lines,
        link_ranges,
    }
}

/// 実リンク範囲の `[text](url)` を `text` に簡約する。
/// 範囲外(インラインコード・文字列内のリンク風文字列)には触れない
fn simplify_link_ranges(line: &str, ranges: &[Range<usize>]) -> String {
    let mut text = line.to_string();
    // 後ろから置き換えて前方範囲のオフセットを維持する
    let mut sorted: Vec<&Range<usize>> = ranges.iter().collect();
    sorted.sort_by(|a, b| b.start.cmp(&a.start));
    for range in sorted {
        let Some(seg) = text.get(range.clone()) else {
            continue;
        };
        // comrak がリンクと解釈した範囲なので `[` 始まりで、最初の `](` が
        // 本文と宛先の区切り。autolink(`<url>`)等 `[` を持たないものは触れない
        if !seg.starts_with('[') {
            continue;
        }
        let Some(sep) = seg.find("](") else {
            continue;
        };
        let inner = seg[1..sep].to_string();
        text.replace_range(range.clone(), &inner);
    }
    text
}

/// verify-move / move-integrity の行正規化(§7.2.1、#51/#56/#66)。
/// 実際の見出しのマーカー範囲だけ除去してレベル差を吸収し、実際のリンク記法
/// `[text](url)` を `text` に簡約(リンク先の実在検証は broken-link が担う)、
/// 前後空白を除去する。コードブロック内の行は内容をそのまま保持し、
/// setext の下線行は内容を持たないので空として扱う
fn normalize_move_line(line: &str, lineno: usize, guards: &MoveLineGuards) -> String {
    if guards.code_block_lines.contains(&lineno) {
        return line.to_string();
    }
    if guards.marker_only_lines.contains(&lineno) {
        return String::new();
    }
    let mut text = match guards.link_ranges.get(&lineno) {
        Some(ranges) => simplify_link_ranges(line, ranges),
        None => line.to_string(),
    };
    // 見出しマーカーは行内リンクより先頭側にあるので、簡約後でも範囲は有効
    if let Some(marker) = guards.heading_markers.get(&lineno) {
        text.replace_range(marker.clone(), "");
    }
    text.trim().to_string()
}

/// ソースの正規化済み実質行(1 基点の行番号付き)を返す。
/// 正規化後に空になる行(空白行・見出しマーカーのみの行)は数えない
pub fn substantive_lines(source: &str) -> Vec<(usize, String)> {
    let guards = move_line_guards(source);
    source
        .lines()
        .enumerate()
        .map(|(i, line)| (i + 1, normalize_move_line(line, i + 1, &guards)))
        .filter(|(_, normalized)| !normalized.trim().is_empty())
        .collect()
}

/// 既定のルール一覧。設定(jaio.toml)から文体の既定値・ユーザー拡張パターン・
/// 許容表記を注入する。
pub fn default_rules(config: &Config) -> Vec<Box<dyn DocumentRule>> {
    vec![
        Box::new(structure::HeadingSkipRule),
        Box::new(structure::EmptySectionRule),
        Box::new(structure::OverNestingRule),
        Box::new(structure::SectionDuplicationRule),
        Box::new(efficiency::RedundantExpressionRule::with_extra(
            &config.patterns.redundant,
        )),
        Box::new(efficiency::FillerRule::with_extra(
            &config.patterns.redundant,
        )),
        Box::new(efficiency::TokenDensityRule),
        Box::new(efficiency::RepeatedQuoteRule),
        Box::new(efficiency::LongSentenceRule),
        Box::new(efficiency::NoCommaRule),
        Box::new(consistency::StyleDeviationRule {
            target: config.style.target,
            // 無効パターンは load_config 時点で検証済み。ここでのビルド失敗は
            // 設定ファイルを経由しない経路(テスト等)でのみ起こり得るため
            // 空の上書きにフォールバックする。絶対パス文書の相対化基準には
            // jaio.toml の配置ディレクトリを使う(#76)
            overrides: StyleOverrides::build(&config.style.overrides, config.toml_base())
                .unwrap_or_default(),
        }),
        Box::new(consistency::NotationVariantRule {
            allowed: config.lint.allow.terms.iter().cloned().collect(),
        }),
        Box::new(consistency::ParticleRule),
        Box::new(consistency::TerminologyRule::with_extra(
            &config.terms.pairs,
            &config.lint.allow.terms,
        )),
        Box::new(consistency::PunctuationRule),
    ]
}

/// リポジトリ横断のルール(設計書 §6.1)。Index 相当の入力が揃った後に実行する。
pub trait CorpusRule: Send + Sync {
    /// ルール ID(例: "relation/broken-link")
    fn id(&self) -> &'static str;
    /// ルールの説明
    fn description(&self) -> &'static str;
    /// ルールを適用する
    fn run(&self, corpus: &corpus::Corpus<'_>) -> Result<RuleRun>;
}

/// 既定のリポジトリ横断ルール一覧。設定(jaio.toml)から移動宣言(#56)を注入する。
/// `move_path_bases` は `[verification.moves]` 宣言パスの解決基準候補
/// (解析ルート)。jaio.toml の配置ディレクトリは config から補う(#73)
pub fn default_corpus_rules(
    config: &Config,
    move_path_bases: &[PathBuf],
) -> Vec<Box<dyn CorpusRule>> {
    // 宣言パスの基準候補: ① jaio.toml の配置ディレクトリ(運用上 PJ ルート)
    // ② 解析ルート。③ cwd は comparison_key の非実在フォールバックが担う
    let mut path_bases: Vec<PathBuf> = Vec::new();
    if let Some(base) = config.toml_base() {
        path_bases.push(base.to_path_buf());
    }
    path_bases.extend(move_path_bases.iter().cloned());
    vec![
        Box::new(corpus::BrokenLinkRule),
        Box::new(corpus::OrphanRule {
            // base は構築時に実パス表記へ解決する(#106: relativize_for_match
            // の呼び出しごとの canonicalize を避ける)
            base: config.toml_base().map(jaio_core::resolved_base),
        }),
        Box::new(corpus::MoveIntegrityRule {
            moves: config.verification.moves.clone(),
            path_bases,
        }),
        Box::new(corpus::CrossDocDuplicationRule),
        Box::new(corpus::DirMismatchRule),
        Box::new(corpus::ConsolidationPlanRule),
        Box::new(corpus::DocCommentDriftRule),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_excluding_preferred_is_symmetric_for_reversed_pairs() {
        // ユーザー定義ペア(preferred が other の部分文字列)でも
        // 両方向のカウントが相互排除される(#100)
        let text = "サーバーとサーバ。サーバもサーバーも同じ。";
        // 内蔵ペア相当(推奨が長い)
        assert_eq!(count_excluding_preferred(text, "サーバー", "サーバ"), 2);
        // 逆定義(推奨が短い): other「サーバー」の範囲に preferred「サーバ」が
        // 含まれるため、other を除いた preferred は 0 になる(数えないのが正)。
        // この向きのペアは jaio-core::Config::validate で設定エラーになる(#100)
        assert_eq!(count_excluding_preferred(text, "サーバ", "サーバー"), 0);
    }

    use super::substantive_lines;

    /// 行番号を落として正規化済みテキストだけ取り出す
    fn normalized(source: &str) -> Vec<String> {
        substantive_lines(source)
            .into_iter()
            .map(|(_, t)| t)
            .collect()
    }

    /// コードブロック内の行はインデント・`#`・リンク風文字列を含めて
    /// 内容をそのまま保持する(#66)。構文正規化は Markdown の構文位置にのみ
    /// 適用され、コードには触れない
    #[test]
    fn code_block_lines_are_kept_verbatim() {
        let source = "# 題名\n\n```python\n# send_notification()\n  indented()\n```\n";
        assert_eq!(
            normalized(source),
            vec![
                "題名".to_string(),
                "```python".to_string(),
                "# send_notification()".to_string(),
                "  indented()".to_string(),
                "```".to_string(),
            ]
        );
    }

    /// インデントコード(4 空白)内も保護対象。リンク風文字列は簡約しない
    #[test]
    fn indented_code_block_is_protected() {
        let source = "本文です。\n\n    const x = \"[a](b)\";\n\n続きです。\n";
        assert_eq!(
            normalized(source),
            vec![
                "本文です。".to_string(),
                "    const x = \"[a](b)\";".to_string(),
                "続きです。".to_string(),
            ]
        );
    }

    /// インラインコード内のリンク風文字列は簡約しないが、同じ行の実リンクは
    /// 従来どおり簡約する
    #[test]
    fn inline_code_is_protected_but_real_links_are_simplified() {
        let source = "`x = \"[a](b)\"` と[実リンク](real.md)です。\n";
        assert_eq!(
            normalized(source),
            vec!["`x = \"[a](b)\"` と実リンクです。".to_string()]
        );
    }

    /// `#tag` のような段落先頭の `#` は見出しではないため除去しない。
    /// 実際の見出しのレベル差は従来どおり吸収する
    #[test]
    fn heading_marker_strip_is_limited_to_real_headings() {
        assert_eq!(normalized("## 見出し\n"), vec!["見出し".to_string()]);
        assert_eq!(
            normalized("#tag は段落です。\n"),
            vec!["#tag は段落です。".to_string()]
        );
    }

    /// 引用内見出し(`> # 題`)と字下げ ATX 見出し(3 空白まで有効)も
    /// sourcepos の列からマーカーだけ除去してレベル差を吸収する。
    /// 引用記号 `>` 自体は構造なので残す
    #[test]
    fn quoted_and_indented_heading_markers_are_stripped() {
        assert_eq!(
            normalized("> # 引用内見出し\n"),
            vec!["> 引用内見出し".to_string()]
        );
        assert_eq!(
            normalized("   ## 字下げ見出し\n"),
            vec!["字下げ見出し".to_string()]
        );
    }

    /// setext 見出しの下線行(`===` / `---`)は内容を持たないマーカー行なので
    /// 実質行から除外する。ATX への書き換えを吸収するため
    #[test]
    fn setext_underline_lines_are_dropped() {
        assert_eq!(normalized("見出し\n===\n"), vec!["見出し".to_string()]);
        assert_eq!(normalized("見出し\n---\n"), vec!["見出し".to_string()]);
    }

    /// 複数行 setext 見出しでは中間行も本文として残し、最終行の下線だけ
    /// 除外する。本文行が `#` で始まってもマーカーとは誤認しない
    #[test]
    fn multiline_setext_keeps_text_lines_verbatim() {
        let source = "最初の行\n#tag の行\n===\n";
        assert_eq!(
            normalized(source),
            vec!["最初の行".to_string(), "#tag の行".to_string()]
        );
    }
}
