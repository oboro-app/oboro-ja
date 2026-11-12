//! 文分割と文体(ですます/である)判定。設計書 §4.2 の層 2 のうち文レベルの解析。

use crate::morph::Morph;

/// 文体の分類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SentenceStyle {
    /// ですます体(敬体)
    Polite,
    /// である体(常体)
    Plain,
    /// 判定不能(体言止め・疑問文など)
    Neutral,
}

/// 文(終端記号を含む)。
#[derive(Debug, Clone)]
pub struct Sentence {
    /// 文テキスト(前後の空白を除去済み)
    pub text: String,
}

/// 文と入力テキスト内の位置(終端記号を含む)。位置付き計測(#48)に使う。
#[derive(Debug, Clone)]
pub struct SentenceSpan {
    /// 文テキスト(前後の空白を除去済み)
    pub text: String,
    /// 入力テキスト先頭からのバイトオフセット(trim 後の文の開始位置)
    pub byte_start: usize,
}

/// 文終端として扱う文字。
const TERMINATORS: [char; 5] = ['。', '！', '!', '？', '?'];

/// テキストを文に分割し、入力内のバイト位置も返す。
/// 分割規則は `split_sentences` と同じ(片方だけを変更しない)
pub fn split_sentences_spans(text: &str) -> Vec<SentenceSpan> {
    let mut out = Vec::new();
    let mut start = 0usize;
    for (idx, ch) in text.char_indices() {
        if TERMINATORS.contains(&ch) {
            let end = idx + ch.len_utf8();
            push_span(&mut out, text, start, end);
            start = end;
        }
    }
    push_span(&mut out, text, start, text.len());
    out
}

/// チャンクを trim して文として追加する。空白・終端記号のみのチャンクは
/// 文として数えない。`byte_start` は trim 後の文の開始バイト位置
fn push_span(out: &mut Vec<SentenceSpan>, text: &str, start: usize, end: usize) {
    let chunk = &text[start..end];
    let trimmed = chunk.trim();
    let meaningful = trimmed
        .chars()
        .any(|c| !c.is_whitespace() && !TERMINATORS.contains(&c));
    if meaningful {
        let leading = chunk.len() - chunk.trim_start().len();
        out.push(SentenceSpan {
            text: trimmed.to_string(),
            byte_start: start + leading,
        });
    }
}

/// テキストを文に分割する。テキストリーフは 1 ブロック分を想定しており、
/// ソフトブレイクは事前に空白化されている前提で改行区切りは行わない。
pub fn split_sentences(text: &str) -> Vec<Sentence> {
    split_sentences_spans(text)
        .into_iter()
        .map(|s| Sentence { text: s.text })
        .collect()
}

/// 敬体と判定する文末の助動詞表現。
const POLITE_SUFFIXES: [&str; 8] = [
    "ます",
    "ました",
    "ません",
    "ましょう",
    "ませ",
    "です",
    "でした",
    "でしょう",
];

/// 文末の形態素列から文体を判定する(設計書 §5.2 style-mixing の基盤)。
pub fn classify_style(morphs: &[Morph]) -> SentenceStyle {
    // 記号(句点等)を除く末尾トークン(最大 4 個)を文末側から取り出し、語順に戻す。
    // Sudachi 辞書では句読点等は「補助記号」「空白」になるため末尾「記号」で一律除外する。
    // 4 は文体判定に必要な形態素列の長さの実測上限(「廃止しませ(た)」=
    // 動詞+助動詞 2 個+補助記号を除いた tail が収まる。#110 で明記)
    let tail: Vec<&Morph> = morphs
        .iter()
        .rev()
        .filter(|m| !m.pos1.ends_with("記号") && m.pos1 != "空白")
        .take(4)
        .collect();
    let tail: Vec<&Morph> = tail.into_iter().rev().collect();

    // 文末近くに「ます」系・「です」系の助動詞があれば敬体。
    // 「廃止しました」が「し/まし/た」に分割されるように、助動詞は surface の
    // 接尾辞に加えて原形(lemma = ます)でも判定する
    for m in &tail {
        if m.pos1 == "助動詞"
            && (POLITE_SUFFIXES.iter().any(|s| m.surface.ends_with(s)) || m.lemma == "ます")
        {
            return SentenceStyle::Polite;
        }
    }
    // 「ください」も敬体の指示表現として扱う
    if tail.iter().any(|m| m.surface == "ください") {
        return SentenceStyle::Polite;
    }

    // 常体: 助動詞全般(敬体の助動詞は上の判定で既に返っているため、ここに来る
    // 助動詞は「だ」「た」「う」「らしい」等の常体系のみ)、動詞・形容詞の終止形、
    // 動詞の意志推量形(「〜しよう」「〜しよう。」)も常体扱いとする
    if let Some(last) = tail.last() {
        if last.pos1 == "助動詞" {
            return SentenceStyle::Plain;
        }
        if last.pos1 == "動詞"
            && (last.surface == "ある"
                || last.conjugation_form.contains("終止形")
                || last.conjugation_form.contains("意志"))
        {
            return SentenceStyle::Plain;
        }
        if last.pos1 == "形容詞" && last.conjugation_form.contains("終止形") {
            return SentenceStyle::Plain;
        }
    }
    SentenceStyle::Neutral
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::morph::Analyzer;
    use anyhow::Result;

    #[test]
    fn splits_on_terminators() {
        let sentences = split_sentences("ひとつめ。ふたつめ? みっつめ!");
        let texts: Vec<&str> = sentences.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, vec!["ひとつめ。", "ふたつめ?", "みっつめ!"]);
    }

    #[test]
    fn ignores_empty_chunks() {
        assert!(split_sentences("。  。").is_empty());
        assert_eq!(split_sentences("最後は終端なし").len(), 1);
    }

    #[test]
    fn spans_track_byte_positions() {
        // 位置付き分割(#48): byte_start は trim 後の文の開始バイト位置
        let text = "  ひとつめ。  ふたつめ。";
        let spans = split_sentences_spans(text);
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].text, "ひとつめ。");
        assert_eq!(&text[spans[0].byte_start..], "ひとつめ。  ふたつめ。");
        assert_eq!(spans[1].text, "ふたつめ。");
        assert_eq!(&text[spans[1].byte_start..], "ふたつめ。");
    }

    #[test]
    fn classifies_sentence_styles() -> Result<()> {
        let mut analyzer = Analyzer::new()?;
        let cases = [
            ("解析します。", SentenceStyle::Polite),
            ("解析することができます。", SentenceStyle::Polite),
            ("性能は十分である。", SentenceStyle::Plain),
            ("この文書はツールの検証に使う。", SentenceStyle::Plain),
            ("ここはである調である。", SentenceStyle::Plain),
            ("解析した。", SentenceStyle::Plain),
            ("ここでは文書を検証しよう。", SentenceStyle::Plain),
            // 「廃止しました」は「し/まし/た」に分割され、
            // 「まし」(lemma=ます)で敬体を判定する必要がある
            ("旧機能は廃止しました。", SentenceStyle::Polite),
            ("設定を読み込みました。", SentenceStyle::Polite),
        ];
        for (text, expected) in cases {
            let morphs = analyzer.analyze(text)?;
            let actual = classify_style(&morphs);
            assert_eq!(
                actual, expected,
                "判定不一致: {text} → {actual:?}(形態素: {morphs:?})"
            );
        }
        Ok(())
    }
}
