//! Markdown 文書構造 AST の構築(comrak)。設計書 §4.1 の層 1 に相当する。

use std::path::PathBuf;

use comrak::nodes::{AstNode, NodeValue};
use comrak::{Arena, Options, parse_document};
use jaio_core::config::StyleTarget;

/// 解析対象ドキュメント。
pub struct Doc {
    /// ファイルパス
    pub path: PathBuf,
    /// 診断表示用パス
    pub display: String,
    /// ソーステキスト
    pub source: String,
    /// YAML frontmatter の生テキスト(デリミタ行 `---` を含む、ソース先頭と
    /// 一致する。comrak の仕様で閉じデリミタ直後の空行 1 つまで含む)。
    /// 無い場合は None。層別トークン計測(#53)のために保持する
    pub frontmatter: Option<String>,
    /// 文書内のリンク列(§4.3)
    pub links: Vec<MarkdownLink>,
    /// 各行の開始バイト位置(line_offsets[i] = i+1 行目の先頭)。
    /// 改行は `\n` のみを想定(comrak も行区切りは \n。CR 単独改行は
    /// クラシック Mac 形式で現在は非対応。#110 で明記)
    /// リーフの行範囲からソースを O(1) で切り出すために使う(Issue #30)
    pub line_offsets: Vec<usize>,
}

/// テキストリーフの種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeafKind {
    /// 見出し
    Heading,
    /// 段落
    Paragraph,
    /// リスト項目
    ListItem,
    /// 表セル
    TableCell,
}

/// テキストリーフ(ブロック 1 つ分の平文化テキストと位置)。
#[derive(Debug, Clone)]
pub struct TextLeaf {
    /// 平文化テキスト(ソフトブレイクは空白化、コードは除外)
    pub text: String,
    /// 開始行番号(1 基点)
    pub line: usize,
    /// 終了行番号(1 基点、両端含む)。ブロックが複数行にまたがるときの
    /// ソース上の行範囲(位置根拠の精度管理などに使う)
    pub end_line: usize,
    /// 種別
    pub kind: LeafKind,
    /// リスト・引用のネスト深さ(最上位のリスト・引用が 1)
    pub nest_depth: u8,
}

/// 見出し情報。
#[derive(Debug, Clone)]
pub struct Heading {
    /// 見出しレベル(1〜6)
    pub level: u8,
    /// 見出しテキスト
    pub title: String,
    /// 行番号(1 基点)
    pub line: usize,
    /// 直後に本文ブロックが続かない(空セクション)か
    pub empty: bool,
}

/// Markdown 内のリンク種別。リンクと画像で扱いが異なるため区別する(#74):
/// 両方とも broken-link の実在検証に使うが、被リンク集計はリンクのみ
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    /// `[text](url)` 形式のリンク
    Link,
    /// `![alt](url)` 形式の画像
    Image,
}

/// Markdown 内のリンク(設計書 §4.3 のリンクグラフの入力)。
#[derive(Debug, Clone)]
pub struct MarkdownLink {
    /// リンク先 URL(表記のまま)
    pub url: String,
    /// 行番号(1 基点)
    pub line: usize,
    /// リンク種別(リンク / 画像、#74)
    pub kind: LinkKind,
}

/// 文書解析に使う comrak オプション。verify-move 等の構文範囲取得でも
/// 同じ解釈を使うため共有する(#66)
pub fn markdown_options() -> Options<'static> {
    let mut options = Options::default();
    options.extension.table = true;
    options.extension.strikethrough = true;
    options.extension.autolink = true;
    // tasklist(- [ ])は無効のまま。有効にするとチェックボックスが
    // leaf テキストから消え、文・トークン計測の対象が mdx 等の他ツールと
    // ずれる。`[ ]` 表記自体は本文テキストに残り、解析への影響は微小
    // (無効の意図の明示、#110)
    // .mdx 等で使われる frontmatter を本文解析から除外する
    // (comrak では extension.front_matter_delimiter 指定で有効化する)
    options.extension.front_matter_delimiter = Some("---".to_string());
    options
}

impl Doc {
    /// frontmatter の `jaio-style` 宣言(`jaio-style: polite` 等)を返す(#55)。
    /// 宣言なし・不正な値は None。YAML の完全パースは行わず `key: value` 行の
    /// 簡易抽出にとどめる(値は対で閉じたクォート付きを許容する)
    ///
    /// 対象はルート mapping のキーのみ(#67): ブロックスカラー(`|`・`>`)の
    /// 内容行やネストした mapping のキーは必ず字下げされるため、行頭から
    /// 始まる `jaio-style:` だけを宣言とみなす。`jaio-style:plain` のように
    /// `:` の直後が空白でない行は YAML ではスカラー文字列であり宣言にしない。
    /// タブ区切り・閉じていないクォート・宣言の重複は YAML として不正な
    /// ため、簡易抽出でも宣言無効として扱う(#69)
    pub fn style_declaration(&self) -> Option<StyleTarget> {
        let frontmatter = self.frontmatter.as_deref()?;
        let mut seen = false;
        let mut found: Option<StyleTarget> = None;
        for line in frontmatter.lines() {
            let Some(rest) = line.strip_prefix("jaio-style:") else {
                continue;
            };
            // タブは YAML の区切りに使えないため空白のみを値の開始と認める
            if !(rest.is_empty() || rest.starts_with(' ')) {
                continue;
            }
            // 行内コメント(空白 + `#`)を除いてから値のクォートを外す
            let value = rest.trim();
            // split の next は常に Some を返すため unwrap_or_default は不要。
            // クォート内の ` #` も壊すが、polite/plain に # を含めない運用で
            // 影響しない(#110 で明記)
            let value = value.split(" #").next().unwrap_or(value).trim();
            // クォートは対でのみ剥がす。閉じていないクォートは不正な値として残す
            let value = if value.len() >= 2
                && ((value.starts_with('"') && value.ends_with('"'))
                    || (value.starts_with('\'') && value.ends_with('\'')))
            {
                value[1..value.len() - 1].trim()
            } else {
                value
            };
            let target = match value {
                "polite" => Some(StyleTarget::Polite),
                "plain" => Some(StyleTarget::Plain),
                _ => None,
            };
            if seen {
                // YAML の重複キーはエラー。2 回目の出現で宣言全体を無効にする
                return None;
            }
            seen = true;
            found = target;
        }
        found
    }

    /// Markdown を解析し、テキストリーフ列・見出し構造・リンク列を抽出する。
    pub fn parse(path: PathBuf, source: String) -> (Doc, Vec<TextLeaf>, Vec<Heading>) {
        let display = path.display().to_string();
        let options = markdown_options();

        let arena = Arena::new();
        let root = parse_document(&arena, &source, &options);

        let mut leaves = Vec::new();
        let mut headings = Vec::new();
        let mut links = Vec::new();
        let mut frontmatter = None;
        for child in root.children() {
            // frontmatter は本文解析の対象外。層別計測(#53)のため生テキストを
            // 保持するだけにとどめる(既存のリーフ・見出し除外挙動は維持)
            if let NodeValue::FrontMatter(literal) = &child.data.borrow().value {
                frontmatter = Some(literal.clone());
                continue;
            }
            walk_block(child, LeafKind::Paragraph, 0, &mut leaves, &mut headings);
            collect_links(child, &mut links);
        }
        let line_offsets = std::iter::once(0)
            .chain(source.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        (
            Doc {
                path,
                display,
                links,
                source,
                frontmatter,
                line_offsets,
            },
            leaves,
            headings,
        )
    }
}

/// ブロックノードを巡回する。inherited はコンテナ(リスト等)から引き継ぐ種別、
/// depth はリスト・引用のネスト深さ(最上位のリスト・引用が 1)。
fn walk_block<'a>(
    node: &'a AstNode<'a>,
    inherited: LeafKind,
    depth: u8,
    leaves: &mut Vec<TextLeaf>,
    headings: &mut Vec<Heading>,
) {
    // コードブロックは日本語解析の対象外
    if matches!(node.data.borrow().value, NodeValue::CodeBlock(_)) {
        return;
    }
    let value = node.data.borrow().value.clone();
    match value {
        NodeValue::Heading(h) => {
            let (line, end_line, text) = leaf_position(node);
            // 次のブロックが同じか浅いレベルの見出し、または文書末なら空セクション。
            // より深いレベルの小見出しが続く場合は導入部として本文を要求しない。
            // 同レベル見出しの直後にテーマブレイク等が続くケースは「本文あり」に
            // なる近似(擬似本文で空セクションの誤報を防ぐ意図。#110 で明記)
            let empty = match node.next_sibling() {
                None => true,
                Some(next) => match next.data.borrow().value {
                    NodeValue::Heading(next_heading) => next_heading.level <= h.level,
                    _ => false,
                },
            };
            leaves.push(TextLeaf {
                text: text.clone(),
                line,
                end_line,
                kind: LeafKind::Heading,
                nest_depth: depth,
            });
            headings.push(Heading {
                level: h.level,
                title: text,
                line,
                empty,
            });
        }
        NodeValue::Paragraph => {
            let (line, end_line, text) = leaf_position(node);
            if !text.trim().is_empty() {
                leaves.push(TextLeaf {
                    text,
                    line,
                    end_line,
                    kind: inherited,
                    nest_depth: depth,
                });
            }
        }
        NodeValue::TableCell => {
            let (line, end_line, text) = leaf_position(node);
            leaves.push(TextLeaf {
                text,
                line,
                end_line,
                kind: LeafKind::TableCell,
                nest_depth: depth,
            });
        }
        NodeValue::Item(_) => {
            for child in node.children() {
                walk_block(child, LeafKind::ListItem, depth, leaves, headings);
            }
        }
        NodeValue::List(_) => {
            for child in node.children() {
                walk_block(child, inherited, depth + 1, leaves, headings);
            }
        }
        NodeValue::BlockQuote => {
            for child in node.children() {
                walk_block(child, inherited, depth + 1, leaves, headings);
            }
        }
        _ => {
            for child in node.children() {
                walk_block(child, inherited, depth, leaves, headings);
            }
        }
    }
}

/// ノードの開始行・終了行と、インラインテキストの平文化を返す。
fn leaf_position<'a>(node: &'a AstNode<'a>) -> (usize, usize, String) {
    let sourcepos = node.data.borrow().sourcepos;
    let mut text = String::new();
    collect_text(node, &mut text);
    (sourcepos.start.line, sourcepos.end.line, text)
}

/// インラインノードからテキストを収集する。インラインコードも例示文の
/// 文脈を保つために含める(コードブロックは walk_block の段階で除外済み)、
/// 改行は空白化する
fn collect_text<'a>(node: &'a AstNode<'a>, out: &mut String) {
    let value = node.data.borrow().value.clone();
    match value {
        NodeValue::Text(t) => out.push_str(&t),
        NodeValue::SoftBreak | NodeValue::LineBreak => out.push(' '),
        // インラインコードも本文の一部として含める(コード内の用語も
        // 検出対象にする設計)。ASCII 記号を含むコードで文分割位置が
        // ずれ得る点は既知の近似(TERMINATORS の側で考慮しない。#110 で明記)
        NodeValue::Code(code) => out.push_str(&code.literal),
        _ => {}
    }
    for child in node.children() {
        collect_text(child, out);
    }
}

/// ノード配下のリンクを収集する(§4.3 のリンクグラフの入力)。
/// リンクと画像の両方を収集する(#74)。実在検証の扱い分けは利用側で行う
fn collect_links<'a>(node: &'a AstNode<'a>, out: &mut Vec<MarkdownLink>) {
    let value = node.data.borrow().value.clone();
    let (url, kind) = match value {
        NodeValue::Link(link) => (Some(link.url), LinkKind::Link),
        NodeValue::Image(link) => (Some(link.url), LinkKind::Image),
        _ => (None, LinkKind::Link),
    };
    if let Some(url) = url {
        let line = node.data.borrow().sourcepos.start.line;
        out.push(MarkdownLink { url, line, kind });
    }
    for child in node.children() {
        collect_links(child, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_headings_and_leaves() {
        let source = "# 題名\n\n本文です。\n\n## 空セクション\n\n## 使い方\n\n- 手順1\n";
        let (_doc, leaves, headings) = Doc::parse(PathBuf::from("test.md"), source.to_string());
        assert_eq!(headings.len(), 3);
        assert_eq!(headings[0].level, 1);
        assert!(headings[1].empty, "直後に見出しが続くセクションは空");
        assert!(!headings[2].empty);
        // 見出し 3 + 段落 1 + リスト項目 1 = 5
        assert_eq!(leaves.len(), 5);
        assert_eq!(leaves[1].kind, LeafKind::Paragraph);
        assert_eq!(leaves[2].kind, LeafKind::Heading);
        assert_eq!(leaves[4].kind, LeafKind::ListItem);
    }

    #[test]
    fn code_block_is_excluded() {
        let source = "    jaio check .\n\n本文です。\n";
        let (_doc, leaves, _headings) = Doc::parse(PathBuf::from("test.md"), source.to_string());
        assert_eq!(leaves.len(), 1);
        assert_eq!(leaves[0].text, "本文です。");
    }

    #[test]
    fn extracts_links() {
        let source =
            "# 題名\n\n[設定](./config.md)と[外部](https://example.com/)へのリンクです。\n";
        let (doc, _leaves, _headings) = Doc::parse(PathBuf::from("test.md"), source.to_string());
        let urls: Vec<&str> = doc.links.iter().map(|l| l.url.as_str()).collect();
        assert_eq!(urls, vec!["./config.md", "https://example.com/"]);
        assert_eq!(doc.links[0].line, 3);
    }

    #[test]
    fn image_links_are_collected_with_kind() {
        // #74: 画像 ![alt](url) もリンクと同じように収集する(種別付き)。
        // 実在検証・被リンク集計の扱い分けは利用側(corpus ルール)が行う
        let source = "前段です。\n\n![図](images/fig.png)\n\n[参照](other.md)\n";
        let (doc, _leaves, _headings) = Doc::parse(PathBuf::from("test.md"), source.to_string());
        assert_eq!(doc.links.len(), 2);
        assert_eq!(doc.links[0].url, "images/fig.png");
        assert_eq!(doc.links[0].kind, LinkKind::Image);
        assert_eq!(doc.links[0].line, 3);
        assert_eq!(doc.links[1].url, "other.md");
        assert_eq!(doc.links[1].kind, LinkKind::Link);
    }

    #[test]
    fn frontmatter_is_not_prose() {
        // `---` 囲みの frontmatter は本文リーフにも見出しにもしない
        // (無効だと thematic break / setext 見出しとして誤解析される)
        let source = "---\ntitle: 題名\n---\n\n本文です。\n";
        let (_doc, leaves, headings) = Doc::parse(PathBuf::from("test.md"), source.to_string());
        assert!(
            leaves.iter().all(|l| !l.text.contains("title")),
            "frontmatter がリーフに混入している: {leaves:?}"
        );
        assert!(
            headings.iter().all(|h| !h.title.contains("title")),
            "frontmatter が見出し化している: {headings:?}"
        );
    }

    #[test]
    fn frontmatter_literal_matches_source_prefix() {
        // 層別計測(#53): frontmatter の生テキストはソース先頭と一致し、
        // 残りが本文として切り出せる。comrak の literal は閉じ `---` 行の
        // 直後の空行 1 つまで含む(空白のみで計測への影響はない)
        let source = "---\ntitle: 題名\ndescription: 説明です。\n---\n\n本文です。\n";
        let (doc, _leaves, _headings) = Doc::parse(PathBuf::from("test.md"), source.to_string());
        let frontmatter = doc.frontmatter.as_deref().expect("frontmatter があるはず");
        assert_eq!(&source[..frontmatter.len()], frontmatter);
        assert!(frontmatter.starts_with("---\n"));
        assert!(frontmatter.trim_end().ends_with("---"));
        assert_eq!(&source[frontmatter.len()..], "本文です。\n");
        // ソース先頭の `---` 行から始まらないと frontmatter にならない(comrak 仕様)
        let plain = "# 見出し\n\n本文です。\n";
        let (doc, _l, _h) = Doc::parse(PathBuf::from("t.md"), plain.to_string());
        assert!(doc.frontmatter.is_none());
    }

    #[test]
    fn style_declaration_reads_only_root_level_key() {
        // #67: frontmatter 内の `jaio-style:` はルート mapping のキーのみ。
        // ブロックスカラーやネストした mapping の中身は宣言ではない
        let decl = |fm: &str| {
            let source = format!("---\n{fm}---\n\n本文です。\n");
            let (doc, _l, _h) = Doc::parse(PathBuf::from("t.md"), source);
            doc.style_declaration()
        };
        // description のブロックスカラー内は宣言でない(ルートの polite が使われる)
        assert_eq!(
            decl("description: |\n  jaio-style: plain\njaio-style: polite\n"),
            Some(StyleTarget::Polite)
        );
        // `>` の folded scalar も同様
        assert_eq!(decl("description: >\n  jaio-style: plain\n"), None);
        // ネストした mapping 内のキーは宣言でない
        assert_eq!(decl("example:\n  jaio-style: plain\n"), None);
        // ルートの宣言は従来どおり(クォート・行内コメントも許容)
        assert_eq!(decl("jaio-style: plain\n"), Some(StyleTarget::Plain));
        assert_eq!(decl("jaio-style: \"polite\"\n"), Some(StyleTarget::Polite));
        assert_eq!(decl("jaio-style: 'plain'\n"), Some(StyleTarget::Plain));
        assert_eq!(decl("jaio-style: plain # 備考\n"), Some(StyleTarget::Plain));
        // `:` 直後が空白でない並びは YAML ではスカラー文字列(宣言ではない)
        assert_eq!(decl("jaio-style:plain\n"), None);
        // 不正値・未指定は従来どおり None(既定・overrides へフォールバック)
        assert_eq!(decl("jaio-style: teinei\n"), None);
        assert_eq!(decl("title: 題名\n"), None);
    }

    #[test]
    fn style_declaration_rejects_malformed_yaml() {
        // #69: YAML として不正な入力への簡易抽出の寛容を狭める。タブ区切り・
        // 閉じていないクォート・キーの重複は宣言無効(他ツールの YAML
        // パーサの解釈と衝突しない安全側)
        let decl = |fm: &str| {
            let source = format!("---\n{fm}---\n\n本文です。\n");
            let (doc, _l, _h) = Doc::parse(PathBuf::from("t.md"), source);
            doc.style_declaration()
        };
        // タブは YAML の区切りに使えない(スカラー文字列扱いになり不正)
        assert_eq!(decl("jaio-style:\tplain\n"), None);
        // 閉じていないクォートは不正な値
        assert_eq!(decl("jaio-style: \"plain\n"), None);
        assert_eq!(decl("jaio-style: plain\"\n"), None);
        // キーの重複は YAML エラー。値の妥当性に関係なく宣言全体を無効にする
        assert_eq!(decl("jaio-style: plain\njaio-style: polite\n"), None);
        assert_eq!(decl("jaio-style: invalid\njaio-style: plain\n"), None);
        // 対で閉じた空クォートは不正値扱い(中身が空のため None のまま変わらない)
        assert_eq!(decl("jaio-style: \"\"\n"), None);
    }

    #[test]
    fn autolinked_urls_keep_scheme() {
        // bare URL / www. は autolink 拡張が http(s) を付けるため
        // is_internal_link 側で scheme 判定すれば外部リンクになる
        let source = "参照: www.example.com と https://example.com/x\n";
        let (doc, _leaves, _headings) = Doc::parse(PathBuf::from("test.md"), source.to_string());
        let urls: Vec<&str> = doc.links.iter().map(|l| l.url.as_str()).collect();
        assert!(
            urls.iter().all(|u| u.starts_with("http")),
            "autolink URL は scheme 付きのはず: {urls:?}"
        );
    }

    #[test]
    fn tracks_nest_depth_of_lists_and_quotes() {
        let source =
            "- 階層1\n  - 階層2\n    - 階層3\n      - 階層4\n\n> 引用です。\n\n>> 深い引用です。\n";
        let (_doc, leaves, _headings) = Doc::parse(PathBuf::from("test.md"), source.to_string());
        let depth_of = |text: &str| {
            leaves
                .iter()
                .find(|l| l.text == text)
                .unwrap_or_else(|| panic!("リーフが見つからない: {text}"))
                .nest_depth
        };
        assert_eq!(depth_of("階層1"), 1);
        assert_eq!(depth_of("階層4"), 4);
        assert_eq!(depth_of("引用です。"), 1);
        assert_eq!(depth_of("深い引用です。"), 2);
    }
}
