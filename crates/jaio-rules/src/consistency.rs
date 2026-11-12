//! consistency カテゴリ: 表記・文体の揺れ(設計書 §5.2)。

use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use jaio_core::config::StyleTarget;
use jaio_core::{Diagnostic, Location, Measurement, Severity, Span};
use jaio_parse::doc::LeafKind;
use jaio_parse::ja::{SentenceStyle, classify_style, split_sentences};
use jaio_parse::morph::Morph;

use crate::{Coverage, DocContext, DocumentRule, RuleRun};
use jaio_core::config::StyleOverrides;

use std::collections::HashSet;

/// 文体判定に十分な最低文数。これ未満しかない文書は文体の選択と見なし報告しない。
const MIN_SENTENCES: usize = 2;

/// 証拠スニペットの最大文字数(Issue #19)。
const SNIPPET_CHARS: usize = 40;

/// 文書の既定文体からの逸脱を検出する(設計書 §1.3、§5.2)。
/// 既定の文体は `target` で切り替えられる(jaio.toml の `[style]`)。
/// 文書単位の宣言も可能(#55): frontmatter の `jaio-style` が最優先、
/// 次に `[style.overrides]` のパスパターン、いずれも無ければ `target`。
///
/// - target 体が優勢(target 体数 >= 他文体数)の文書: 他文体の文を逸脱として warn する
/// - 他文体が優勢の文書: 文書全体の文体が既定から外れているため info を 1 件出す
///   (技術文書としてである体が適切な場合もあるため、warn にはしない)
pub struct StyleDeviationRule {
    /// このリポジトリが既定とする文体
    pub target: StyleTarget,
    /// パスパターンによる文書単位の上書き(jaio.toml の `[style.overrides]`)
    pub overrides: StyleOverrides,
}

impl StyleDeviationRule {
    /// この文書の実効文体を解決する。frontmatter の `jaio-style` 宣言が
    /// 最優先(文書自身の属性)、次に `[style.overrides]` のパスパターン、
    /// いずれも無ければリポジトリ既定
    fn effective_target(&self, ctx: &DocContext<'_>) -> StyleTarget {
        if let Some(declared) = ctx.doc.style_declaration() {
            return declared;
        }
        // doc.display は jaio-cli で cwd 相対に正規化される(cwd 配下の絶対
        // パスは寄せられ、cwd 外の実行では絶対のまま)。照合は StyleOverrides
        // が jaio.toml 基準で相対化して行うため(#76)、ここでは残る `./` を
        // normalize で除いて渡すだけ
        let path = crate::normalize_path(Path::new(&ctx.doc.display));
        if let Some(target) = self.overrides.get(&path) {
            return target;
        }
        self.target
    }
}

impl DocumentRule for StyleDeviationRule {
    fn id(&self) -> &'static str {
        "consistency/style-deviation"
    }

    fn description(&self) -> &'static str {
        // 文書単位の宣言(jaio-style / style.overrides)で実効文体は文書ごとに変わる。
        // 他ルールと揃えて description は 1 行にする(#110)
        match self.target {
            StyleTarget::Polite => "ですます体を既定とした文体の逸脱",
            StyleTarget::Plain => "である体を既定とした文体の逸脱",
        }
    }

    fn run(&self, ctx: &mut DocContext<'_>) -> Result<RuleRun> {
        // 文書単位の宣言(jaio-style / style.overrides)を解決した実効文体
        let target = self.effective_target(ctx);
        let mut polite: Vec<(usize, String)> = Vec::new();
        let mut plain: Vec<(usize, String)> = Vec::new();
        let mut sentences_seen = 0usize;
        // 文体判定の対象は段落のみ。箇条書きは体言止めが慣例のため除外する(設計書 §5.2)
        for leaf in ctx.leaves.iter().filter(|l| l.kind == LeafKind::Paragraph) {
            for sentence in split_sentences(&leaf.text) {
                sentences_seen += 1;
                let morphs = ctx.analyze(&sentence.text)?;
                match classify_style(&morphs) {
                    SentenceStyle::Polite => polite.push((leaf.line, sentence.text)),
                    SentenceStyle::Plain => plain.push((leaf.line, sentence.text)),
                    SentenceStyle::Neutral => {}
                }
            }
        }

        let occurrences = |sentences: &[(usize, String)]| -> Vec<Location> {
            sentences
                .iter()
                .map(|(line, text)| {
                    Location::occurrence(
                        ctx.doc.display.clone(),
                        Span {
                            line: *line,
                            col: 1,
                        },
                        jaio_core::snippet(text, SNIPPET_CHARS),
                    )
                })
                .collect()
        };

        // target 体と他文体の名称・集計を選ぶ
        let (target_name, other_name, target_count, other): (
            &str,
            &str,
            usize,
            &[(usize, String)],
        ) = match target {
            StyleTarget::Polite => ("ですます体", "である体", polite.len(), &plain),
            StyleTarget::Plain => ("である体", "ですます体", plain.len(), &polite),
        };
        let other_count = other.len();
        let target_help = match target {
            StyleTarget::Polite => "ですます体(丁寧体)へ統一してください",
            StyleTarget::Plain => "である体(常体)へ統一してください",
        };

        let mut diags = Vec::new();
        let style_measurements = || {
            vec![
                Measurement::measured("polite_sentences", polite.len() as f64)
                    .unit("count")
                    .scope("document"),
                Measurement::measured("plain_sentences", plain.len() as f64)
                    .unit("count")
                    .scope("document"),
                Measurement::measured("classified_sentences", (polite.len() + plain.len()) as f64)
                    .unit("count")
                    .scope("document"),
            ]
        };
        if target_count >= other_count {
            // target 体が優勢: 他文体の文が混入していれば逸脱として報告する
            if target_count >= MIN_SENTENCES && !other.is_empty() {
                let (line, text) = &other[0];
                let mut diag = Diagnostic::new(
                    ctx.doc.display.clone(),
                    Span {
                        line: *line,
                        col: 1,
                    },
                    self.id(),
                    format!(
                        "文体の逸脱: {target_name}を既定とする文書に{other_name}の文が {} 文あります。例: 「{}」",
                        other_count,
                        text
                    ),
                )
                .severity(Severity::Warn)
                .review_question(format!(
                    "{target_help}。引用や文書の役割による意図的な混在か文脈を確認してください"
                ));
                diag.measurements = style_measurements();
                diag.locations = occurrences(other);
                diags.push(diag);
            }
        } else if other_count >= MIN_SENTENCES {
            // 他文体が優勢: 文書全体の文体が既定から外れているので文書レベルで報告する
            let (line, text) = &other[0];
            let mut diag = Diagnostic::new(
                ctx.doc.display.clone(),
                Span {
                    line: *line,
                    col: 1,
                },
                self.id(),
                format!(
                    "文書全体が{other_name}です({other_name} {} 文 / {target_name} {} 文)。既定の文体は{target_name}です。冒頭の例: 「{}」",
                    other_count,
                    target_count,
                    text
                ),
            )
            .severity(Severity::Info)
            .review_question(format!(
                "{target_name}への移行が適切か判断してください。{}は jaio.toml の [style] か文書 frontmatter の jaio-style で宣言できます",
                match target {
                    StyleTarget::Polite => "である体が適切な文書",
                    StyleTarget::Plain => "ですます体が適切な文書",
                }
            ));
            diag.measurements = style_measurements();
            diag.locations = occurrences(other);
            diags.push(diag);
        }
        // 判定には文体が分類できた文が必要(#36)。MIN_SENTENCES 未満なら
        // 文書レベルの判定に到達できないので measured=0 とする
        let classified = polite.len() + plain.len();
        let coverage = if classified < MIN_SENTENCES {
            Coverage {
                targets: sentences_seen,
                measured: 0,
                reason: Some(format!(
                    "文体判定可能な文が {classified} 件しかありません(必要 {MIN_SENTENCES} 件)"
                )),
            }
        } else {
            Coverage {
                targets: sentences_seen,
                measured: sentences_seen,
                reason: None,
            }
        };
        Ok(RuleRun {
            diagnostics: diags,
            coverage,
        })
    }
}

/// 同一述語(動詞の原形)に対する格助詞の揺れ(てにをは)を検出する。
/// 述語の原形 → 格助詞の出現を集計し、多数派と異なる助詞を報告する。
/// 誤検出が多いため info で開始する(設計書 §12)。
pub struct ParticleRule;

/// 揺れと判断するのに必要な多数派の最低出現数。
const MIN_MAJORITY_USES: usize = 2;

/// 格助詞(が/を/に/で/へ/と/から/より等)かどうか。
/// Sudachi では助詞の中分類が「格助詞」となる。
fn is_case_particle(morph: &Morph) -> bool {
    morph.pos1 == "助詞" && morph.pos2 == "格助詞"
}

/// 述語キー表示の補正。Sudachi の正規化表記では基本動詞が漢字表記に
/// 正規化されるため(為る=する、成る=なる 等)、診断表示では一般的な
/// ひらがなの標準形で示す。
fn canonical_verb_lemma(lemma: &str) -> &str {
    match lemma {
        "為る" => "する",
        "成る" => "なる",
        "有る" => "ある",
        "居る" => "いる",
        "無い" => "ない",
        other => other,
    }
}

/// 揺れとして報告する助詞ペアは `detection.particle_pairs` での opt-in 制
/// (既定は空で報告しない)。格の意味役割(対象・手段・場所)の判定には格フレーム
/// 辞書と係り受け解析が必要で、決定論的な範囲では正当な使い分けと区別できない。
/// 意味判断は JSON 出力で LLM に委譲する(設計書 §5.2、§2)。
///
/// 実運用測定(非公開の 5 リポジトリ)に基づく判断:
/// - `を/が`: 主格と対格の正当な使い分けばかりで、揺れらしい例が 1 件もなかった
/// - `に/へ`: 方向表現の揺れとして正当な指摘も出るが、前側の格を拾う強化(#5)後は
///   log 調テキストで実際の構文と無関係な組が大量に集計され、大規模な実リポジトリで
///   104 件の誤報となった(#23)。よって既定では報告しない
///
/// 報告する多数派対少数派の最大頻度比。これを超える偏りは偶発と見なす。
/// 初期ヒューリスティック(実測による調整履歴はなし、#109)。
/// 調整する場合は scripts/eval-rules.py の境界ケースで再現率・誤報を確認する
const MAX_FREQUENCY_RATIO: usize = 3;

impl DocumentRule for ParticleRule {
    fn id(&self) -> &'static str {
        "consistency/particle"
    }

    fn description(&self) -> &'static str {
        "同一述語に対する格助詞の揺れ(てにをは)"
    }

    fn run(&self, ctx: &mut DocContext<'_>) -> Result<RuleRun> {
        #[derive(Default)]
        struct Usage {
            count: usize,
            first_line: usize,
        }
        // 述語キー(原形、複合は「名詞+動詞」)→ (格助詞 → 出現集計)
        let mut index: HashMap<String, HashMap<String, Usage>> = HashMap::new();
        let mut sentences_seen = 0usize;

        for leaf in ctx.leaves.iter().filter(|l| DocContext::is_body_leaf(l)) {
            for sentence in split_sentences(&leaf.text) {
                sentences_seen += 1;
                let morphs = ctx.analyze(&sentence.text)?;
                // 述語に至るまでの「名詞+格助詞」をすべて保留し、最初に来る動詞に
                // 紐付ける。複数格を持つ文(「A へ B を解析する」)でも前側の格を
                // 拾えるようにする(Issue #5。完全な助詞列の順序比較ではなく集合ベース)。
                // 複合動詞(「解析する」等)は直前の名詞をキーに結合する。
                // 注意: この近似では「AをBにする」の A の「を」はキー「Bする」に
                // 帰属する(係り受け解析がないため、正しい述語への割当はできない。
                // 文節・係り受け導入の検討は #81)。opt-in かつ info 重大度のため
                // この誤帰属は報告精度の限界として許容する
                let mut pending: Vec<&Morph> = Vec::new();
                let mut prev: Option<&Morph> = None;
                for morph in morphs.iter() {
                    if morph.pos1 == "動詞" {
                        if pending.is_empty() {
                            prev = Some(morph);
                            continue;
                        }
                        let verb_key = canonical_verb_lemma(&morph.lemma);
                        let key = match prev {
                            Some(noun) if noun.pos1 == "名詞" => {
                                format!("{}{}", noun.lemma, verb_key)
                            }
                            _ => {
                                // 裸の「する」は「〜とする」「〜にする」等の構文差を
                                // 助詞の揺れとして拾いやすいため、述語キーから除外する
                                if verb_key == "する" {
                                    prev = Some(morph);
                                    continue;
                                }
                                verb_key.to_string()
                            }
                        };
                        for particle in pending.drain(..) {
                            let usage = index
                                .entry(key.clone())
                                .or_default()
                                .entry(particle.surface.clone())
                                .or_insert(Usage {
                                    count: 0,
                                    first_line: leaf.line,
                                });
                            usage.count += 1;
                        }
                    } else if is_case_particle(morph) {
                        // 格助詞は名詞に付くため、直前が名詞のときだけ有効とする
                        if matches!(prev, Some(noun) if noun.pos1 == "名詞") {
                            pending.push(morph);
                        }
                    } else if morph.pos1.ends_with("記号") || morph.pos1 == "空白" {
                        pending.clear();
                    }
                    prev = Some(morph);
                }
            }
        }

        let mut diags = Vec::new();
        for (predicate, usages) in index {
            // 報告対象ペアごとに、その 2 つの助詞だけを取り出して比較する。
            // 3 種以上の助詞が混ざる述語でも、対象ペアの揺れは検出できる
            for (x, y) in &ctx.detection.particle_pairs {
                let (Some(xu), Some(yu)) = (usages.get(x), usages.get(y)) else {
                    continue;
                };
                let mut pair: [(&str, &Usage); 2] = [(x, xu), (y, yu)];
                // 多数派を正候補として提示する(判断は人間/LLM に委譲)
                pair.sort_by(|a, b| b.1.count.cmp(&a.1.count).then(a.0.cmp(b.0)));
                let (major_particle, major) = pair[0];
                let (minor_particle, minor) = pair[1];
                // 頻度比が極端なものは偶然の一致として扱わない
                if major.count > minor.count * MAX_FREQUENCY_RATIO {
                    continue;
                }
                if major.count < MIN_MAJORITY_USES {
                    continue;
                }
                let (major_count, minor_count) = (major.count, minor.count);
                let minor_line = minor.first_line;
                diags.push(
                    Diagnostic::new(
                        ctx.doc.display.clone(),
                        Span {
                            line: minor_line,
                            col: 1,
                        },
                        self.id(),
                        format!(
                            "格助詞の揺れ(てにをは): 「{predicate}」に「{major_particle}」({major_count}件)と「{minor_particle}」({minor_count}件)が混在"
                        ),
                    )
                    .severity(Severity::Info)
                    .review_question(format!(
                        "「{predicate}」に使う助詞をどちらかに統一できるか、意味上の使い分けがないか確認してください"
                    ))
                    .measurement(
                        Measurement::measured("particle_count", major_count as f64)
                            .unit("count")
                            .scope(format!("{predicate}:{major_particle}")),
                    )
                    .measurement(
                        Measurement::measured("particle_count", minor_count as f64)
                            .unit("count")
                            .scope(format!("{predicate}:{minor_particle}")),
                    )
                    .limitation("格の意味役割の判定は行っていません(opt-in ルール)"),
                );
            }
        }
        // 各文は格助詞パターンの検査を完走した単位とみなす(#36)。
        // 対象ペアが 0 件でも「見つからなかった」は正常な判定結果
        Ok(RuleRun {
            diagnostics: diags,
            coverage: Coverage {
                targets: sentences_seen,
                measured: sentences_seen,
                reason: None,
            },
        })
    }
}

/// 同一読みの動詞・形容詞等で表記が揺れている語を検出する。
/// 名詞は同音異義の誤検出が多いため対象外とし、用語統一(terminology)は
/// ペア辞書ベースで対応する(設計書 §5.2)。
/// 実運用測定(非公開のリポジトリ)により、読み一致だけでは「返る/変える」のような
/// 同音異義の別語まで拾うため、表記の共通文字数でも絞り込む。
pub struct NotationVariantRule {
    /// 許容する表記(jaio.toml の `[lint.allow] terms`)。
    /// この表記を含むペアは意図的な使い分けと見なして報告しない
    pub allowed: HashSet<String>,
}

/// 2 つの表記が同じ語の揺れらしいか(原形が異なる場合の補助ヒューリスティック)。
/// 送り仮名の揺れ(行う/行なう、見なす/みなさ)は共通文字が 2 字以上ある一方、
/// 同音異義の別語(返る/変える、返る/替える)は共通文字が 1 字程度に留まる
/// (実運用測定に基づく経験則)。
/// かな漢字揺れ(したがって/従って)は共通文字が送り仮名 1 字に留まるため
/// この関数では拾えず、原形(lemma)一致で判定する(呼び出し側)
fn looks_like_variant(a: &str, b: &str) -> bool {
    let common = a.chars().filter(|c| b.contains(*c)).count();
    common >= 2
}

impl DocumentRule for NotationVariantRule {
    fn id(&self) -> &'static str {
        "consistency/notation-variant"
    }

    fn description(&self) -> &'static str {
        "同一語彙の表記ゆれ(読みが同じで表記が異なる語)"
    }

    fn run(&self, ctx: &mut DocContext<'_>) -> Result<RuleRun> {
        // (品詞1, 読み) → (表記 → 出現集計)
        #[derive(Default)]
        struct Usage {
            count: usize,
            first_line: usize,
            /// 原形。同一原形の表記差は真の揺れ(かな漢字揺れを含む)として扱う
            lemma: String,
        }
        let mut index: HashMap<(String, String), HashMap<String, Usage>> = HashMap::new();

        for leaf in ctx.leaves {
            let morphs = ctx.analyze(&leaf.text)?;
            for m in morphs.iter() {
                if !matches!(m.pos1.as_str(), "動詞" | "形容詞" | "副詞") {
                    continue;
                }
                // 読みが短い語(2 音以下)は偶然の一致が多く対象外
                if m.reading.chars().count() < 3 {
                    continue;
                }
                let key = (m.pos1.clone(), m.reading.clone());
                let usage = index
                    .entry(key)
                    .or_default()
                    .entry(m.surface.clone())
                    .or_insert(Usage {
                        count: 0,
                        first_line: leaf.line,
                        lemma: m.lemma.clone(),
                    });
                usage.count += 1;
            }
        }

        let mut diags = Vec::new();
        for usages in index.into_values() {
            if usages.len() < 2 {
                continue;
            }
            // 多数派を正候補として提示する(判断は人間/LLM に委譲)
            let mut entries: Vec<(String, Usage)> = usages.into_iter().collect();
            entries.sort_by(|a, b| b.1.count.cmp(&a.1.count).then(a.0.cmp(&b.0)));
            let (major_surface, major) = &entries[0];
            // 多数派が 1 回しか出ていないと「少数派も 1 回」の偶然ペア
            // (どちらか一方を書き間えただけの孤立ミス)まで報告に乗るため、
            // 「揺れ」の成立条件として多数派に最低 2 出現を要求する。
            // 初期ヒューリスティック(実測による調整履歴はなし、#109)
            if major.count < 2 {
                continue;
            }
            for (minor_surface, minor) in &entries[1..] {
                // 許容表記(jaio.toml の [lint.allow])を含むペアは報告しない
                if self.allowed.contains(major_surface.as_str())
                    || self.allowed.contains(minor_surface.as_str())
                {
                    continue;
                }
                // 原形が一致するペアは同一語の表記差(かな漢字揺れ:
                // したがって/従って、行う/行なう 等)なので真の揺れとして報告する。
                // 原形が異なる場合は同音異義の別語(返る/変える 等)の恐れがあるため、
                // 表記の共通文字が少ないペアを報告しない(実運用測定による)
                if major.lemma != minor.lemma && !looks_like_variant(major_surface, minor_surface) {
                    continue;
                }
                diags.push(
                    Diagnostic::new(
                        ctx.doc.display.clone(),
                        Span {
                            line: minor.first_line,
                            col: 1,
                        },
                        self.id(),
                        format!(
                            "表記ゆれ: 「{major_surface}」({}件)と「{minor_surface}」({}件)",
                            major.count, minor.count
                        ),
                    )
                    .severity(Severity::Info)
                    .review_question(format!(
                        "「{major_surface}」への統一が適切か確認してください(製品名・引用・異義語の使い分けの可能性があります)"
                    ))
                    .measurement(
                        Measurement::measured("surface_count", major.count as f64)
                            .unit("count")
                            .scope(format!("surface:{major_surface}")),
                    )
                    .measurement(
                        Measurement::measured("surface_count", minor.count as f64)
                            .unit("count")
                            .scope(format!("surface:{minor_surface}")),
                    ),
                );
            }
        }
        // 各リーフは読み・表記の索引化を完走した単位とみなす(#36)
        Ok(RuleRun {
            diagnostics: diags,
            coverage: Coverage {
                targets: ctx.leaves.len(),
                measured: ctx.leaves.len(),
                reason: None,
            },
        })
    }
}

/// 内蔵の用語ペア(JTF 日本語標準スタイルガイド系の長音の有無など、
/// 技術文書で頻出する統一対象)。先頭が推奨表記。
const BUILTIN_TERM_PAIRS: &[(&str, &str)] = &[
    ("リポジトリ", "レポジトリ"),
    ("データベース", "データーベース"),
    ("サーバー", "サーバ"),
    ("コンピューター", "コンピュータ"),
    ("ユーザー", "ユーザ"),
    ("フォルダー", "フォルダ"),
    ("メモリー", "メモリ"),
    ("ディレクトリー", "ディレクトリ"),
    ("カテゴリー", "カテゴリ"),
    ("セキュリティー", "セキュリティ"),
    ("イニシャライザー", "イニシャライザ"),
    ("インデックス", "インデクス"),
];

/// ペア辞書ベースの用語揺れを検出する(設計書 §5.2)。
/// 名詞は読み・原形による自動検出だと同音異義の誤検出が多いため(実運用測定)、
/// 人手管理のペア(内蔵 + `[terms]` 拡張)のみを対象とする。
pub struct TerminologyRule {
    /// 適用するペア(推奨表記, 別表記)
    pairs: Vec<(String, String)>,
    /// 許容する表記(jaio.toml の `[lint.allow] terms`)。
    /// この表記を含むペアは意図的な使い分けと見なして報告しない
    allowed: HashSet<String>,
}

/// 内蔵ペア + ユーザー拡張(jaio.toml の `[terms]`)をマージしたペア辞書を作る。
/// 要素数が 2 でない・空を含むペアは無視する
pub(crate) fn merged_term_pairs(extra: &[Vec<String>]) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = BUILTIN_TERM_PAIRS
        .iter()
        .map(|(a, b)| ((*a).to_string(), (*b).to_string()))
        .collect();
    for pair in extra {
        if pair.len() == 2 && !pair[0].is_empty() && !pair[1].is_empty() {
            pairs.push((pair[0].clone(), pair[1].clone()));
        }
    }
    pairs
}

impl TerminologyRule {
    /// 内蔵ペア + ユーザー拡張(jaio.toml の `[terms]`)でルールを作る。
    /// 要素数が 2 でない・空を含むペアは無視する
    pub fn with_extra(extra: &[Vec<String>], allowed: &[String]) -> Self {
        Self {
            pairs: merged_term_pairs(extra),
            allowed: allowed.iter().cloned().collect(),
        }
    }
}

impl DocumentRule for TerminologyRule {
    fn id(&self) -> &'static str {
        "consistency/terminology"
    }

    fn description(&self) -> &'static str {
        "用語揺れ(ペア辞書ベース)"
    }

    fn run(&self, ctx: &mut DocContext<'_>) -> Result<RuleRun> {
        let mut diags = Vec::new();
        for (preferred, other) in &self.pairs {
            // 許容表記([lint.allow])を含むペアは意図的な使い分けと見なす
            if self.allowed.contains(preferred.as_str()) || self.allowed.contains(other.as_str()) {
                continue;
            }
            // 推奨表記を除去してから別表記を数える。
            // 「サーバー」が「サーバ」を部分文字列として含むため、
            // 除去しないと推奨表記の側を別表記として誤カウントする
            let count_in = |haystack: &str| -> usize {
                crate::count_excluding_preferred(haystack, preferred, other)
            };
            // preferred_count は素の matches で数える。other を範囲除外すると
            // 内蔵ペアの向き(other が preferred に含まれる)では推奨の出現まで
            // 消えて 0 になる(#100)。逆包含ペア(preferred が other に含まれる)
            // は jaio-core::Config::validate で設定エラーになるため、ここでは
            // 水増しの心配がない
            let preferred_count: usize = ctx
                .leaves
                .iter()
                .filter(|l| DocContext::is_body_leaf(l))
                .map(|l| l.text.matches(preferred.as_str()).count())
                .sum();
            let other_count: usize = ctx
                .leaves
                .iter()
                .filter(|l| DocContext::is_body_leaf(l))
                .map(|l| count_in(&l.text))
                .sum();
            if preferred_count == 0 || other_count == 0 {
                continue;
            }
            // 別表記が現れる最初の本文リーフ上の実位置を位置として報告する。
            // 推奨表記の部分文字列(「サーバー」の「サーバ」)を拾わないよう
            // 推奨表記を潰してから探す
            let span = ctx
                .leaves
                .iter()
                .filter(|l| DocContext::is_body_leaf(l))
                .find(|l| count_in(&l.text) > 0)
                .map(|leaf| {
                    ctx.locate(leaf, other, 0, Some(preferred)).unwrap_or(Span {
                        line: leaf.line,
                        col: 1,
                    })
                })
                .unwrap_or(Span { line: 1, col: 1 });
            diags.push(
                Diagnostic::new(
                    ctx.doc.display.clone(),
                    span,
                    self.id(),
                    format!(
                        "用語揺れ: 「{preferred}」({}件)と「{other}」({}件)",
                        preferred_count, other_count
                    ),
                )
                .severity(Severity::Info)
                .review_question(format!(
                    "「{preferred}」への統一が適切か確認してください(意図的な使い分けは [lint.allow] で対応)"
                ))
                .measurement(
                    Measurement::measured("term_count", preferred_count as f64)
                        .unit("count")
                        .scope(format!("term:{preferred}")),
                )
                .measurement(
                    Measurement::measured("term_count", other_count as f64)
                        .unit("count")
                        .scope(format!("term:{other}")),
                ),
            );
        }
        // 判定単位は用語ペア。許容表記を除く全ペアについて出現数を数え切った
        // (未出現も「揺れなし」の判定結果、#36)
        Ok(RuleRun {
            diagnostics: diags,
            coverage: Coverage {
                targets: self.pairs.len(),
                measured: self.pairs.len(),
                reason: None,
            },
        })
    }
}

/// 句読点の表記揺れを検出する(設計書 §5.2)。
/// 和文の読点 `、`・句点 `。`、全角の `，`・`．`、および和文文脈にある
/// ASCII の `,`・`.` が同一文書内で混在する場合に、少数派の出現位置を
/// warn で報告する。ASCII 形は数値・英語・URL と区別がつかないため、
/// 直前が和文文字のものだけを対象にする。
/// 片方のみの一貫した使用は文体の選択と見なし報告しない。
pub struct PunctuationRule;

/// 和文の文字(ひらがな・カタカナ・漢字・々〆〇・長音)か
pub(crate) fn is_ja_char(c: char) -> bool {
    matches!(
        c,
        '\u{3040}'..='\u{30FF}' | '\u{4E00}'..='\u{9FFF}' | '\u{3005}'..='\u{3007}'
    )
}

/// ASCII の `,`/`.` を和文の句読点と見なす文脈か(§5.2)。数値(1,000 / v1.2)・
/// 英語・URL は直前が和文文字にならないため除外される。直後は和文文字・空白・
/// 行末・閉じ括弧(」』)）)を許す
pub(crate) fn is_ascii_punct_in_ja_context(prev: Option<char>, next: Option<char>) -> bool {
    prev.is_some_and(is_ja_char)
        && next.is_none_or(|n| {
            is_ja_char(n) || n.is_whitespace() || matches!(n, '」' | '』' | ')' | '）')
        })
}

/// テキスト中で和文文脈にある ASCII 句読点 `mark` の位置(文字インデックス)を列挙
pub(crate) fn ja_context_ascii_positions(text: &str, mark: char) -> Vec<usize> {
    let chars: Vec<char> = text.chars().collect();
    (0..chars.len())
        .filter(|&i| {
            chars[i] == mark
                && is_ascii_punct_in_ja_context(
                    i.checked_sub(1).map(|j| chars[j]),
                    chars.get(i + 1).copied(),
                )
        })
        .collect()
}

/// 揺れとして扱う句読点グループ((和文形, 全角形, ASCII 形), 名称)。
/// ASCII 形は和文文脈にあるものだけを数える(§5.2)
pub(crate) const PUNCT_GROUPS: [((char, char, char), &str); 2] =
    [(('、', '，', ','), "読点"), (('。', '．', '.'), "句点")];

impl DocumentRule for PunctuationRule {
    fn id(&self) -> &'static str {
        "consistency/punctuation"
    }

    fn description(&self) -> &'static str {
        "句読点の表記揺れ(、/，/, ・ 。/．/. の混在。ASCII は和文文脈のみ)"
    }

    fn run(&self, ctx: &mut DocContext<'_>) -> Result<RuleRun> {
        let mut diags = Vec::new();
        for ((standard, fullwidth, ascii), name) in PUNCT_GROUPS {
            // 各表記の出現位置(先頭の出現を指す)と出現数(#42)。
            // 添字は 0=和文形、1=全角形、2=ASCII 形で優先順位の並びと同じ。
            // 多数派判定と件数は設計書 §5.2 のとおり出現数ベースとする
            let mut spans_of: [Vec<Location>; 3] = [Vec::new(), Vec::new(), Vec::new()];
            let mut counts_of: [usize; 3] = [0, 0, 0];
            for leaf in ctx.leaves.iter().filter(|l| DocContext::is_body_leaf(l)) {
                let context = jaio_core::snippet(&leaf.text, SNIPPET_CHARS);
                let fallback = Span {
                    line: leaf.line,
                    col: 1,
                };
                if leaf.text.contains(standard) {
                    counts_of[0] += leaf.text.matches(standard).count();
                    let span = ctx.locate(leaf, &standard.to_string(), 0, None);
                    let mut loc = Location::occurrence(
                        ctx.doc.display.clone(),
                        span.unwrap_or(fallback),
                        context.clone(),
                    );
                    loc.approximate = span.is_none();
                    spans_of[0].push(loc);
                }
                if leaf.text.contains(fullwidth) {
                    counts_of[1] += leaf.text.matches(fullwidth).count();
                    let span = ctx.locate(leaf, &fullwidth.to_string(), 0, None);
                    let mut loc = Location::occurrence(
                        ctx.doc.display.clone(),
                        span.unwrap_or(fallback),
                        context.clone(),
                    );
                    loc.approximate = span.is_none();
                    spans_of[1].push(loc);
                }
                // ASCII 形は和文文脈にあるものだけを出現扱いにする
                let ascii_positions = ja_context_ascii_positions(&leaf.text, ascii);
                if let Some(&first) = ascii_positions.first() {
                    counts_of[2] += ascii_positions.len();
                    // ソース上の位置を needle で探す。2 文字(直前文字 + ASCII)
                    // だと、同一の 2 文字が文脈外(URL・ファイル名等)に先に
                    // 出現したときその位置を誤って指す(#101)。直前文字 +
                    // ASCII + 直後文字の 3 文字で和文文脈側を一意化する
                    let prev = leaf.text.chars().nth(first - 1).unwrap_or(ascii);
                    let next = leaf
                        .text
                        .chars()
                        .nth(first + 1)
                        .map(|c| c.to_string())
                        .unwrap_or_default();
                    let needle = format!("{prev}{ascii}{next}");
                    let span = ctx.locate(leaf, &needle, 0, None);
                    let mut loc = Location::occurrence(
                        ctx.doc.display.clone(),
                        span.map(|s| Span {
                            line: s.line,
                            col: s.col + 1,
                        })
                        .unwrap_or(fallback),
                        context.clone(),
                    );
                    loc.approximate = span.is_none();
                    spans_of[2].push(loc);
                }
            }
            let marks = [standard, fullwidth, ascii];
            let present: Vec<usize> = (0..3).filter(|&i| counts_of[i] > 0).collect();
            if present.len() < 2 {
                continue;
            }
            // 多数派は出現数が最大の表記。同数なら和文形 > 全角形 > ASCII 形の
            // 優先(添字が小さい方)で和文形寄りに決める(#42)
            let major = present
                .iter()
                .min_by_key(|&&i| (std::cmp::Reverse(counts_of[i]), i))
                .copied()
                .unwrap();
            // 多数派以外の各表記を逸脱として報告し、多数派を非拘束的な候補として提示する
            for &i in &present {
                if i == major {
                    continue;
                }
                let minor_spans = &spans_of[i];
                let mut diag = Diagnostic::new(
                    ctx.doc.display.clone(),
                    minor_spans[0].span,
                    self.id(),
                    format!(
                        "{name}の表記が混在しています: 「{}」{}箇所と「{}」{}箇所",
                        marks[major],
                        counts_of[major],
                        marks[i],
                        counts_of[i]
                    ),
                )
                .severity(Severity::Warn)
                .review_question(format!(
                    "{name}を「{}」に統一するのが適切か確認してください(数式・コード・外国語を含む意図的な使い分けの可能性があります)",
                    marks[major]
                ))
                .measurement(
                    Measurement::measured("mark_count", counts_of[major] as f64)
                        .unit("count")
                        .scope(format!("mark:{}", marks[major])),
                )
                .measurement(
                    Measurement::measured("mark_count", counts_of[i] as f64)
                        .unit("count")
                        .scope(format!("mark:{}", marks[i])),
                )
                .measurement(
                    Measurement::measured("mark_leaf_count", minor_spans.len() as f64)
                        .unit("count")
                        .scope(format!("locations_reported:{}", marks[i])),
                )
                .limitation("ASCII 形は直前が和文文字のものだけを数えています(§5.2)");
                diag.locations = minor_spans.clone();
                diags.push(diag);
            }
        }
        // 判定単位は句読点グループ。両グループの出現数えを完走した
        // (単一表記の一貫した使用も「混在なし」の判定結果、#36)
        Ok(RuleRun {
            diagnostics: diags,
            coverage: Coverage {
                targets: PUNCT_GROUPS.len(),
                measured: PUNCT_GROUPS.len(),
                reason: None,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{canonical_verb_lemma, is_ascii_punct_in_ja_context};

    #[test]
    fn canonicalizes_sudachi_kanji_lemmas() {
        assert_eq!(canonical_verb_lemma("為る"), "する");
        assert_eq!(canonical_verb_lemma("成る"), "なる");
        assert_eq!(canonical_verb_lemma("有る"), "ある");
        assert_eq!(canonical_verb_lemma("居る"), "いる");
        assert_eq!(canonical_verb_lemma("無い"), "ない");
        // 対象外の語はそのまま返す
        assert_eq!(canonical_verb_lemma("解析する"), "解析する");
    }

    #[test]
    fn ascii_punct_requires_ja_context() {
        // 「です.」(行末)・「です.次」は和文文脈
        assert!(is_ascii_punct_in_ja_context(Some('す'), None));
        assert!(is_ascii_punct_in_ja_context(Some('す'), Some('次')));
        // 数値「1.5」は直前が数字なので対象外
        assert!(!is_ascii_punct_in_ja_context(Some('1'), Some('5')));
        // 英語「Hello, world」・URL「example.com」も対象外
        assert!(!is_ascii_punct_in_ja_context(Some('o'), Some(' ')));
        assert!(!is_ascii_punct_in_ja_context(Some('e'), Some('c')));
        // 「です.md」のように直後が Latin なら対象外(拡張子・URL の一部)
        assert!(!is_ascii_punct_in_ja_context(Some('す'), Some('m')));
    }
}
