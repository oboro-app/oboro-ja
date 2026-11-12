//! structure カテゴリ: Markdown 構造の乱れ(設計書 §5.3)。

use anyhow::Result;
use jaio_core::{Diagnostic, Measurement, Severity, Span};
use jaio_index::{bigram_jaccard_parts, normalize_text};
use jaio_parse::doc::{Heading, TextLeaf};

use crate::{Coverage, DocContext, DocumentRule, RuleRun};

/// 見出しレベルの跳び(h2 → h4 等)を検出する。
pub struct HeadingSkipRule;

impl DocumentRule for HeadingSkipRule {
    fn id(&self) -> &'static str {
        "structure/heading-skip"
    }

    fn description(&self) -> &'static str {
        "見出しレベルが 1 段階以上飛んでいる"
    }

    fn run(&self, ctx: &mut DocContext<'_>) -> Result<RuleRun> {
        let mut diags = Vec::new();
        let mut prev: Option<&Heading> = None;
        for heading in ctx.headings {
            if let Some(prev_heading) = prev
                && heading.level > prev_heading.level + 1
            {
                diags.push(
                    Diagnostic::new(
                        ctx.doc.display.clone(),
                        Span {
                            line: heading.line,
                            col: 1,
                        },
                        self.id(),
                        format!(
                            "見出しレベルの跳び: h{} の直下で h{} を使っています",
                            prev_heading.level, heading.level
                        ),
                    )
                    .severity(Severity::Warn)
                    .review_question(format!(
                        "h{} 以下のレベルを段階的に使うのが適切か、意図的な構造か確認してください",
                        prev_heading.level + 1
                    ))
                    .measurement(
                        Measurement::measured(
                            "heading_level_gap",
                            (heading.level - prev_heading.level) as f64,
                        )
                        .unit("count")
                        .scope("heading"),
                    ),
                );
            }
            prev = Some(heading);
        }
        Ok(RuleRun::complete(diags, ctx.headings.len()))
    }
}

/// 本文のない見出し(空セクション)を検出する。
pub struct EmptySectionRule;

impl DocumentRule for EmptySectionRule {
    fn id(&self) -> &'static str {
        "structure/empty-section"
    }

    fn description(&self) -> &'static str {
        "本文のない見出し(空セクション)"
    }

    fn run(&self, ctx: &mut DocContext<'_>) -> Result<RuleRun> {
        let mut diags = Vec::new();
        for heading in ctx.headings.iter().filter(|h| h.empty) {
            diags.push(
                Diagnostic::new(
                    ctx.doc.display.clone(),
                    Span {
                        line: heading.line,
                        col: 1,
                    },
                    self.id(),
                    format!("本文のない見出し: 「{}」", heading.title),
                )
                .severity(Severity::Info)
                .review_question(
                    "本文を追記するか、見出し自体が不要かを確認してください(生成ツールの規約の可能性があります)"
                        .to_string(),
                ),
            );
        }
        Ok(RuleRun::complete(diags, ctx.headings.len()))
    }
}

/// リスト・引用の過剰なネストを検出する(設計書 §5.3)。
/// 報告する深さは `jaio.toml` の `[detection] max_nest_depth` で調整できる。
pub struct OverNestingRule;

impl DocumentRule for OverNestingRule {
    fn id(&self) -> &'static str {
        "structure/over-nesting"
    }

    fn description(&self) -> &'static str {
        "過剰なリスト・引用のネスト"
    }

    fn run(&self, ctx: &mut DocContext<'_>) -> Result<RuleRun> {
        let mut diags = Vec::new();
        let max_depth = ctx.detection.max_nest_depth;
        for leaf in ctx.leaves.iter().filter(|l| l.nest_depth > max_depth) {
            diags.push(
                Diagnostic::new(
                    ctx.doc.display.clone(),
                    Span {
                        line: leaf.line,
                        col: 1,
                    },
                    self.id(),
                    format!("深いネスト: {} 段階の入れ子のテキストです", leaf.nest_depth),
                )
                .severity(Severity::Warn)
                .review_question(format!(
                    "階層を {} 段階以内に減らすか、見出しでの分割が適切か確認してください",
                    max_depth
                ))
                .measurement(
                    Measurement::measured("nest_depth", leaf.nest_depth as f64)
                        .unit("count")
                        .threshold(max_depth as f64, ">")
                        .scope("leaf"),
                ),
            );
        }
        Ok(RuleRun::complete(diags, ctx.leaves.len()))
    }
}

/// 文書内の重複セクションを検出する(統合の判断は人間/LLM に委譲、設計書 §2)。
/// 類似度しきい値と最小文字数は `jaio.toml` の `[detection]` で調整できる。
pub struct SectionDuplicationRule;

/// 比較用のセクション抽出(見出し位置と正規化済みテキスト)。
/// 文書間重複(cross-doc-duplication)の位置特定でも使う。
pub struct SectionSlice {
    pub line: usize,
    pub title: String,
    pub body: String,
}

/// 見出しごとに、次の見出しまでの本文を 1 セクションとして集める。
/// 本文は正規化済みで、`min_chars` 未満のセクションは除外する。
pub fn extract_sections(
    leaves: &[TextLeaf],
    headings: &[Heading],
    min_chars: usize,
) -> Vec<SectionSlice> {
    let mut sections: Vec<SectionSlice> = Vec::new();
    for (idx, heading) in headings.iter().enumerate() {
        let end_line = headings.get(idx + 1).map_or(usize::MAX, |next| next.line);
        let mut body = String::new();
        for leaf in leaves
            .iter()
            .filter(|l| l.line > heading.line && l.line < end_line)
        {
            if DocContext::is_body_leaf(leaf) {
                body.push_str(&leaf.text);
            }
        }
        let normalized = normalize_text(&body);
        if normalized.chars().count() >= min_chars {
            sections.push(SectionSlice {
                line: heading.line,
                title: heading.title.clone(),
                body: normalized,
            });
        }
    }
    sections
}

impl DocumentRule for SectionDuplicationRule {
    fn id(&self) -> &'static str {
        "structure/section-duplication"
    }

    fn description(&self) -> &'static str {
        "文書内の重複セクション"
    }

    fn run(&self, ctx: &mut DocContext<'_>) -> Result<RuleRun> {
        // 見出しごとに、次の見出しまでの本文を 1 セクションとして集める
        let sections = extract_sections(
            ctx.leaves,
            ctx.headings,
            ctx.detection.section_body_min_chars,
        );

        let mut diags = Vec::new();
        for idx in 0..sections.len() {
            let earlier = &sections[idx];
            for later in &sections[idx + 1..] {
                let (body_inter, body_union) = bigram_jaccard_parts(&earlier.body, &later.body);
                let body_sim = if body_union == 0 {
                    0.0
                } else {
                    body_inter as f64 / body_union as f64
                };
                if body_sim < ctx.detection.section_similarity {
                    continue;
                }
                let (head_inter, head_union) = bigram_jaccard_parts(
                    &normalize_text(&earlier.title),
                    &normalize_text(&later.title),
                );
                let heading_sim = if head_union == 0 {
                    0.0
                } else {
                    head_inter as f64 / head_union as f64
                };
                diags.push(
                    Diagnostic::new(
                        ctx.doc.display.clone(),
                        Span {
                            line: later.line,
                            col: 1,
                        },
                        self.id(),
                        format!(
                            "重複の可能性: 「{}」と本文が類似(本文 {body_sim:.2}、見出し {heading_sim:.2})",
                            earlier.title
                        ),
                    )
                    .severity(Severity::Info)
                    .review_question(format!(
                        "どちらかに統合できないか確認してください(正候補の判断は委譲: {} 行目)",
                        earlier.line
                    ))
                    .measurement(
                        Measurement::measured("bigram_jaccard", body_sim)
                            .method("bigram-jaccard")
                            .unit("ratio")
                            .ratio(body_inter as f64, body_union as f64)
                            .threshold(ctx.detection.section_similarity, ">=")
                            .scope("section_body_pair"),
                    )
                    .measurement(
                        Measurement::measured("bigram_jaccard", heading_sim)
                            .method("bigram-jaccard")
                            .unit("ratio")
                            .ratio(head_inter as f64, head_union as f64)
                            .scope("section_title_pair"),
                    )
                    .limitation("語彙類似度は意味的同一性を保証しません"),
                );
            }
        }
        // セクションが 2 件未満では比較自体が成立しない(判定不能、#36)
        let coverage = if sections.len() < 2 {
            Coverage {
                targets: sections.len(),
                measured: 0,
                reason: Some(format!(
                    "比較可能なセクションが {} 件しかありません",
                    sections.len()
                )),
            }
        } else {
            Coverage {
                targets: sections.len(),
                measured: sections.len(),
                reason: None,
            }
        };
        Ok(RuleRun {
            diagnostics: diags,
            coverage,
        })
    }
}
