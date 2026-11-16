//! ソースコードコメントのモデル(§5.4 の doc-comment-drift の入力)。

use std::path::PathBuf;

/// コメント抽出済みのソースファイル 1 件。
#[derive(Debug, Clone)]
pub struct CommentFile {
    /// ソースファイルのパス
    pub path: PathBuf,
    /// 診断表示用パス
    pub display: String,
    /// 正規化済みのコメント本文(記号除去・連結済み)
    pub body: String,
    /// 本文に寄与した先頭コメントの開始行(1 基点)。診断の根拠位置用(#44)
    pub first_line: usize,
    /// ソースファイル内容の SHA-256(hex、#36 の入力識別用)
    pub sha256: String,
}
