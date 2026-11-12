//! efficiency カテゴリ: トークン効率(設計書 §5.1)。
//! 置き換え先のある冗長表現と削除候補のフィラーは別ルールとして扱う。

use anyhow::Result;
use jaio_core::{Diagnostic, Location, Measurement, Severity, Span};
use jaio_index::normalize_text;
use jaio_parse::doc::TextLeaf;
use jaio_parse::ja::SentenceSpan;
use jaio_parse::ja::split_sentences_spans;
use std::collections::HashMap;

use crate::{Coverage, DocContext, DocumentRule, RuleRun};

/// 冗長表現パターン(表現, 置き換え先)。空の置き換え先は削除候補を意味する。
/// 長いパターンを先に並べ、短いパターンの重複検出を防ぐ。
const REDUNDANT_EXPRESSIONS: &[(&str, &str)] = &[
    ("使用することができます", "使用できます"),
    ("利用することができます", "利用できます"),
    ("行うことができます", "行えます"),
    ("行うことが可能です", "行えます"),
    ("することができます", "できます"),
    ("することが可能です", "できます"),
    ("することが出来ます", "できます"),
    ("必要となります", "必要です"),
    ("かと思われます", "と思われます"),
    ("かと思います", "と思います"),
    ("させていただきます", "します"),
    ("と言っても過言ではない", ""),
];

/// 定型冗長表現を検出する(設計書 §5.1)。置き換え先が空(削除候補)の
/// パターンはフィラーとして `FillerRule` が担うため、ここでは扱わない。
pub struct RedundantExpressionRule {
    /// 適用するパターン辞書(表現, 置き換え先)。長い順に並んでいること
    patterns: Vec<(String, String)>,
}

/// フィラー句・空虚修飾を検出する(設計書 §5.1)。
/// 置き換え先が空 = 削除候補のパターンを扱う。
pub struct FillerRule {
    /// 適用するフィラーパターン(表現)。長い順に並んでいること
    patterns: Vec<String>,
}

/// ユーザー拡張をマージして長い順にソートしたパターン辞書を作る。
/// 空のパターンは `find` が常に先頭一致を返して進行しなくなるため除外する
pub(crate) fn merged_patterns(
    extra: &std::collections::BTreeMap<String, String>,
) -> Vec<(String, String)> {
    let mut merged: std::collections::BTreeMap<String, String> = REDUNDANT_EXPRESSIONS
        .iter()
        .map(|(p, r)| ((*p).to_string(), (*r).to_string()))
        .collect();
    for (p, r) in extra {
        if p.is_empty() {
            continue;
        }
        merged.insert(p.clone(), r.clone());
    }
    // 長いパターンを先に並べ、短いパターンの重複検出を防ぐ。
    // 同長は辞書順で並べ、決定的な並びにする
    let mut patterns: Vec<(String, String)> = merged.into_iter().collect();
    patterns.sort_by(|a, b| {
        b.0.chars()
            .count()
            .cmp(&a.0.chars().count())
            .then_with(|| a.0.cmp(&b.0))
    });
    patterns
}

impl RedundantExpressionRule {
    /// 内蔵辞書 + ユーザー拡張でルールを作る。ユーザー拡張は内蔵と同じ
    /// 表現の置き換え先を上書きする
    pub fn with_extra(extra: &std::collections::BTreeMap<String, String>) -> Self {
        // 置き換え先があるパターンのみ扱う(空は FillerRule 側)
        let patterns = merged_patterns(extra)
            .into_iter()
            .filter(|(_, r)| !r.is_empty())
            .collect();
        Self { patterns }
    }
}

impl FillerRule {
    /// 内蔵辞書 + ユーザー拡張からフィラー(削除候補)パターンを作る
    pub fn with_extra(extra: &std::collections::BTreeMap<String, String>) -> Self {
        let patterns = merged_patterns(extra)
            .into_iter()
            .filter(|(_, r)| r.is_empty())
            .map(|(p, _)| p)
            .collect();
        Self { patterns }
    }
}

/// 冗長表現・フィラー共通のパターン走査(#96)。
/// body leaf ごとに patterns を適用し、より長いパターンを優先する covered
/// 管理・leaf.text の出現番号とソース位置の対応維持(Issue #30)・
/// ソース上に連続して現れない出現のリーフ先頭近似(#44)を共通で扱う。
/// `entries` は (パターン, 置き換え先) の列で、置き換え先 None は
/// 削除候補(フィラー)。`build` は診断を組み立てるだけにし、
/// approximate の locations 追加はこの関数側で行う
fn scan_pattern_diags(
    ctx: &DocContext<'_>,
    entries: &[(String, Option<String>)],
    mut build: impl FnMut(&str, &str, Option<&str>, Span) -> Diagnostic,
) -> (Vec<Diagnostic>, usize) {
    let mut diags = Vec::new();
    let targets = ctx
        .leaves
        .iter()
        .filter(|l| DocContext::is_body_leaf(l))
        .count();
    for leaf in ctx.leaves.iter().filter(|l| DocContext::is_body_leaf(l)) {
        // 検出済みのバイト区間。より長いパターンを優先するための管理
        let mut covered: Vec<(usize, usize)> = Vec::new();
        for (pattern, replacement) in entries {
            // leaf.text 内の n 番目 = ソース内の n 番目という対応を保つため、
            // skip した分も出現番号に数える(skip 判定より前に数える)。
            // 全出現位置は一度だけ走査する(Issue #30)
            let positions = ctx.locate_all(leaf, pattern, None);
            let mut from = 0usize;
            let mut occurrence = 0usize;
            while let Some(offset) = leaf.text[from..].find(pattern.as_str()) {
                let start = from + offset;
                let end = start + pattern.len();
                from = end;
                occurrence += 1;
                // 既に検出した区間と重なる場合は飛ばす
                if covered.iter().any(|(s, e)| start < *e && *s < end) {
                    continue;
                }
                covered.push((start, end));
                // Markdown 記法を挟む等でソース上に連続して現れない場合の近似
                let span = positions.get(occurrence - 1).copied().unwrap_or(Span {
                    line: leaf.line,
                    col: leaf.text[..start].chars().count() + 1,
                });
                // ソース上に連続して現れない場合はリーフ先頭近似と明示する
                let approximate = positions.get(occurrence - 1).is_none();
                let mut diag = build(&ctx.doc.display, pattern, replacement.as_deref(), span);
                if approximate {
                    diag.locations.push(Location {
                        file: ctx.doc.display.clone(),
                        span,
                        end: None,
                        snippet: None,
                        role: "primary",
                        approximate: true,
                    });
                }
                diags.push(diag);
            }
        }
    }
    (diags, targets)
}

impl DocumentRule for RedundantExpressionRule {
    fn id(&self) -> &'static str {
        "efficiency/redundant-expression"
    }

    fn description(&self) -> &'static str {
        "定型冗長表現(置き換え候補)"
    }

    fn run(&self, ctx: &mut DocContext<'_>) -> Result<RuleRun> {
        let entries: Vec<(String, Option<String>)> = self
            .patterns
            .iter()
            .map(|(p, r)| (p.clone(), Some(r.clone())))
            .collect();
        let (diags, targets) = scan_pattern_diags(
            ctx,
            &entries,
            |display, pattern, replacement, span| {
                let replacement = replacement.expect("冗長表現の走査では置き換え先が常に Some");
                let current = ctx.counter.count(pattern);
                let estimated = ctx.counter.count(replacement);
                // トークン差分は指定 encoding での局所比較(編集後の文書全体の
                // 実測とは別指標)。増減の符号を保存し単純合算しない(§7.4)
                Diagnostic::new(
                display.to_string(),
                span,
                "efficiency/redundant-expression",
                format!("冗長表現: 「{pattern}」→「{replacement}」"),
            )
            .severity(Severity::Warn)
            .review_question(format!(
                "「{pattern}」を「{replacement}」に短縮しても意味・敬意が変わらないか文脈を確認してください"
            ))
            .measurement(
                Measurement::measured("token_count", current as f64)
                    .unit("tokens")
                    .scope(format!("pattern:{pattern}")),
            )
            .measurement(
                Measurement::measured("token_count", estimated as f64)
                    .unit("tokens")
                    .scope(format!("candidate:{replacement}")),
            )
            .measurement(
                Measurement::measured("token_delta", current as f64 - estimated as f64)
                    .unit("tokens")
                    .method("local_encoding_count")
                    .scope("pattern"),
            )
            .limitation(
                "トークン差分はパターン単位の局所比較であり、編集後の文書全体の実測値ではありません",
            )
            },
        );
        Ok(RuleRun::complete(diags, targets))
    }
}

/// コンテンツ語比率の統計的外れ値として扱う下限(平均 - σ係数 x 標準偏差)。
/// 初期ヒューリスティック(実測による調整履歴はなし、#109)。統計的外れ値
/// 検出の慣用的な値(平均 ± 2σ ≒ 95%)を採用
const TOKEN_DENSITY_SIGMA: f64 = 2.0;
/// 比率を計算する段落の最小トークン数。短すぎる段落は比率が不安定なため除外する。
/// 初期ヒューリスティック(実測による調整履歴はなし、#109)
const TOKEN_DENSITY_MIN_TOKENS: usize = 20;
/// 統計を取るのに必要な測定対象段落の最小数。これ未満では外れ値を定義しない。
/// 初期ヒューリスティック(実測による調整履歴はなし、#109)
const TOKEN_DENSITY_MIN_PARAGRAPHS: usize = 3;

/// 情報密度の低い段落を検出する(設計書 §5.1)。
/// 段落ごとの「コンテンツ語(名詞・動詞)数 / トークン数」を取り、
/// 文書内で 平均 - 2σ を下回る段落を統計的外れ値として Info で報告する。
/// 密度が低い理由の意味判断(要約にできるか等)は行わない。
pub struct TokenDensityRule;

/// コンテンツ語(名詞・動詞)かどうか。名詞の非自立(こと・ため 等)と
/// 代名詞は機能語に近いためコンテンツ語に数えない。
fn is_content_word(m: &jaio_parse::morph::Morph) -> bool {
    match m.pos1.as_str() {
        "動詞" => true,
        "名詞" => !matches!(m.pos2.as_str(), "非自立" | "代名詞" | "数詞"),
        _ => false,
    }
}

impl DocumentRule for TokenDensityRule {
    fn id(&self) -> &'static str {
        "efficiency/token-density"
    }

    fn description(&self) -> &'static str {
        "情報密度の低い段落(コンテンツ語比率の統計的外れ値)"
    }

    fn run(&self, ctx: &mut DocContext<'_>) -> Result<RuleRun> {
        // 散文前提の統計: リスト主導の文書は段落標本の意味が崩れるため対象外
        if !ctx.is_prose_document() {
            return Ok(RuleRun::not_applicable(
                "散文比率が下限未満のため対象外です(リスト主導の文書)",
            ));
        }
        // 段落リーフごとの (行, コンテンツ語数, トークン数) を測定する。
        // 比率だけでなく分子・分母を保持し、診断へ根拠として載せる(#34)
        let mut measured: Vec<(usize, usize, usize)> = Vec::new();
        for leaf in ctx
            .leaves
            .iter()
            .filter(|l| l.kind == jaio_parse::doc::LeafKind::Paragraph)
        {
            let tokens = ctx.counter.count(&leaf.text);
            if tokens < TOKEN_DENSITY_MIN_TOKENS {
                continue;
            }
            let morphs = ctx.analyze(&leaf.text)?;
            let content = morphs.iter().filter(|m| is_content_word(m)).count();
            measured.push((leaf.line, content, tokens));
        }
        // 判定単位は文書。標本段落が足りなければ外れ値判定は成立しない(#36)
        if measured.len() < TOKEN_DENSITY_MIN_PARAGRAPHS {
            return Ok(RuleRun {
                diagnostics: Vec::new(),
                coverage: Coverage {
                    targets: 1,
                    measured: 0,
                    reason: Some(format!(
                        "判定に必要な標本段落が不足しています({} 件、必要 {} 件)",
                        measured.len(),
                        TOKEN_DENSITY_MIN_PARAGRAPHS
                    )),
                },
            });
        }

        let n = measured.len() as f64;
        let mean = measured
            .iter()
            .map(|(_, c, t)| *c as f64 / *t as f64)
            .sum::<f64>()
            / n;
        let variance = measured
            .iter()
            .map(|(_, c, t)| {
                let r = *c as f64 / *t as f64;
                (r - mean) * (r - mean)
            })
            .sum::<f64>()
            / n;
        let threshold = mean - TOKEN_DENSITY_SIGMA * variance.sqrt();

        let mut diags = Vec::new();
        for (line, content, tokens) in measured {
            let ratio = content as f64 / tokens as f64;
            if ratio < threshold {
                diags.push(
                    Diagnostic::new(
                        ctx.doc.display.clone(),
                        Span { line, col: 1 },
                        self.id(),
                        format!(
                            "コンテンツ語比率が文書平均を大きく下回ります(比率 {ratio:.2}、平均 {mean:.2})"
                        ),
                    )
                    .severity(Severity::Info)
                    .review_question(
                        "比率が低い理由が文脈上の問題か確認してください(接続表現の整理が適切かは意味判断です)",
                    )
                    .measurement(
                        Measurement::measured("content_word_ratio", ratio)
                            .method("content_morphs/tokens")
                            .unit("ratio")
                            .ratio(content as f64, tokens as f64)
                            .threshold(threshold, "<")
                            .scope("paragraph"),
                    )
                    .measurement(
                        Measurement::measured("content_word_ratio_mean", mean)
                            .unit("ratio")
                            .scope("document"),
                    )
                    .measurement(
                        Measurement::measured("sample_paragraphs", n)
                            .unit("count")
                            .scope("document"),
                    )
                    .limitation("コンテンツ語比率は情報価値そのものではありません"),
                );
            }
        }
        Ok(RuleRun::complete(diags, 1))
    }
}

/// 証拠スニペットの最大文字数(Issue #19)。
const SNIPPET_CHARS: usize = 40;

/// 同一文の重複出現(引用コピペ等)を検出する(設計書 §5.1)。
/// 正規化済みの文(空白・句読点・大小文字を除去)が閾値以上の長さで
/// 2 回以上出現したとき、2 回目以降を重複として報告する。
pub struct RepeatedQuoteRule;

impl DocumentRule for FillerRule {
    fn id(&self) -> &'static str {
        "efficiency/filler"
    }

    fn description(&self) -> &'static str {
        "フィラー句・空虚修飾(削除候補)"
    }

    fn run(&self, ctx: &mut DocContext<'_>) -> Result<RuleRun> {
        let entries: Vec<(String, Option<String>)> =
            self.patterns.iter().map(|p| (p.clone(), None)).collect();
        let (diags, targets) = scan_pattern_diags(
            ctx,
            &entries,
            |display, pattern, _replacement, span| {
                // フィラーは削除候補のため置き換え先を持たない(None 固定)
                let current = ctx.counter.count(pattern);
                Diagnostic::new(
                display.to_string(),
                span,
                "efficiency/filler",
                format!("削除候補のフィラー: 「{pattern}」"),
            )
            .severity(Severity::Warn)
            .review_question("削除しても意味が変わらないか文脈を確認してください".to_string())
            .measurement(
                Measurement::measured("token_count", current as f64)
                    .unit("tokens")
                    .scope(format!("pattern:{pattern}")),
            )
            .measurement(
                Measurement::measured("token_delta", current as f64)
                    .unit("tokens")
                    .method("local_encoding_count")
                    .scope("pattern"),
            )
            .limitation(
                "トークン差分はパターン単位の局所比較であり、編集後の文書全体の実測値ではありません",
            )
            },
        );
        Ok(RuleRun::complete(diags, targets))
    }
}

impl DocumentRule for RepeatedQuoteRule {
    fn id(&self) -> &'static str {
        "efficiency/repeated-quote"
    }

    fn description(&self) -> &'static str {
        "同一文の重複出現(引用コピペ等)"
    }

    fn run(&self, ctx: &mut DocContext<'_>) -> Result<RuleRun> {
        // 散文前提のルール: リスト主導の文書(課題管理ファイル等)では
        // 同一文言の反復が構造的であり、引用コピペの兆候と見なせない
        if !ctx.is_prose_document() {
            return Ok(RuleRun::not_applicable(
                "散文比率が下限未満のため対象外です(リスト主導の文書)",
            ));
        }
        let min_chars = ctx.detection.repeat_quote_min_chars;
        // 正規化済み文 → (出現行, 元の文) のリスト
        let mut index: std::collections::HashMap<String, Vec<(usize, String)>> =
            std::collections::HashMap::new();
        for leaf in ctx.leaves.iter().filter(|l| DocContext::is_body_leaf(l)) {
            for sentence in jaio_parse::ja::split_sentences(&leaf.text) {
                let normalized = normalize_text(&sentence.text);
                if normalized.chars().count() >= min_chars {
                    // スニペット用に元の文も保持する(Issue #19)
                    index
                        .entry(normalized)
                        .or_default()
                        .push((leaf.line, sentence.text));
                }
            }
        }

        let mut diags = Vec::new();
        // 判定対象は閾値以上の長さの文。0 件なら適用対象なし
        let targets: usize = index.values().map(|v| v.len()).sum();
        // 出現順に処理し、診断順序を決定的にする(HashMap の反復順に依存しない)
        let mut groups: Vec<Vec<(usize, String)>> =
            index.into_values().filter(|l| l.len() >= 2).collect();
        groups.sort();
        for lines in groups {
            // 2 回目以降の代表位置(最初の出現は原文と見なす)
            let line = lines[1].0;
            let mut diag = Diagnostic::new(
                ctx.doc.display.clone(),
                Span { line, col: 1 },
                self.id(),
                format!("同一文の重複出現({} カ所)", lines.len()),
            )
            .severity(Severity::Info)
            .review_question(
                "引用・定型注意・手順の反復として必要か、文脈で判断してください".to_string(),
            )
            .measurement(
                Measurement::measured("occurrence_count", lines.len() as f64)
                    .unit("count")
                    .scope("document"),
            );
            for &(line, ref text) in &lines {
                diag.locations.push(Location::occurrence(
                    ctx.doc.display.clone(),
                    Span { line, col: 1 },
                    jaio_core::snippet(text, SNIPPET_CHARS),
                ));
            }
            diags.push(diag);
        }
        Ok(RuleRun::complete(diags, targets))
    }
}

/// 本文リーフの各文と、位置特定に必要な情報を集める(#48 の文単位ルールの
/// 共通入力)。`occurrence` はリーフ内での同一文の出現順(0 基点)で、
/// `DocContext::locate` の位置特定に渡す
fn body_sentence_spans<'a>(ctx: &'a DocContext<'_>) -> Vec<(&'a TextLeaf, SentenceSpan, usize)> {
    let mut out = Vec::new();
    for leaf in ctx.leaves.iter().filter(|l| DocContext::is_body_leaf(l)) {
        // 同一文の再出現で locate の出現番号を進める。文の総数に対して
        // 同一文は少なく、clone の実コストは無視できる
        let mut seen: HashMap<String, usize> = HashMap::new();
        for sent in split_sentences_spans(&leaf.text) {
            let occurrence = seen.get(&sent.text).copied().unwrap_or(0);
            seen.insert(sent.text.clone(), occurrence + 1);
            out.push((leaf, sent, occurrence));
        }
    }
    out
}

/// 文の和文文字数(非 ASCII 文字の数)。ASCII(英字・URL・コード)は読み負荷が
/// 軽いため長さに数えない(#48: 英字列挙による誤検出の実測あり)
fn japanese_char_count(text: &str) -> usize {
    text.chars().filter(|c| !c.is_ascii()).count()
}

/// 文が読点(、/，/ 和文文字に後続する ASCII ,)を含むか。英字列挙の
/// 区切りカンマ(`a, b, c`)は読点と見なさない。列挙中黒「・」も和文文字に
/// 後続する場合に限り区切りとして数える(#60: 中黒区切りのラベル列挙は
/// 読解の区切りとして機能するため、no-comma の過敏検出を避ける)
fn has_japanese_pause(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c == '、' || c == '，' {
            return true;
        }
        if i > 0 && !chars[i - 1].is_ascii() && matches!(c, ',' | '・') {
            return true;
        }
    }
    false
}

/// 長い文(分割候補)を検出する(設計書 §5.1、#48)。
/// 和文文字数が `detection.long_sentence_min_chars` 以上の文を報告する。
/// 長文は形態素分解の誤読リスクを上げるため、token-density(段落単位)と
/// 文単位で補完する
pub struct LongSentenceRule;

impl DocumentRule for LongSentenceRule {
    fn id(&self) -> &'static str {
        "efficiency/long-sentence"
    }

    fn description(&self) -> &'static str {
        "長い文(分割候補)"
    }

    fn run(&self, ctx: &mut DocContext<'_>) -> Result<RuleRun> {
        let min_chars = ctx.detection.long_sentence_min_chars;
        let mut diags = Vec::new();
        let mut targets = 0usize;
        for (leaf, sent, occurrence) in body_sentence_spans(ctx) {
            let chars = japanese_char_count(&sent.text);
            if chars < min_chars {
                continue;
            }
            targets += 1;
            let span = ctx
                .locate(leaf, &sent.text, occurrence, None)
                .unwrap_or(Span {
                    line: leaf.line,
                    col: 1,
                });
            diags.push(
                Diagnostic::new(
                    ctx.doc.display.clone(),
                    span,
                    self.id(),
                    format!(
                        "長い文です(和文 {chars} 文字、閾値 {min_chars} 文字): 「{}」",
                        jaio_core::snippet(&sent.text, SNIPPET_CHARS)
                    ),
                )
                .severity(Severity::Warn)
                .review_question(
                    "意味を保ったまま 2 文以上に分割できるか確認してください".to_string(),
                )
                .measurement(
                    Measurement::measured("japanese_char_count", chars as f64)
                        .unit("chars")
                        .threshold(min_chars as f64, ">=")
                        .scope("sentence"),
                )
                .limitation("ASCII(英字・URL・コード)は読み負荷が軽いため文字数から除外しています"),
            );
        }
        Ok(RuleRun::complete(diags, targets))
    }
}

/// 読点を含まない長い文を検出する(設計書 §5.1、#48)。
/// 和文文字数が `detection.no_comma_min_chars` 以上で読点を含まない文を
/// 報告する。読点なし長文は修飾関係の曖昧性リスクの代理指標
pub struct NoCommaRule;

impl DocumentRule for NoCommaRule {
    fn id(&self) -> &'static str {
        "efficiency/no-comma"
    }

    fn description(&self) -> &'static str {
        "読点を含まない長い文"
    }

    fn run(&self, ctx: &mut DocContext<'_>) -> Result<RuleRun> {
        let min_chars = ctx.detection.no_comma_min_chars;
        let mut diags = Vec::new();
        let mut targets = 0usize;
        for (leaf, sent, occurrence) in body_sentence_spans(ctx) {
            let chars = japanese_char_count(&sent.text);
            if chars < min_chars {
                continue;
            }
            targets += 1;
            if has_japanese_pause(&sent.text) {
                continue;
            }
            let span = ctx
                .locate(leaf, &sent.text, occurrence, None)
                .unwrap_or(Span {
                    line: leaf.line,
                    col: 1,
                });
            diags.push(
                Diagnostic::new(
                    ctx.doc.display.clone(),
                    span,
                    self.id(),
                    format!(
                        "読点を含まない長い文です(和文 {chars} 文字、閾値 {min_chars} 文字): 「{}」",
                        jaio_core::snippet(&sent.text, SNIPPET_CHARS)
                    ),
                )
                .severity(Severity::Warn)
                .review_question(
                    "修飾関係が曖昧になっていないか、読点での分割を検討してください".to_string(),
                )
                .measurement(
                    Measurement::measured("japanese_char_count", chars as f64)
                        .unit("chars")
                        .threshold(min_chars as f64, ">=")
                        .scope("sentence"),
                )
                .measurement(
                    Measurement::measured("pause_count", 0.0)
                        .unit("count")
                        .scope("sentence"),
                )
                .limitation("URL・コード等の ASCII 列挙のカンマは読点に数えていません"),
            );
        }
        Ok(RuleRun::complete(diags, targets))
    }
}
