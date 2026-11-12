//! jaio-collect: ソースコードからのコメント抽出(tree-sitter、設計書 §6.3)。
//! Rust / TypeScript / Python / Go / Bash / Swift の comment ノードを対象とする。

use std::path::Path;

use anyhow::Result;
use tree_sitter::{Language, Parser};

/// 対応する言語の一覧(`[comments] languages` の有効値)。
pub const SUPPORTED_LANGUAGES: &[&str] = &["rust", "typescript", "python", "go", "bash", "swift"];

/// 拡張子から言語名を解決する(設定での言語絞り込み用)。
/// 対応言語の単一表(拡張子 → 言語名・tree-sitter grammar)。
/// 言語名の絞り込み([comments] languages)と grammar 解決がこの 1 つの表を
/// 共有するため、拡張子を追加するときの取りこぼしが起きない(#110)。
/// 拡張子比較は大文字小文字を無視する(RS や Ts も対象)
type GrammarFn = fn() -> tree_sitter::Language;
const LANGUAGES: &[(&str, &str, GrammarFn)] = &[
    ("rs", "rust", || tree_sitter_rust::LANGUAGE.into()),
    ("ts", "typescript", || {
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
    }),
    ("tsx", "typescript", || {
        tree_sitter_typescript::LANGUAGE_TSX.into()
    }),
    ("py", "python", || tree_sitter_python::LANGUAGE.into()),
    ("go", "go", || tree_sitter_go::LANGUAGE.into()),
    ("sh", "bash", || tree_sitter_bash::LANGUAGE.into()),
    ("bash", "bash", || tree_sitter_bash::LANGUAGE.into()),
    ("swift", "swift", || tree_sitter_swift::LANGUAGE.into()),
];

pub fn language_name(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?;
    LANGUAGES
        .iter()
        .find(|(e, _, _)| e.eq_ignore_ascii_case(ext))
        .map(|(_, name, _)| *name)
}

/// 拡張子から tree-sitter 言語を解決する。
fn language_for(path: &Path) -> Option<Language> {
    let ext = path.extension()?.to_str()?;
    LANGUAGES
        .iter()
        .find(|(e, _, _)| e.eq_ignore_ascii_case(ext))
        .map(|(_, _, grammar)| grammar())
}

/// ソースコードからコメント(comment ノード)を開始行つきで抽出する。
/// 未対応の拡張子は空ベクタを返す。
pub fn extract_comments(path: &Path, source: &str) -> Result<Vec<ExtractedComment>> {
    let Some(language) = language_for(path) else {
        return Ok(Vec::new());
    };
    let mut parser = Parser::new();
    parser.set_language(&language)?;
    let Some(tree) = parser.parse(source, None) else {
        anyhow::bail!("パースに失敗しました: {}", path.display());
    };
    let mut comments = Vec::new();
    walk_comments(tree.root_node(), source, &mut comments);
    Ok(comments)
}

/// 抽出されたコメント 1 件。行番号は診断の根拠位置に使う(#44)
#[derive(Debug, Clone)]
pub struct ExtractedComment {
    /// コメント開始行(1 基点)
    pub line: usize,
    /// コメントの生テキスト(記号除去前)
    pub text: String,
}

/// AST を巡回してコメントノードのテキストと開始行を集める。
/// ノード種別は言語ごとに異なる: 大半は `comment`、Rust grammar は
/// `line_comment` / `block_comment`、Swift grammar は `multiline_comment` を使う。
fn walk_comments(node: tree_sitter::Node, source: &str, out: &mut Vec<ExtractedComment>) {
    match node.kind() {
        "comment" | "line_comment" | "block_comment" | "multiline_comment" => {
            // utf8_text はソースが &str のため通常失敗しないが、失敗時に
            // コメントが黙って欠落しないよう警告を出す(#110)
            let text = match node.utf8_text(source.as_bytes()) {
                Ok(text) => text,
                Err(err) => {
                    eprintln!(
                        "warning: コメントの読み取りに失敗したため欠落します({err}): 行 {}",
                        node.start_position().row + 1
                    );
                    // このノードをスキップし、子走査は続ける
                    return;
                }
            };
            // シバン(`#!`)は実質的なコメントでないため除外する。
            // 判定は「ファイル先頭行のコメント」に限定する(#94)。
            // 内容(`#!` を含む行)だけの判定にすると
            // `# !!! 注意 !!!` のような正当なコメントまで欠落していた
            if node.start_position().row == 0
                && text.trim_start_matches('#').trim_start().starts_with('!')
            {
                return;
            }
            out.push(ExtractedComment {
                line: node.start_position().row + 1,
                text: text.to_string(),
            });
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_comments(child, source, out);
    }
}

/// コメント記号(`//`、`///`、`/* */`、`#`、`*` 継続行)を除去して本文だけにする。
pub fn clean_comment(text: &str) -> String {
    let mut out = String::new();
    for line in text.lines() {
        let mut l = line.trim();
        // 行頭の / と * をすべて除去する(開始記号・継続行・閉じ記号に対応)
        l = l.trim_start_matches(['/', '*']);
        if l.starts_with('#') {
            l = &l[1..];
        }
        // インライン形式のブロックコメント(/* ... */)では閉じ記号が行末に残る
        l = l.trim_end().strip_suffix("*/").unwrap_or(l).trim_end();
        out.push_str(l.trim());
        out.push(' ');
    }
    out.trim().to_string()
}

/// 日本語(仮名・漢字)を含むか。日本語文書ツールの対象外コメントを弾く。
/// 半角カナ(U+FF61-FF9F)と CJK 拡張 A(U+3400-4DBF)も日本語とみなす(#44:
/// レガシー文書の半角カナや人名用漢字で対象から漏れないようにする)
/// 日本語(仮名・漢字)を含むか。日本語文書ツールの対象外コメントを弾く。
/// CJK 拡張 B 以面(サロゲート領域)にも対応する(#44)
pub fn contains_japanese(text: &str) -> bool {
    text.chars().any(|c| {
        ('\u{3040}'..='\u{309F}').contains(&c)          // 平仮名
            || ('\u{30A0}'..='\u{30FF}').contains(&c)    // 片仮名
            || ('\u{FF61}'..='\u{FF9F}').contains(&c)    // 半角カナ
            || ('\u{3400}'..='\u{4DBF}').contains(&c)    // CJK 統合漢字拡張 A
            || ('\u{4E00}'..='\u{9FFF}').contains(&c)    // CJK 統合漢字
            || ('\u{F900}'..='\u{FAFF}').contains(&c)    // CJK 互換漢字
            || ('\u{20000}'..='\u{2FFFF}').contains(&c) // CJK 拡張 B 以降
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_rust_comments() -> Result<()> {
        let source = r#"
//! モジュールの説明です。

/// 設定を読み込みます。
pub fn load() -> u32 {
    // 短い注記
    let url = "https://example.com/#comment"; // 文字列内は無視
    url.len() as u32
}
/* ブロックコメント
   ここも本文です */
"#;
        let comments = extract_comments(Path::new("a.rs"), source)?;
        assert_eq!(
            comments.len(),
            5,
            "comment ノードは 5 つのはず: {comments:?}"
        );
        assert!(comments[0].text.contains("モジュールの説明"));
        assert!(comments[1].text.contains("設定を読み込み"));
        Ok(())
    }

    #[test]
    fn shebang_is_excluded_but_bang_comments_are_kept() -> Result<()> {
        // シバン(先頭行の #!)は除外するが、それ以外の #! 始まりコメントは
        // 正当なコメントとして残す(#94)。内容だけの判定だと
        // `# !!! 注意 !!!` まで欠落していた
        let source = "#!/bin/bash\n# !!! 注意 !!!\n# 通常のコメント\n";
        let comments = extract_comments(Path::new("a.sh"), source)?;
        assert_eq!(comments.len(), 2, "{comments:?}");
        assert!(comments[0].text.contains("注意"));
        assert!(comments[1].text.contains("通常のコメント"));
        Ok(())
    }

    #[test]
    fn extracts_python_and_go_comments() -> Result<()> {
        let py = "# 設定を読み込みます\ndef load():\n    return 1\n";
        let comments = extract_comments(Path::new("a.py"), py)?;
        assert_eq!(comments.len(), 1);

        let go = "package main\n\n// 設定を読み込みます\nfunc load() int { return 1 }\n";
        let comments = extract_comments(Path::new("a.go"), go)?;
        assert_eq!(comments.len(), 1);
        Ok(())
    }

    #[test]
    fn extracts_typescript_comments() -> Result<()> {
        let ts = "// 設定を読み込みます\nexport function load(): number {\n  return 1;\n}\n";
        let comments = extract_comments(Path::new("a.ts"), ts)?;
        assert_eq!(comments.len(), 1);
        Ok(())
    }

    #[test]
    fn unsupported_extension_returns_empty() -> Result<()> {
        assert!(extract_comments(Path::new("a.txt"), "x")?.is_empty());
        // 対応拡張子はパースできる
        assert!(!extract_comments(Path::new("a.rs"), "// c")?.is_empty());
        Ok(())
    }

    #[test]
    fn cleans_comment_markers() {
        assert_eq!(
            clean_comment("/// 設定を読み込みます。"),
            "設定を読み込みます。"
        );
        assert_eq!(clean_comment("// 単行"), "単行");
        assert_eq!(
            clean_comment("/*\n * 複数行\n * コメント\n */"),
            "複数行 コメント"
        );
        assert_eq!(clean_comment("# Python"), "Python");
        // インラインのブロックコメントは行末の閉じ記号も除去する
        assert_eq!(clean_comment("/* インライン */"), "インライン");
        assert_eq!(clean_comment("/** ドキュメント */"), "ドキュメント");
    }

    #[test]
    fn extracts_single_line_comment() -> Result<()> {
        let comments = extract_comments(Path::new("a.rs"), "// c1\nfn a() {}\n")?;
        assert_eq!(comments.len(), 1, "単一行コメント 1 つのはず");
        assert_eq!((comments[0].line, comments[0].text.as_str()), (1, "// c1"));
        Ok(())
    }

    #[test]
    fn detects_japanese() {
        assert!(contains_japanese("設定を読み込みます"));
        assert!(contains_japanese("カタカナ"));
        // 半角カナと CJK 拡張 A(人名用漢字等)も日本語として扱う(#44)
        assert!(contains_japanese("ﾃｽﾄｹｰｽ"));
        assert!(contains_japanese("㐂び"));
        assert!(!contains_japanese("ascii only 123"));
    }

    #[test]
    fn records_comment_start_lines() -> Result<()> {
        // 診断の根拠位置として開始行(1 基点)を保持する(#44)
        let source = "fn a() {}\n\n// 3 行目のコメント\nfn b() {}\n\n// 6 行目のコメント\n";
        let comments = extract_comments(Path::new("a.rs"), source)?;
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[0].line, 3);
        assert_eq!(comments[1].line, 6);
        Ok(())
    }
}

#[cfg(test)]
mod bash_swift_tests {
    use super::*;

    #[test]
    fn extracts_bash_comments() -> Result<()> {
        // sh / bash の # コメント(Issue #11。実リポジトリで日本語コメントが
        // 多数確認されたため追加した)
        let source = "#! /bin/bash\n# 設定を読み込みます\nset -eu\n\n# ログを出力する\necho ok\n";
        let comments = extract_comments(Path::new("a.sh"), source)?;
        assert_eq!(comments.len(), 2, "シバン(!)は除外する: {comments:?}");
        assert!(comments[0].text.contains("設定を読み込みます"));
        assert!(comments[1].text.contains("ログを出力"));
        // .bash 拡張子も同じ言語
        let comments = extract_comments(Path::new("a.bash"), source)?;
        assert_eq!(comments.len(), 2);
        Ok(())
    }

    #[test]
    fn extracts_swift_comments() -> Result<()> {
        // Swift の行コメントとブロックコメント(日本語コメントの実績が
        // 実リポジトリ 2 つで 33 ファイル中 26、43 中 28 で確認された)
        let source = "// 初期化が成功したことを確認する\nfunc run() {}\n/* 複数行の\n   説明コメント */\nlet x = 1\n";
        let comments = extract_comments(Path::new("a.swift"), source)?;
        assert_eq!(
            comments.len(),
            2,
            "行 + ブロックの 2 コメントのはず: {comments:?}"
        );
        assert!(comments[0].text.contains("初期化が成功"));
        assert!(comments[1].text.contains("複数行"));
        Ok(())
    }
}
