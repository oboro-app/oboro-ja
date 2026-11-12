//! Corpus 構築の共通入力。check(並列経路)と dump(逐次経路)が
//! 同じ変換を使うことを保証する(#96・#39)。片側だけの修正で
//! 「check と dump が同じ実効設定・収集を使う」契約が崩れないようにする。
//! Corpus は入力(文書・コメント・セクション)を借用するため、所有する
//! comment_files / sections は呼び出し側が保持して参照を渡す

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Result;

use super::*;

/// Corpus 構築に必要な 1 文書分の入力(#96)。
pub(crate) struct CorpusDocInput {
    /// 正文(body leaf の連結を normalize_text したもの)
    pub body: String,
    /// ファイル更新時刻。recency 重みが無効のときは UNIX_EPOCH
    pub mtime: SystemTime,
    /// mtime 取得失敗時の operational error(呼び出し側で記録する)
    pub metadata_error: Option<JsonOperationalError>,
}

/// 解析済み 1 ファイルから Corpus 入力を組み立てる。
/// body は body leaf(段落+リスト)の連結を normalize_text したもの(#44)。
/// mtime は正候補スコアの recency 重みが有効なときだけ取得する(§5.5、#34)。
/// 既定(重み 0)では取得自体を行わない。必要なのに取得できなかった場合は
/// `metadata_error` を返すため、呼び出し側で operational error として記録し
/// 解析済みとして残す(#36)
pub(crate) fn corpus_doc_input(
    file: &Path,
    leaves: &[TextLeaf],
    want_mtime: bool,
) -> CorpusDocInput {
    let body: String = leaves
        .iter()
        .filter(|l| DocContext::is_body_leaf(l))
        .map(|l| l.text.as_str())
        .collect();
    let (mtime, metadata_error) = if want_mtime {
        match std::fs::metadata(file).and_then(|m| m.modified()) {
            Ok(mtime) => (mtime, None),
            Err(err) => (
                SystemTime::UNIX_EPOCH,
                Some(JsonOperationalError {
                    path: Some(normalize_display(file)),
                    stage: "metadata".to_string(),
                    reason: err.to_string(),
                }),
            ),
        }
    } else {
        (SystemTime::UNIX_EPOCH, None)
    };
    CorpusDocInput {
        body: normalize_text(&body),
        mtime,
        metadata_error,
    }
}

/// セクション列を構築する(重複位置の特定、§5.4)。
pub(crate) fn build_sections(
    section_inputs: &[(Vec<TextLeaf>, Vec<Heading>)],
    min_body: usize,
) -> Vec<Vec<SectionSlice>> {
    section_inputs
        .iter()
        .map(|(leaves, headings)| extract_sections(leaves, headings, min_body))
        .collect()
}

/// ソースコードの日本語コメントを抽出する(doc-comment-drift の入力、§5.4)。
/// ルールが lint 設定で抑制されているなら入力取得自体を行わない(#36)。
/// 抽出したコメントのソースも解析入力として識別し(#36)、doc と併せて
/// 表示パス昇順に並べ直し、収集順の偶然に依らない決定的な列にする。
/// 戻り値は (comment_files, 抽出失敗の operational errors)
pub(crate) fn collect_comment_inputs(
    paths: &[PathBuf],
    config: &Config,
    sources: &mut Vec<JsonSource>,
) -> Result<(Vec<CommentFile>, Vec<JsonOperationalError>)> {
    let (comment_files, errors) = if rule_suppressed(config, "relation/doc-comment-drift") {
        (Vec::new(), Vec::new())
    } else {
        collect_comment_files(paths, config)?
    };
    for comment in &comment_files {
        sources.push(JsonSource {
            path: comment.display.clone(),
            sha256: comment.sha256.clone(),
        });
    }
    sources.sort_by(|a, b| a.path.cmp(&b.path));
    Ok((comment_files, errors))
}

/// Corpus::new に渡す入力の借用束(#96)。所有は呼び出し側が保持する
pub(crate) struct CorpusBorrowed<'a> {
    pub docs: &'a [Doc],
    pub bodies: &'a [String],
    pub doc_tokens: &'a [usize],
    pub mtimes: &'a [SystemTime],
    pub sections: &'a [Vec<SectionSlice>],
    pub comment_files: &'a [CommentFile],
}

/// Corpus を構築する(check と dump の共通、#96)。
/// 明示指定されたファイルは orphan 判定から除外する(実体同一性キー、#44)
pub(crate) fn build_corpus<'a>(
    paths: &[PathBuf],
    config: &Config,
    inputs: CorpusBorrowed<'a>,
) -> Result<Corpus<'a>> {
    let explicit: HashSet<PathBuf> = paths
        .iter()
        .filter(|p| p.is_file())
        .map(|p| comparison_key(p))
        .collect();
    Corpus::new(
        inputs.docs,
        inputs.bodies,
        inputs.doc_tokens,
        inputs.mtimes,
        config.merge.clone(),
        config.detection.clone(),
        explicit,
        inputs.comment_files,
        inputs.sections,
    )
}
