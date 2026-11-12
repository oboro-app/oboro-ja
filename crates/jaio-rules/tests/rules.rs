//! ルールの統合テスト。fixtures/sample.md に対して全ルールを適用する。

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use std::collections::HashSet;
use std::time::SystemTime;

use jaio_core::Severity;
use jaio_core::comments::CommentFile;
use jaio_core::config::{Config, DetectionConfig, MergeConfig};
use jaio_parse::doc::Doc;
use jaio_parse::morph::Analyzer;
use jaio_parse::tokencount::TokenCounter;
use jaio_rules::{Corpus, DocContext, default_corpus_rules, default_rules, normalize_path};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample.md")
}

fn run_all_rules() -> anyhow::Result<Vec<String>> {
    let path = fixture();
    let source = fs::read_to_string(&path)?;
    let (doc, leaves, headings) = Doc::parse(path, source);
    let mut analyzer = Analyzer::new()?;
    let counter = TokenCounter::new()?;
    // particle の報告ペアは opt-in(#23)。fixture 検証では例示として有効化する
    let detection = DetectionConfig {
        particle_pairs: vec![("に".to_string(), "へ".to_string())],
        ..DetectionConfig::default()
    };
    let mut cache = HashMap::new();

    let mut rule_ids = Vec::new();
    for rule in default_rules(&Config::default()) {
        let mut ctx = DocContext {
            doc: &doc,
            leaves: &leaves,
            headings: &headings,
            analyzer: &mut analyzer,
            cache: &mut cache,
            counter: &counter,
            detection: &detection,
        };
        for diagnostic in rule.run(&mut ctx)?.diagnostics {
            rule_ids.push(diagnostic.rule_id.to_string());
        }
    }
    Ok(rule_ids)
}

#[test]
fn detects_all_expected_rule_kinds() -> anyhow::Result<()> {
    let rule_ids = run_all_rules()?;
    let expected = [
        "structure/heading-skip",
        "structure/empty-section",
        "structure/over-nesting",
        "structure/section-duplication",
        "efficiency/redundant-expression",
        "consistency/style-deviation",
        "consistency/notation-variant",
        "consistency/particle",
    ];
    for id in expected {
        assert!(
            rule_ids.iter().any(|found| found == id),
            "期待した診断が出ない: {id}(実際の検出: {rule_ids:?})"
        );
    }
    Ok(())
}

/// インラインの Markdown に全ルールを適用して、診断を返す。
fn diagnostics_on(source: &str) -> anyhow::Result<Vec<jaio_core::Diagnostic>> {
    diagnostics_on_with(source, DetectionConfig::default())
}

/// 検出設定を明示する中核ヘルパー(particle ペアの opt-in 検証に使う)
fn diagnostics_on_with(
    source: &str,
    detection: DetectionConfig,
) -> anyhow::Result<Vec<jaio_core::Diagnostic>> {
    let (doc, leaves, headings) = Doc::parse(PathBuf::from("inline.md"), source.to_string());
    let mut analyzer = Analyzer::new()?;
    let counter = TokenCounter::new()?;
    let mut cache = HashMap::new();
    let mut found = Vec::new();
    for rule in default_rules(&Config::default()) {
        let mut ctx = DocContext {
            doc: &doc,
            leaves: &leaves,
            headings: &headings,
            analyzer: &mut analyzer,
            cache: &mut cache,
            counter: &counter,
            detection: &detection,
        };
        for diagnostic in rule.run(&mut ctx)?.diagnostics {
            found.push(diagnostic);
        }
    }
    Ok(found)
}

/// インラインの Markdown に全ルールを適用して、検出したルール ID を返す。
fn rule_ids_on(source: &str) -> anyhow::Result<Vec<String>> {
    Ok(diagnostics_on(source)?
        .into_iter()
        .map(|d| d.rule_id.to_string())
        .collect())
}

#[test]
fn pattern_diagnostics_point_at_source_position() -> anyhow::Result<()> {
    // リスト項目のマーカ分・複数行段落の 2 行目以降も実位置で報告する(Issue #9)
    let source = "# 題名\n\n- 前置きの文です。使用することができます。\n\n一行目の本文です。\n二行目で利用することができます。\n";
    let diags = diagnostics_on(source)?;
    let redundant: Vec<_> = diags
        .iter()
        .filter(|d| d.rule_id == "efficiency/redundant-expression")
        .collect();
    assert_eq!(redundant.len(), 2, "{diags:?}");
    // "- 前置きの文です。" の後ろ: "- "(2 文字) + "前置きの文です。"(8 文字) => col 11
    assert_eq!((redundant[0].span.line, redundant[0].span.col), (3, 11));
    // 段落 2 行目の "二行目で"(4 文字) の後ろ => col 5
    assert_eq!((redundant[1].span.line, redundant[1].span.col), (6, 5));
    Ok(())
}

#[test]
fn locate_positions_stay_correct_deep_in_long_document() -> anyhow::Result<()> {
    // Issue #30: 行オフセット索引でリーフ行範囲を切り出す経路の回帰。
    // 多数行の文書でも末尾付近のリーフ位置が正しいこと
    let mut source = String::new();
    for i in 0..2000 {
        source.push_str(&format!("段落{i}の本文です。\n\n"));
    }
    source.push_str("最後の段落で使用することができます。\n");
    let diags = diagnostics_on(&source)?;
    let redundant: Vec<_> = diags
        .iter()
        .filter(|d| d.rule_id == "efficiency/redundant-expression")
        .collect();
    assert_eq!(redundant.len(), 1, "{diags:?}");
    // 2000 段落 x 2 行 = 4000 行の後、最終段落は 4001 行目
    assert_eq!(redundant[0].span.line, 4001);
    Ok(())
}

#[test]
fn redundant_expression_ignores_match_inside_link_destination() -> anyhow::Result<()> {
    // リンク URL(`](...)`)内の出現は leaf.text に含まれない。生行にだけ
    // ある出現で出現番号がずれ、列が URL 内を指さないことの確認
    let source = "[詳細](https://example.com/使用することができます)を見て、設定で使用することができます。\n";
    let diags = diagnostics_on(source)?;
    let redundant: Vec<_> = diags
        .iter()
        .filter(|d| d.rule_id == "efficiency/redundant-expression")
        .collect();
    assert_eq!(redundant.len(), 1, "{diags:?}");
    // パターンの先頭「使」の列を指すはず(「設定で」の 3 文字後)
    let expected = source[..source.find("設定で使用することができます").unwrap()]
        .chars()
        .count()
        + "設定で".chars().count()
        + 1;
    assert_eq!(
        (redundant[0].span.line, redundant[0].span.col),
        (1, expected),
        "URL 内の出現ではなく本文側の実位置を指すはず"
    );
    Ok(())
}

#[test]
fn redundant_expression_locates_match_inside_autolink() -> anyhow::Result<()> {
    // `<scheme:...>` 形式の autolink は comrak が URL をリンクテキストとして
    // leaf.text に含めるため、autolink 内の出現も実位置で報告される
    let source =
        "<https://example.com/使用することができます> を見て、設定で使用することができます。\n";
    let diags = diagnostics_on(source)?;
    let redundant: Vec<_> = diags
        .iter()
        .filter(|d| d.rule_id == "efficiency/redundant-expression")
        .collect();
    assert_eq!(redundant.len(), 2, "{diags:?}");
    let autolink_col = source.find("使用することができます").unwrap();
    let body_col = source.rfind("使用することができます").unwrap();
    assert_eq!(
        (redundant[0].span.line, redundant[0].span.col),
        (1, source[..autolink_col].chars().count() + 1),
        "autolink 内の出現位置を指すはず"
    );
    assert_eq!(
        (redundant[1].span.line, redundant[1].span.col),
        (1, source[..body_col].chars().count() + 1),
        "本文側の出現位置を指すはず"
    );
    Ok(())
}

#[test]
fn redundant_expression_ignores_reference_definition_line() -> anyhow::Result<()> {
    // 段落ブロック先頭の `[label]: dest` 参照定義行は comrak が leaf.text
    // から取り除くが行範囲には残る。定義行内の出現で列がずれないこと
    let source =
        "[d]: https://example.com/使用することができます\n設定で使用することができます。\n";
    let diags = diagnostics_on(source)?;
    let redundant: Vec<_> = diags
        .iter()
        .filter(|d| d.rule_id == "efficiency/redundant-expression")
        .collect();
    assert_eq!(redundant.len(), 1, "{diags:?}");
    assert_eq!(
        (redundant[0].span.line, redundant[0].span.col),
        (2, 4),
        "参照定義行の出現ではなく本文側の実位置を指すはず: {diags:?}"
    );
    Ok(())
}

#[test]
fn terminology_diagnostic_points_at_other_form_position() -> anyhow::Result<()> {
    // 別表記の実位置を指す。推奨表記「サーバー」に含まれる「サーバ」を
    // 拾って 1 行目を指さないことの確認(Issue #9)
    let source = "サーバーに接続します。\n\n次にサーバの設定を参照します。\n";
    let diags = diagnostics_on(source)?;
    let term: Vec<_> = diags
        .iter()
        .filter(|d| d.rule_id == "consistency/terminology")
        .collect();
    assert_eq!(term.len(), 1, "{diags:?}");
    // 「次に」(2 文字) の後ろ => col 3
    assert_eq!((term[0].span.line, term[0].span.col), (3, 3));
    Ok(())
}

#[test]
fn detects_over_nesting() -> anyhow::Result<()> {
    let source = "- 階層1\n  - 階層2\n    - 階層3\n      - 階層4\n";
    let found = rule_ids_on(source)?;
    assert!(
        found.iter().any(|id| id == "structure/over-nesting"),
        "深いネストを検出するはず: {found:?}"
    );
    Ok(())
}

#[test]
fn shallow_nesting_is_not_flagged() -> anyhow::Result<()> {
    let source = "- 階層1\n  - 階層2\n    - 階層3\n";
    let found = rule_ids_on(source)?;
    assert!(
        !found.iter().any(|id| id == "structure/over-nesting"),
        "3 段階までのネストは報告しないはず: {found:?}"
    );
    Ok(())
}

#[test]
fn crlf_line_break_does_not_shift_column() -> anyhow::Result<()> {
    // CRLF の行末 \r を列計算から除外する(列補正 #44 の回帰、#110)。
    // 3 行目冒頭のパターンが col 1 を指すこと(\r が混ざるとずれる)
    let source = "# 題名\r\n\r\n使用することができます。\r\n";
    let diags = diagnostics_on(source)?;
    let diag = diags
        .iter()
        .find(|d| d.rule_id == "efficiency/redundant-expression")
        .expect("冗長表現の診断が出るはず");
    assert_eq!((diag.span.line, diag.span.col), (3, 1), "{:?}", diag.span);
    Ok(())
}

#[test]
fn detects_punctuation_mixing() -> anyhow::Result<()> {
    // 和文の読点 `、` と全角カンマ `，` の混在を検出する(段落を分けて別リーフにする)
    let mixed = "設定は起動時に読み込まれます、変更は再起動後に反映されます。\n\n検証は直後に行われます，確認も同時に済ませます。\n";
    let found = rule_ids_on(mixed)?;
    assert!(
        found.iter().any(|id| id == "consistency/punctuation"),
        "読点の混在を検出するはず: {found:?}"
    );
    // 和文だけの一貫した使用は報告しない
    let consistent =
        "設定は起動時に読み込まれます、変更は再起動後に反映されます、検証は直後に行われます。\n";
    let found = rule_ids_on(consistent)?;
    assert!(
        !found.iter().any(|id| id == "consistency/punctuation"),
        "一貫した表記は報告しないはず: {found:?}"
    );
    Ok(())
}

#[test]
fn ascii_punctuation_span_is_not_pulled_by_out_of_context_occurrence() -> anyhow::Result<()> {
    // 和文文脈の ASCII 句読点の位置特定(#101)。同一の 2 文字(直前文字 +
    // ASCII)が文脈外(ファイル名の `.`)に先出現していると、2 文字の
    // needle では先出現側を誤って指していた。直後文字を含む 3 文字の
    // needle で和文文脈側を指すことを確認する
    let source = "# 題名\n\n実行する.md と実行する. 次へ進む。\n";
    let diags = diagnostics_on(source)?;
    let punct: Vec<_> = diags
        .iter()
        .filter(|d| d.rule_id == "consistency/punctuation")
        .collect();
    assert_eq!(punct.len(), 1, "{diags:?}");
    // 「実行する. 」の ASCII 句点 => 3 行目 col 14(文脈外の `実行する.md` の
    // col 5 でないこと)。span は needle(直前文字)基準 + 1 で ASCII を指す
    assert_eq!(
        (punct[0].span.line, punct[0].span.col),
        (3, 14),
        "和文文脈側の ASCII 句点を指すはず: {:?}",
        punct[0].span
    );
    Ok(())
}

#[test]
fn detects_ascii_punctuation_in_ja_context() -> anyhow::Result<()> {
    // 和文文脈の ASCII 句読点は混在の第 3 表記として検出する(Issue #4)
    let source = "# 題名\n\n設定を読み込みます。次に検証します.\n\n最後に保存します。バージョンは v1.2 です。\n";
    let diags = diagnostics_on(source)?;
    let punct: Vec<_> = diags
        .iter()
        .filter(|d| d.rule_id == "consistency/punctuation")
        .collect();
    assert_eq!(punct.len(), 1, "{diags:?}");
    assert!(
        punct[0].observation.contains("「.」"),
        "ASCII 形が少数派として報告されるはず: {:?}",
        punct[0].observation
    );
    // 3 行目の「検証します」直後の `.` => col 18
    assert_eq!((punct[0].span.line, punct[0].span.col), (3, 18));

    // 英語・数値の ASCII 句読点は和文文脈ではないため無視する
    let source = "# 題名\n\nHello, world. 数値は 1,000 です。\n\n次の文です。\n";
    let diags = diagnostics_on(source)?;
    assert!(
        !diags.iter().any(|d| d.rule_id == "consistency/punctuation"),
        "英語・数値の句読点は報告しないはず: {diags:?}"
    );
    Ok(())
}

#[test]
fn detects_low_density_paragraph() -> anyhow::Result<()> {
    // コンテンツ語の多い段落 3 つ + 機能語だらけの段落 1 つ。
    // 4 段落目は「まあ」「一応」などの接続・フィラー中心でコンテンツ語がほぼない。
    let dense = "静的解析エンジンは構文木と形態素列を入力として診断規則を適用し検出結果を出力します。各規則は文書構造と品詞情報を参照します。";
    let thin = "まあこれはですね、一応そういう感じになっていまして、ええとそのあたりはいろいろあるんですけれども、とりあえずそんな感じです。";
    // 外れ値検出はサンプル数が少ないと分散が外れ値自身に引っ張られるため、
    // 基準側の段落を多めに置く
    let dense_paras = (1..=5)
        .map(|i| format!("{dense}その{i}。"))
        .collect::<Vec<_>>()
        .join("\n\n");
    let source = format!("{dense_paras}\n\n{thin}\n");
    let found = rule_ids_on(&source)?;
    assert!(
        found.iter().any(|id| id == "efficiency/token-density"),
        "低密度の段落を検出するはず: {found:?}"
    );
    // 均質な文書は報告しない
    let uniform = format!("{dense_paras}\n");
    let found = rule_ids_on(&uniform)?;
    assert!(
        !found.iter().any(|id| id == "efficiency/token-density"),
        "均一な密度なら報告しないはず: {found:?}"
    );
    Ok(())
}

#[test]
fn detects_duplicate_sections() -> anyhow::Result<()> {
    let body = "このセクションでは設定ファイルの読み込み処理について説明します。設定ファイルは起動時に一度だけ読み込まれます。";
    let source = format!("## 設定の読み込み\n\n{body}\n\n## 設定の読み込み(詳細)\n\n{body}\n");
    let found = rule_ids_on(&source)?;
    assert!(
        found.iter().any(|id| id == "structure/section-duplication"),
        "同一内容のセクションを検出するはず: {found:?}"
    );
    Ok(())
}

#[test]
fn distinct_sections_are_not_flagged() -> anyhow::Result<()> {
    let source = "## 設定の読み込み\n\nこのセクションでは設定ファイルの読み込み処理について説明します。設定ファイルは起動時に一度だけ読み込まれます。\n\n## 終了コード\n\nこのセクションではコマンドの終了コードと出力形式について説明します。終了コードは診断の重大度で決まります。\n";
    let found = rule_ids_on(source)?;
    assert!(
        !found.iter().any(|id| id == "structure/section-duplication"),
        "内容の異なるセクションは報告しないはず: {found:?}"
    );
    Ok(())
}

#[test]
fn list_dominated_document_skips_prose_rules() -> anyhow::Result<()> {
    // リスト主導の文書(課題管理ファイル等)では同一文言の反復が構造的なため、
    // 散文前提ルール(repeated-quote / token-density)は適用しない
    let sentence = "この項目は検証待ちのため対応方針の確定後に着手します。";
    let mut source = String::from("# 課題一覧\n\n");
    for i in 0..12 {
        source.push_str(&format!("- 課題{i}: {sentence}\n"));
    }
    let found = rule_ids_on(&source)?;
    assert!(
        !found.iter().any(|id| id == "efficiency/repeated-quote"),
        "リスト主導文書の反復は報告しないはず: {found:?}"
    );
    Ok(())
}

#[test]
fn detects_long_sentence_and_no_comma() -> anyhow::Result<()> {
    // 和文 100 文字超の読点なし文は long-sentence と no-comma の両方に出る
    let long = "あ".repeat(105);
    let source = format!("# 題名\n\n{long}。\n");
    let diags = diagnostics_on(&source)?;
    assert!(
        diags
            .iter()
            .any(|d| d.rule_id == "efficiency/long-sentence"),
        "{diags:?}"
    );
    assert!(
        diags.iter().any(|d| d.rule_id == "efficiency/no-comma"),
        "{diags:?}"
    );
    // 読点を含む長文(和文 110 字超)は long-sentence のみ
    let mut with_pause = "あ".repeat(60);
    with_pause.push('、');
    with_pause.push_str(&"い".repeat(50));
    let source2 = format!("# 題名\n\n{with_pause}。\n");
    let diags2 = diagnostics_on(&source2)?;
    assert!(
        diags2
            .iter()
            .any(|d| d.rule_id == "efficiency/long-sentence"),
        "{diags2:?}"
    );
    assert!(
        diags2.iter().all(|d| d.rule_id != "efficiency/no-comma"),
        "{diags2:?}"
    );
    // 和文 50 字の文はどちらにも出ない
    let short = "あ".repeat(50);
    let source3 = format!("# 題名\n\n{short}。\n");
    let diags3 = diagnostics_on(&source3)?;
    assert!(
        diags3
            .iter()
            .all(|d| d.rule_id != "efficiency/long-sentence"),
        "{diags3:?}"
    );
    assert!(
        diags3.iter().all(|d| d.rule_id != "efficiency/no-comma"),
        "{diags3:?}"
    );
    Ok(())
}

#[test]
fn long_sentence_ignores_ascii_runs_and_threshold_is_configurable() -> anyhow::Result<()> {
    // ASCII(英字・URL・コード)は読み負荷が軽いため長さに数えない
    // (#48: 英字列挙による誤検出の実測)
    let source = format!(
        "# 題名\n\n{} と {} を使います。\n",
        "jaio".repeat(40),
        "https://example.com/a/very/long/path/segment"
    );
    let diags = diagnostics_on(&source)?;
    assert!(
        diags
            .iter()
            .all(|d| d.rule_id != "efficiency/long-sentence"),
        "{diags:?}"
    );
    // 閾値は [detection] で調整できる
    let detection = DetectionConfig {
        long_sentence_min_chars: 10,
        no_comma_min_chars: 10,
        ..DetectionConfig::default()
    };
    let source2 = "# 題名\n\nこれは閾値を下げた対象になる文です。\n";
    let diags2 = diagnostics_on_with(source2, detection)?;
    assert!(
        diags2
            .iter()
            .any(|d| d.rule_id == "efficiency/long-sentence"),
        "{diags2:?}"
    );
    Ok(())
}

#[test]
fn sentence_rules_point_at_source_position() -> anyhow::Result<()> {
    // 文単位ルールの報告位置はリーフ先頭ではなく文の実位置(リストマーカ分を含む)
    let detection = DetectionConfig {
        long_sentence_min_chars: 10,
        no_comma_min_chars: 10,
        ..DetectionConfig::default()
    };
    let source =
        "# 題名\n\n- 短い文です。\n- これは読点のない長い文になりますので検出対象になります。\n";
    let diags = diagnostics_on_with(source, detection)?;
    let diag = diags
        .iter()
        .find(|d| d.rule_id == "efficiency/no-comma")
        .expect("no-comma が出るはず");
    assert_eq!(diag.span.line, 4, "{diags:?}");
    // "- "(2 文字)の後の文頭
    assert_eq!(diag.span.col, 3, "{diags:?}");
    // observation に文のスニペットが含まれる(どの文か一目で分かる、#59)
    assert!(
        diag.observation.contains("これは読点のない"),
        "{}",
        diag.observation
    );
    Ok(())
}

#[test]
fn enumeration_middle_dot_counts_as_pause() -> anyhow::Result<()> {
    // 列挙中黒「・」(和文文字に後続)は区切りとして数え、no-comma の
    // 対象外になる(#60)。ASCII に後続する中黒(英字列挙)は数えない
    let detection = DetectionConfig {
        no_comma_min_chars: 10,
        ..DetectionConfig::default()
    };
    let enumerated =
        "# 題名\n\n出典ラベルは出典・ヒアリング・推定の 3 種類から選んで報告書に記載します。\n";
    let diags = diagnostics_on_with(enumerated, detection.clone())?;
    assert!(
        diags.iter().all(|d| d.rule_id != "efficiency/no-comma"),
        "和文中黒列挙は no-comma の対象外のはず: {:?}",
        diags
            .iter()
            .map(|d| d.observation.clone())
            .collect::<Vec<_>>()
    );
    // ASCII 列挙(foo・bar)の中黒は区切りとしない(和文文字に後続しないため)
    let ascii_enum =
        "# 題名\n\nfoo・bar・baz の中黒は英字列挙の区切りなので読点には数えない結果になります。\n";
    let diags = diagnostics_on_with(ascii_enum, detection)?;
    assert!(
        diags.iter().any(|d| d.rule_id == "efficiency/no-comma"),
        "ASCII 列挙の中黒だけでは pause にならないはず: {:?}",
        diags
            .iter()
            .map(|d| d.observation.clone())
            .collect::<Vec<_>>()
    );
    Ok(())
}

#[test]
fn prose_document_still_reports_repeated_quote() -> anyhow::Result<()> {
    // 散文では同一文の重複を引き続き検出する(ガードが過剰でないことの確認)
    let sentence = "このセクションでは設定ファイルの読み込み処理とその運用について説明します。";
    let source = format!("# 概要\n\n{sentence}\n\n{sentence}\n\n{sentence}\n");
    let found = rule_ids_on(&source)?;
    assert!(
        found.iter().any(|id| id == "efficiency/repeated-quote"),
        "散文の同一文重複は検出するはず: {found:?}"
    );
    Ok(())
}

#[test]
fn detects_kanji_kana_notation_variant() -> anyhow::Result<()> {
    // かな漢字揺れ(したがって/従って)は原形(従う)が一致するため真の揺れとして検出する。
    // 旧実装では共通文字が送り仮名 1 字のため取りこぼしていた(実運用試用の課題)
    let source =
        "従って、次の工程へ進みます。したがって、出力を確認します。従って、完了を記録します。\n";
    let found = rule_ids_on(source)?;
    assert!(
        found.iter().any(|id| id == "consistency/notation-variant"),
        "かな漢字揺れを検出するはず: {found:?}"
    );
    Ok(())
}

#[test]
fn homophone_different_words_are_not_reported() -> anyhow::Result<()> {
    // 返る/変える は同音(カエル)だが原形が異なる別語。共通文字 1 字のため報告しない
    let source =
        "画面に返る値を記録します。設定を変える値を比較します。画面に返る値を出力します。\n";
    let found = rule_ids_on(source)?;
    assert!(
        !found.iter().any(|id| id == "consistency/notation-variant"),
        "同音異義の別語は報告しないはず: {found:?}"
    );
    Ok(())
}

/// particle ペアを opt-in した検出設定
fn particle_enabled() -> DetectionConfig {
    DetectionConfig {
        particle_pairs: vec![("に".to_string(), "へ".to_string())],
        ..DetectionConfig::default()
    }
}

#[test]
fn detects_particle_variation() -> anyhow::Result<()> {
    // 報告対象ペアは opt-in(に/へ)。多数派 2 以上・頻度比 3 以内で報告する
    let source = "設定をサーバーに送信する。ログをサーバーへ送信する。結果をサーバーに送信する。\n";
    let found: Vec<String> = diagnostics_on_with(source, particle_enabled())?
        .into_iter()
        .map(|d| d.rule_id.to_string())
        .collect();
    assert!(
        found.iter().any(|id| id == "consistency/particle"),
        "同一述語の格助詞揺れを検出するはず: {found:?}"
    );
    Ok(())
}

#[test]
fn particle_is_opt_in_and_silent_by_default() -> anyhow::Result<()> {
    // 既定では報告ペアが空のため、同じ構文でも particle は報告しない(#23)
    let source = "設定をサーバーに送信する。ログをサーバーへ送信する。結果をサーバーに送信する。\n";
    let found = rule_ids_on(source)?;
    assert!(
        !found.iter().any(|id| id == "consistency/particle"),
        "既定(ペア空)では報告しないはず: {found:?}"
    );
    Ok(())
}

#[test]
fn non_reportable_particle_pair_is_ignored() -> anyhow::Result<()> {
    // を/に は正当な格の使い分け(対象・到達点)が多く、報告対象外
    let source = "ファイルを解析する。最後に結果に解析する。\n";
    let found = rule_ids_on(source)?;
    assert!(
        !found.iter().any(|id| id == "consistency/particle"),
        "報告対象外の助詞ペアは出ないはず: {found:?}"
    );
    Ok(())
}

#[test]
fn subject_object_particle_pair_is_ignored() -> anyhow::Result<()> {
    // を/が は主格と対格の正当な使い分け(「列車が通過する」「ポイントを通過する」)で
    // 実運用でも揺れらしい例が出なかったため、報告対象外とする
    let source = "列車が通過する。ポイントを通過する。ゲートを通過する。\n";
    let found = rule_ids_on(source)?;
    assert!(
        !found.iter().any(|id| id == "consistency/particle"),
        "主格・対格の使い分けは報告しないはず: {found:?}"
    );
    Ok(())
}

#[test]
fn homophone_different_words_are_not_variants() -> anyhow::Result<()> {
    // 読みが同じでも別の語(返る/変える)は表記ゆれとして報告しない
    let source = "設定を元に戻し、値を変える。状態が返る。設定に返る。\n";
    let found = rule_ids_on(source)?;
    assert!(
        !found.iter().any(|id| id == "consistency/notation-variant"),
        "同音異義の別語は表記ゆれにしないはず: {found:?}"
    );
    Ok(())
}

#[test]
fn okurigana_variants_are_detected() -> anyhow::Result<()> {
    // 送り仮名の揺れ(行う/行なう)は表記ゆれとして検出する
    let source = "解析を行う。出力を行う。確認を行なう。\n";
    let found = rule_ids_on(source)?;
    assert!(
        found.iter().any(|id| id == "consistency/notation-variant"),
        "送り仮名の揺れを検出するはず: {found:?}"
    );
    Ok(())
}

#[test]
fn clean_document_produces_no_style_or_expression_diags() -> anyhow::Result<()> {
    // 敬体で統一し、冗長表現を含まない文書は style-deviation / redundant-expression を出さない
    let source = "# 題名\n\n本文です。\n\n解析します。\n";
    let (doc, leaves, headings) = Doc::parse(PathBuf::from("clean.md"), source.to_string());
    let mut analyzer = Analyzer::new()?;
    let counter = TokenCounter::new()?;
    let detection = DetectionConfig::default();
    let mut cache = HashMap::new();

    let mut found = Vec::new();
    for rule in default_rules(&Config::default()) {
        let mut ctx = DocContext {
            doc: &doc,
            leaves: &leaves,
            headings: &headings,
            analyzer: &mut analyzer,
            cache: &mut cache,
            counter: &counter,
            detection: &detection,
        };
        for diagnostic in rule.run(&mut ctx)?.diagnostics {
            found.push(diagnostic.rule_id.to_string());
        }
    }
    assert!(
        !found.iter().any(|id| id == "consistency/style-deviation"),
        "敬体統一文書で style-deviation は出ないはず: {found:?}"
    );
    assert!(
        !found
            .iter()
            .any(|id| id == "efficiency/redundant-expression"),
        "冗長表現を含まない文書で redundant-expression は出ないはず: {found:?}"
    );
    Ok(())
}

/// style-deviation ルールの診断重大度一覧を返す。
fn style_severities_on(source: &str) -> anyhow::Result<Vec<Severity>> {
    let (doc, leaves, headings) = Doc::parse(PathBuf::from("inline.md"), source.to_string());
    let mut analyzer = Analyzer::new()?;
    let counter = TokenCounter::new()?;
    let detection = DetectionConfig::default();
    let mut cache = HashMap::new();
    let mut severities = Vec::new();
    for rule in default_rules(&Config::default()) {
        if rule.id() != "consistency/style-deviation" {
            continue;
        }
        let mut ctx = DocContext {
            doc: &doc,
            leaves: &leaves,
            headings: &headings,
            analyzer: &mut analyzer,
            cache: &mut cache,
            counter: &counter,
            detection: &detection,
        };
        for diagnostic in rule.run(&mut ctx)?.diagnostics {
            severities.push(diagnostic.severity);
        }
    }
    Ok(severities)
}

#[test]
fn polite_dominant_document_reports_deviation_warn() -> anyhow::Result<()> {
    // 敬体が優勢の文書に混入した常体文は逸脱(warn)として報告する
    let severities = style_severities_on("# 題名\n\n解析します。\n整形します。\n出力する。\n")?;
    assert_eq!(severities, vec![Severity::Warn]);
    Ok(())
}

#[test]
fn plain_dominant_document_gets_doc_level_info() -> anyhow::Result<()> {
    // 常体が優勢の文書は逸脱(warn)ではなく、文書レベルの info で報告する
    let severities = style_severities_on("# 題名\n\n解析する。\n整形する。\n出力する。\n")?;
    assert_eq!(severities, vec![Severity::Info]);
    Ok(())
}

#[test]
fn polite_minority_does_not_cause_false_deviation() -> anyhow::Result<()> {
    // 常体 4 文 + 敬体 1 文の文書で、多数の常体文を「逸脱」として報告しない(逆転誤報の回帰テスト)
    let severities =
        style_severities_on("# 題名\n\n解析する。\n整形する。\n出力する。\n確認します。\n")?;
    assert_eq!(
        severities,
        vec![Severity::Info],
        "敬体少数の常体文書は info のみのはず"
    );
    Ok(())
}

#[test]
fn tiny_document_is_not_judged() -> anyhow::Result<()> {
    // 判定に足る文数がない小さな文書は報告しない
    let severities = style_severities_on("# 題名\n\n解析する。\n")?;
    assert!(severities.is_empty());
    Ok(())
}

#[test]
fn detects_broken_links_and_orphans() -> anyhow::Result<()> {
    // corpus fixture: a → b(正常)、a → missing.md(リンク切れ)、orphan は無参照
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/corpus");
    let mut sources = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "md") {
            let source = fs::read_to_string(&path)?;
            let (doc, _, _) = Doc::parse(path, source);
            sources.push(doc);
        }
    }
    assert_eq!(sources.len(), 3, "fixture は 3 ファイルのはず");

    // 本文が空(重複検出対象外)なので relation の broken-link / orphan だけを検証する
    let bodies: Vec<String> = vec![String::new(); sources.len()];
    let mtimes = vec![SystemTime::UNIX_EPOCH; sources.len()];
    let corpus = Corpus::new(
        &sources,
        &bodies,
        &[],
        &mtimes,
        MergeConfig::default(),
        DetectionConfig::default(),
        HashSet::new(),
        &[],
        &[],
    )?;
    let mut found = Vec::new();
    for rule in default_corpus_rules(&Config::default(), &[]) {
        for diagnostic in rule.run(&corpus)?.diagnostics {
            found.push((
                diagnostic.rule_id.to_string(),
                diagnostic.observation.clone(),
            ));
        }
    }
    assert!(
        found
            .iter()
            .any(|(id, msg)| id == "relation/broken-link" && msg.contains("missing.md")),
        "リンク切れを検出するはず: {found:?}"
    );
    assert!(
        found
            .iter()
            .filter(|(id, _)| id == "relation/orphan")
            .count()
            == 1,
        "孤立文書は orphan.md の 1 件のはず: {found:?}"
    );
    let orphan_messages: Vec<&String> = found
        .iter()
        .filter(|(id, _)| id == "relation/orphan")
        .map(|(_, m)| m)
        .collect();
    assert!(
        orphan_messages.iter().any(|m| m.contains("orphan.md")),
        "孤立文書は orphan.md を指すはず: {orphan_messages:?}"
    );
    Ok(())
}

#[test]
fn query_in_link_url_does_not_break_resolution() -> anyhow::Result<()> {
    // #68: 相対 URL の query・fragment はファイル解決から除外する。
    // README からの参照が query 付きでも guide.md に解決され、
    // broken-link / orphan を誤判定しない。不存在リンクは従来どおり検出する
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/query-links");
    let mut sources = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "md") {
            let source = fs::read_to_string(&path)?;
            let (doc, _, _) = Doc::parse(path, source);
            sources.push(doc);
        }
    }
    assert_eq!(sources.len(), 2, "fixture は 2 ファイルのはず");

    let bodies: Vec<String> = vec![String::new(); sources.len()];
    let mtimes = vec![SystemTime::UNIX_EPOCH; sources.len()];
    let corpus = Corpus::new(
        &sources,
        &bodies,
        &[],
        &mtimes,
        MergeConfig::default(),
        DetectionConfig::default(),
        HashSet::new(),
        &[],
        &[],
    )?;
    let mut found = Vec::new();
    for rule in default_corpus_rules(&Config::default(), &[]) {
        for diagnostic in rule.run(&corpus)?.diagnostics {
            found.push((
                diagnostic.rule_id.to_string(),
                diagnostic.observation.clone(),
            ));
        }
    }
    // query 付きで参照された実在ファイルは broken-link にならない。
    // 不存在の lost.md?p=1 は引き続き検出される
    let broken: Vec<&String> = found
        .iter()
        .filter(|(id, _)| id == "relation/broken-link")
        .map(|(_, m)| m)
        .collect();
    assert!(
        broken.iter().all(|m| !m.contains("guide.md")),
        "guide.md への query 付きリンクは解決されるはず: {broken:?}"
    );
    assert!(
        broken.iter().any(|m| m.contains("lost.md")),
        "不存在リンクは検出されるはず: {broken:?}"
    );
    // README(エントリポイント)以外の唯一の文書 guide.md は参照済みのため
    // orphan は出ない
    assert!(
        found.iter().all(|(id, _)| id != "relation/orphan"),
        "query 付きリンク経由の参照で orphan を誤判定しないはず: {found:?}"
    );
    Ok(())
}

#[test]
fn github_template_links_resolve_from_repo_root() -> anyhow::Result<()> {
    // #84: .github の PR テンプレートの相対リンクは GitHub と同じ repo ルート
    // 基準で解決する。実在先(docs/x.md)は報告せず、不存在(docs/missing.md)は
    // 従来どおり検出する
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/github-links");
    // fixture は .github/ と docs/ のサブディレクトリに置いている(read_dir の
    // 直下列挙ではなく直接指定)
    let files = [
        dir.join(".github/pull_request_template.md"),
        dir.join("docs/x.md"),
    ];
    let mut sources = Vec::new();
    for path in files {
        let source = fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("fixture の読み込みに失敗: {}: {e}", path.display()))?;
        let (doc, _, _) = Doc::parse(path, source);
        sources.push(doc);
    }
    assert_eq!(sources.len(), 2, "fixture は 2 ファイルのはず");
    let bodies: Vec<String> = vec![String::new(); sources.len()];
    let mtimes = vec![SystemTime::UNIX_EPOCH; sources.len()];
    let corpus = Corpus::new(
        &sources,
        &bodies,
        &[],
        &mtimes,
        MergeConfig::default(),
        DetectionConfig::default(),
        HashSet::new(),
        &[],
        &[],
    )?;
    let mut broken: Vec<String> = Vec::new();
    for rule in default_corpus_rules(&Config::default(), &[]) {
        if rule.id() != "relation/broken-link" {
            continue;
        }
        for d in rule.run(&corpus)?.diagnostics {
            broken.push(d.observation);
        }
    }
    assert!(
        broken.iter().any(|m| m.contains("missing.md")),
        "不存在リンクは検出するはず: {broken:?}"
    );
    assert!(
        broken.iter().all(|m| !m.contains("docs/x.md")),
        "repo ルート基準で実在する docs/x.md は報告しないはず: {broken:?}"
    );
    Ok(())
}

#[test]
fn query_only_links_are_not_broken_links() -> anyhow::Result<()> {
    // #68: query-only リンク("?view=full")は現行文書内の表示切替参照。
    // アンカーのみ(#setup)と同じく解決対象外で、broken-link にしない
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/query-only");
    let mut sources = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "md") {
            let source = fs::read_to_string(&path)?;
            let (doc, _, _) = Doc::parse(path, source);
            sources.push(doc);
        }
    }
    let bodies: Vec<String> = vec![String::new(); sources.len()];
    let mtimes = vec![SystemTime::UNIX_EPOCH; sources.len()];
    let corpus = Corpus::new(
        &sources,
        &bodies,
        &[],
        &mtimes,
        MergeConfig::default(),
        DetectionConfig::default(),
        HashSet::new(),
        &[],
        &[],
    )?;
    let mut found = Vec::new();
    for rule in default_corpus_rules(&Config::default(), &[]) {
        for d in rule.run(&corpus)?.diagnostics {
            found.push(d.observation);
        }
    }
    assert!(
        found.iter().all(|m| !m.contains("view=full")),
        "query-only リンクは broken-link にしないはず: {found:?}"
    );
    Ok(())
}

#[test]
fn image_links_are_verified_but_not_counted_as_inbound() -> anyhow::Result<()> {
    // #74: 画像 ![alt](url) は broken-link の実在検証対象に含める(拡張子
    // 補完はしない)。一方で被リンク集計(orphan)には数えない。fixture の
    // a.md は fig.png(実在)・missing.png(不在)・b.md(画像記法での文書参照)
    // の 3 画像を含む
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/image-links");
    let mut sources = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "md") {
            let source = fs::read_to_string(&path)?;
            let (doc, _, _) = Doc::parse(path, source);
            sources.push(doc);
        }
    }
    assert_eq!(sources.len(), 2, "fixture は 2 ファイルのはず");

    let bodies: Vec<String> = vec![String::new(); sources.len()];
    let mtimes = vec![SystemTime::UNIX_EPOCH; sources.len()];
    let corpus = Corpus::new(
        &sources,
        &bodies,
        &[],
        &mtimes,
        MergeConfig::default(),
        DetectionConfig::default(),
        HashSet::new(),
        &[],
        &[],
    )?;
    let mut found = Vec::new();
    for rule in default_corpus_rules(&Config::default(), &[]) {
        for diagnostic in rule.run(&corpus)?.diagnostics {
            found.push((
                diagnostic.rule_id.to_string(),
                diagnostic.observation.clone(),
            ));
        }
    }
    let broken: Vec<&String> = found
        .iter()
        .filter(|(id, _)| id == "relation/broken-link")
        .map(|(_, m)| m)
        .collect();
    // 存在しない画像は報告する
    assert!(
        broken.iter().any(|m| m.contains("missing.png")),
        "不存在画像は broken-link で報告されるはず: {broken:?}"
    );
    // 実在画像と、画像記法で参照した実在文書は報告しない
    assert!(
        broken
            .iter()
            .all(|m| !m.contains("fig.png") && !m.contains("b.md")),
        "実在する画像・文書への画像参照は報告しないはず: {broken:?}"
    );
    assert_eq!(
        broken.len(),
        1,
        "報告は missing.png の 1 件のみのはず: {broken:?}"
    );
    // 画像記法による文書参照は被リンクに数えないため b.md は orphan のまま
    let orphan: Vec<&String> = found
        .iter()
        .filter(|(id, _)| id == "relation/orphan")
        .map(|(_, m)| m)
        .collect();
    assert!(
        orphan.iter().any(|m| m.contains("b.md")),
        "画像参照は被リンクに数えないため b.md は orphan のはず: {orphan:?}"
    );
    Ok(())
}

#[test]
fn orphan_counts_only_resolved_link_targets() -> anyhow::Result<()> {
    // 拡張子なしリンク `alias` は alias.md に解決される。同じ候補列に居た
    // alias.mdx は参照とみなさず orphan として報告する(未解決候補の過剰参照を防ぐ)
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/orphan");
    let mut sources = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "md" || e == "mdx") {
            let source = fs::read_to_string(&path)?;
            let (doc, _, _) = Doc::parse(path, source);
            sources.push(doc);
        }
    }
    assert_eq!(sources.len(), 3, "fixture は 3 ファイルのはず");

    let bodies: Vec<String> = vec![String::new(); sources.len()];
    let mtimes = vec![SystemTime::UNIX_EPOCH; sources.len()];
    let corpus = Corpus::new(
        &sources,
        &bodies,
        &[],
        &mtimes,
        MergeConfig::default(),
        DetectionConfig::default(),
        HashSet::new(),
        &[],
        &[],
    )?;
    let mut orphans: Vec<String> = Vec::new();
    let mut broken = 0usize;
    for rule in default_corpus_rules(&Config::default(), &[]) {
        for d in rule.run(&corpus)?.diagnostics {
            match d.rule_id {
                "relation/orphan" => orphans.push(d.observation.clone()),
                "relation/broken-link" => broken += 1,
                _ => {}
            }
        }
    }
    assert_eq!(broken, 0, "alias は alias.md に解決されるはず: {broken}");
    assert_eq!(
        orphans.len(),
        1,
        "孤立は alias.mdx の 1 件のはず: {orphans:?}"
    );
    assert!(orphans[0].contains("alias.mdx"), "{orphans:?}");
    Ok(())
}

#[test]
fn orphan_entry_points_are_configurable() -> anyhow::Result<()> {
    // detection.orphan_entry_points(gitignore 構文)でエントリポイントを
    // 設定できる(Issue #6)。fixture: README.md が alias(=alias.md)を
    // 参照し、alias.mdx は無参照、README.md を参照する文書は無い
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/orphan");
    let mut sources = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "md" || e == "mdx") {
            let source = fs::read_to_string(&path)?;
            let (doc, _, _) = Doc::parse(path, source);
            sources.push(doc);
        }
    }
    assert_eq!(sources.len(), 3, "fixture は 3 ファイルのはず");

    let bodies: Vec<String> = vec![String::new(); sources.len()];
    let mtimes = vec![SystemTime::UNIX_EPOCH; sources.len()];

    let orphans = |detection: DetectionConfig| -> anyhow::Result<Vec<String>> {
        let corpus = Corpus::new(
            &sources,
            &bodies,
            &[],
            &mtimes,
            MergeConfig::default(),
            detection,
            HashSet::new(),
            &[],
            &[],
        )?;
        let mut found = Vec::new();
        for rule in default_corpus_rules(&Config::default(), &[]) {
            if rule.id() != "relation/orphan" {
                continue;
            }
            for d in rule.run(&corpus)?.diagnostics {
                found.push(d.observation.clone());
            }
        }
        Ok(found)
    };

    // A: 空にすると README.md も孤立として報告される
    let found = orphans(DetectionConfig {
        orphan_entry_points: vec![],
        ..Default::default()
    })?;
    assert!(
        found.iter().any(|m| m.contains("README.md")),
        "エントリポイントが空なら README.md も孤立のはず: {found:?}"
    );

    // B: alias.mdx をエントリポイントにすると alias.mdx は報告されず、
    // 無参照の README.md が報告される
    let found = orphans(DetectionConfig {
        orphan_entry_points: vec!["alias.mdx".into()],
        ..Default::default()
    })?;
    assert!(
        !found.iter().any(|m| m.contains("alias.mdx")),
        "エントリポイント指定した alias.mdx は報告しないはず: {found:?}"
    );
    assert!(
        found.iter().any(|m| m.contains("README.md")),
        "README.md を参照する文書は無いので孤立のはず: {found:?}"
    );

    // C: 無効なパターンは設定エラーになる
    // (gitignore 層は allow_unclosed_class が既定のため "[" はエラーにならず、
    // 逆順の文字クラスで InvalidRange を起こす)
    let corpus = Corpus::new(
        &sources,
        &bodies,
        &[],
        &mtimes,
        MergeConfig::default(),
        DetectionConfig {
            orphan_entry_points: vec!["[z-a]".into()],
            ..Default::default()
        },
        HashSet::new(),
        &[],
        &[],
    )?;
    let orphan_rule = default_corpus_rules(&Config::default(), &[])
        .into_iter()
        .find(|r| r.id() == "relation/orphan")
        .expect("orphan ルールがあるはず");
    assert!(
        orphan_rule.run(&corpus).is_err(),
        "無効なパターンはエラーになるはず"
    );
    Ok(())
}

#[test]
fn link_resolution_tolerates_directories_html_and_encoding() -> anyhow::Result<()> {
    // 実在する html・ディレクトリ・URL エンコードされた空白入りファイルは
    // リンク切れにしない(設計書 §5.4)。明示指定ファイルは orphan 扱いにしない
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/links");
    let page = dir.join("page.md");
    let source = fs::read_to_string(&page)?;
    let (doc, _, _) = Doc::parse(page.clone(), source);
    let docs = vec![doc];
    let bodies = vec![String::new()];
    let mtimes = vec![SystemTime::UNIX_EPOCH];
    let explicit: HashSet<std::path::PathBuf> = [page].iter().map(|p| normalize_path(p)).collect();
    let corpus = Corpus::new(
        &docs,
        &bodies,
        &[],
        &mtimes,
        MergeConfig::default(),
        DetectionConfig::default(),
        explicit,
        &[],
        &[],
    )?;

    let mut broken: Vec<String> = Vec::new();
    let mut orphans = 0usize;
    for rule in default_corpus_rules(&Config::default(), &[]) {
        for diagnostic in rule.run(&corpus)?.diagnostics {
            match diagnostic.rule_id {
                "relation/broken-link" => broken.push(diagnostic.observation.clone()),
                "relation/orphan" => orphans += 1,
                _ => {}
            }
        }
    }
    assert_eq!(broken.len(), 1, "切れは nope.md の 1 件のはず: {broken:?}");
    assert!(broken[0].contains("nope.md"));
    assert_eq!(orphans, 0, "明示指定したファイルは孤立扱いにしない");
    Ok(())
}

/// 表示名だけを持つ空の Doc(重複検出ルールのテスト用)。
fn empty_doc(name: &str) -> Doc {
    let (doc, _, _) = Doc::parse(PathBuf::from(name), String::new());
    doc
}

#[test]
fn consolidation_groups_duplicates_and_respects_min_similarity() -> anyhow::Result<()> {
    // 重複検出と統合提案: 3 文書が同一グループなら正候補以外の 2 件に提案が出る。
    // 報告閾値(detection.doc_similarity)と統合閾値(merge.min_similarity)は
    // 独立している(§8)。統合閾値だけ上げても重複の報告は消えない
    let base = "このセクションでは設定ファイルの読み込み処理について説明します。設定ファイルは起動時に一度だけ読み込まれ、変更された場合は再起動が必要です。読み込みに失敗した場合はエラーメッセージを表示して終了します。".repeat(3);
    let docs = vec![empty_doc("a.md"), empty_doc("b.md"), empty_doc("c.md")];
    let bodies = vec![
        jaio_index::normalize_text(&base),
        jaio_index::normalize_text(&base),
        jaio_index::normalize_text(&base),
    ];
    let tokens = vec![300usize; 3];
    let mtimes = vec![SystemTime::UNIX_EPOCH; 3];

    let run = |merge: MergeConfig, detection: DetectionConfig| -> anyhow::Result<(usize, usize)> {
        let corpus = Corpus::new(
            &docs,
            &bodies,
            &tokens,
            &mtimes,
            merge,
            detection,
            HashSet::new(),
            &[],
            &[],
        )?;
        let mut duplications = 0usize;
        let mut plans = 0usize;
        for rule in default_corpus_rules(&Config::default(), &[]) {
            for d in rule.run(&corpus)?.diagnostics {
                match d.rule_id {
                    "relation/cross-doc-duplication" => duplications += 1,
                    "merge/consolidation-plan" => plans += 1,
                    _ => {}
                }
            }
        }
        Ok((duplications, plans))
    };

    // 既定では同一本文の 3 ペアすべて重複。報告はペア両側の文書に付くため
    // 3 ペア x 両側 = 6 件、統合提案は正候補以外の 2 件(§5.4、#22)
    let (duplications, plans) = run(MergeConfig::default(), DetectionConfig::default())?;
    assert_eq!(duplications, 6, "3 文書の全ペアが両側に報告されるはず");
    assert_eq!(plans, 2, "正候補以外の 2 件に統合提案が出るはず");

    // 統合閾値だけを 1.0 超にすると提案は消えるが重複の報告は残る(閾値の分離)
    let strict_merge = MergeConfig {
        min_similarity: 1.01,
        ..Default::default()
    };
    let (duplications, plans) = run(strict_merge, DetectionConfig::default())?;
    assert_eq!(
        (duplications, plans),
        (6, 0),
        "統合閾値を上げても重複の報告は残るはず"
    );

    // 報告閾値を 1.0 超にすると報告・提案ともに出ない
    let strict_detection = DetectionConfig {
        doc_similarity: 1.01,
        ..Default::default()
    };
    let (duplications, plans) = run(MergeConfig::default(), strict_detection)?;
    assert_eq!((duplications, plans), (0, 0), "報告閾値超過では検出しない");
    Ok(())
}

#[test]
fn detects_doc_comment_drift() -> anyhow::Result<()> {
    // コメントと文書の本文が高類似のとき重複として報告する(§5.4)。
    // 文書本文は比較対象の最小長(200 字)を超える必要がある
    let body = "このセクションでは設定ファイルの読み込み処理について説明します。設定ファイルは起動時に一度だけ読み込まれ、変更された場合は再起動が必要です。読み込みに失敗した場合はエラーメッセージを表示して終了します。".repeat(3);
    let docs = vec![empty_doc("docs/config.md")];
    let bodies = vec![jaio_index::normalize_text(&body)];
    let tokens = vec![300usize];
    let mtimes = vec![SystemTime::UNIX_EPOCH];
    let comments = vec![CommentFile {
        path: PathBuf::from("src/config.rs"),
        display: "src/config.rs".to_string(),
        body: jaio_index::normalize_text(&body),
        first_line: 7,
        sha256: String::new(),
    }];
    let corpus = Corpus::new(
        &docs,
        &bodies,
        &tokens,
        &mtimes,
        MergeConfig::default(),
        DetectionConfig::default(),
        HashSet::new(),
        &comments,
        &[],
    )?;
    let mut found = Vec::new();
    for rule in default_corpus_rules(&Config::default(), &[]) {
        for diagnostic in rule.run(&corpus)?.diagnostics {
            found.push(diagnostic.rule_id.to_string());
        }
    }
    assert!(
        found.iter().any(|id| id == "relation/doc-comment-drift"),
        "コメントと文書の重複を検出するはず: {found:?}"
    );
    Ok(())
}

#[test]
fn short_comments_are_ignored_for_drift() -> anyhow::Result<()> {
    // 最小長(100 字)未満のコメントは偶然の一致が多く対象外
    let doc_body = "このセクションでは設定ファイルの読み込み処理について説明します。設定ファイルは起動時に一度だけ読み込まれ、変更された場合は再起動が必要です。読み込みに失敗した場合はエラーメッセージを表示して終了します。".repeat(3);
    let comment_body = "設定を読み込みます";
    let docs = vec![empty_doc("docs/config.md")];
    let bodies = vec![jaio_index::normalize_text(&doc_body)];
    let mtimes = vec![SystemTime::UNIX_EPOCH];
    let comments = vec![CommentFile {
        path: PathBuf::from("src/config.rs"),
        display: "src/config.rs".to_string(),
        body: jaio_index::normalize_text(comment_body),
        first_line: 1,
        sha256: String::new(),
    }];
    let corpus = Corpus::new(
        &docs,
        &bodies,
        &[],
        &mtimes,
        MergeConfig::default(),
        DetectionConfig::default(),
        HashSet::new(),
        &comments,
        &[],
    )?;
    let mut found = Vec::new();
    for rule in default_corpus_rules(&Config::default(), &[]) {
        for diagnostic in rule.run(&corpus)?.diagnostics {
            found.push(diagnostic.rule_id.to_string());
        }
    }
    assert!(
        !found.iter().any(|id| id == "relation/doc-comment-drift"),
        "短いコメントは検出しないはず: {found:?}"
    );
    Ok(())
}

#[test]
fn style_target_plain_reports_polite_deviation() -> anyhow::Result<()> {
    // [style] target = "plain" の場合: である体優勢の文書でですます体の混入を報告する
    let config = Config::from_toml(
        r#"[style]
target = "plain""#,
    )?;
    let source = "# 題名\n\n解析する。\n整形する。\n出力します。\n";
    let (doc, leaves, headings) = Doc::parse(PathBuf::from("plain.md"), source.to_string());
    let mut analyzer = Analyzer::new()?;
    let counter = TokenCounter::new()?;

    let mut severities = Vec::new();
    for rule in default_rules(&config) {
        let mut cache = HashMap::new();
        let mut ctx = DocContext {
            doc: &doc,
            leaves: &leaves,
            headings: &headings,
            analyzer: &mut analyzer,
            counter: &counter,
            detection: &DetectionConfig::default(),
            cache: &mut cache,
        };
        for diagnostic in rule.run(&mut ctx)?.diagnostics {
            if diagnostic.rule_id == "consistency/style-deviation" {
                severities.push(diagnostic.severity);
            }
        }
    }
    assert_eq!(
        severities,
        vec![Severity::Warn],
        "である体既定では敬体の混入が warn のはず"
    );
    Ok(())
}

#[test]
fn user_redundant_patterns_are_applied() -> anyhow::Result<()> {
    // [patterns.redundant] のユーザー拡張パターンで検出する
    let config = Config::from_toml(
        r#"[patterns.redundant]
"検討を行う必要がございます" = "検討が必要です""#,
    )?;
    let source = "# 題名\n\n本件について検討を行う必要がございます。\n";
    let (doc, leaves, headings) = Doc::parse(PathBuf::from("user.md"), source.to_string());
    let mut analyzer = Analyzer::new()?;
    let counter = TokenCounter::new()?;

    let mut found = Vec::new();
    for rule in default_rules(&config) {
        let mut cache = HashMap::new();
        let mut ctx = DocContext {
            doc: &doc,
            leaves: &leaves,
            headings: &headings,
            analyzer: &mut analyzer,
            counter: &counter,
            detection: &DetectionConfig::default(),
            cache: &mut cache,
        };
        for diagnostic in rule.run(&mut ctx)?.diagnostics {
            if diagnostic.rule_id == "efficiency/redundant-expression" {
                found.push(diagnostic.observation.clone());
            }
        }
    }
    assert!(
        found
            .iter()
            .any(|m| m.contains("検討を行う必要がございます")),
        "ユーザー拡張パターンを検出するはず: {found:?}"
    );
    Ok(())
}

#[test]
fn allowed_terms_suppress_notation_variant() -> anyhow::Result<()> {
    // [lint.allow] terms に含まれる表記のペアは表記ゆれとして報告しない
    let config = Config::from_toml(
        r#"[lint.allow]
terms = ["行なう"]"#,
    )?;
    let source = "解析を行う。出力を行う。確認を行なう。\n";
    let (doc, leaves, headings) = Doc::parse(PathBuf::from("allow.md"), source.to_string());
    let mut analyzer = Analyzer::new()?;
    let counter = TokenCounter::new()?;

    let mut found = Vec::new();
    for rule in default_rules(&config) {
        let mut cache = HashMap::new();
        let mut ctx = DocContext {
            doc: &doc,
            leaves: &leaves,
            headings: &headings,
            analyzer: &mut analyzer,
            counter: &counter,
            detection: &DetectionConfig::default(),
            cache: &mut cache,
        };
        for diagnostic in rule.run(&mut ctx)?.diagnostics {
            if diagnostic.rule_id == "consistency/notation-variant" {
                found.push(diagnostic.observation.clone());
            }
        }
    }
    assert!(
        found.is_empty(),
        "許容表記を含むペアは報告しないはず: {found:?}"
    );
    Ok(())
}

#[test]
fn empty_user_pattern_is_ignored() -> anyhow::Result<()> {
    // 空のパターンは find が常に先頭一致を返すため除外される(進行不能の回避)
    let config = Config::from_toml(
        r#"[patterns.redundant]
"" = "x""#,
    )?;
    let source = "# 題名\n\n本文です。\n";
    let (doc, leaves, headings) = Doc::parse(PathBuf::from("empty.md"), source.to_string());
    let mut analyzer = Analyzer::new()?;
    let counter = TokenCounter::new()?;

    let mut found = Vec::new();
    for rule in default_rules(&config) {
        let mut cache = HashMap::new();
        let mut ctx = DocContext {
            doc: &doc,
            leaves: &leaves,
            headings: &headings,
            analyzer: &mut analyzer,
            counter: &counter,
            detection: &DetectionConfig::default(),
            cache: &mut cache,
        };
        for diagnostic in rule.run(&mut ctx)?.diagnostics {
            if diagnostic.rule_id == "efficiency/redundant-expression" {
                found.push(diagnostic.observation.clone());
            }
        }
    }
    assert!(
        found.is_empty(),
        "空パターンは何も検出しないはず: {found:?}"
    );
    Ok(())
}

#[test]
fn detects_terminology_variation() -> anyhow::Result<()> {
    // 内蔵ペア(サーバー/サーバ)の混在を検出する。単独出現では報告しない
    let source = "# 題名\n\nサーバーに接続します。サーバを再起動します。設定を保存します。\n";
    let (doc, leaves, headings) = Doc::parse(PathBuf::from("terms.md"), source.to_string());
    let mut analyzer = Analyzer::new()?;
    let counter = TokenCounter::new()?;

    let mut found = Vec::new();
    for rule in default_rules(&Config::default()) {
        let mut cache = HashMap::new();
        let mut ctx = DocContext {
            doc: &doc,
            leaves: &leaves,
            headings: &headings,
            analyzer: &mut analyzer,
            counter: &counter,
            detection: &DetectionConfig::default(),
            cache: &mut cache,
        };
        for diagnostic in rule.run(&mut ctx)?.diagnostics {
            if diagnostic.rule_id == "consistency/terminology" {
                found.push(diagnostic.observation.clone());
            }
        }
    }
    assert!(
        found
            .iter()
            .any(|m| m.contains("サーバー") && m.contains("サーバ")),
        "用語揺れを検出するはず: {found:?}"
    );

    // 片方だけの文書では報告しない
    let source_only = "# 題名\n\nサーバーに接続します。サーバーを再起動します。\n";
    let (doc, leaves, headings) = Doc::parse(PathBuf::from("terms2.md"), source_only.to_string());
    let mut found = Vec::new();
    for rule in default_rules(&Config::default()) {
        let mut cache = HashMap::new();
        let mut ctx = DocContext {
            doc: &doc,
            leaves: &leaves,
            headings: &headings,
            analyzer: &mut analyzer,
            counter: &counter,
            detection: &DetectionConfig::default(),
            cache: &mut cache,
        };
        for diagnostic in rule.run(&mut ctx)?.diagnostics {
            if diagnostic.rule_id == "consistency/terminology" {
                found.push(diagnostic.observation.clone());
            }
        }
    }
    assert!(found.is_empty(), "単独出現では報告しないはず: {found:?}");
    Ok(())
}

#[test]
fn user_term_pairs_are_applied() -> anyhow::Result<()> {
    // [terms] pairs のユーザー拡張ペアで検出する
    let config = Config::from_toml(
        r#"[terms]
pairs = [["かかけ", "カカケ"]]"#,
    )?;
    let source = "# 題名\n\nかかけを開きます。カカケを閉じます。\n";
    let (doc, leaves, headings) = Doc::parse(PathBuf::from("user-terms.md"), source.to_string());
    let mut analyzer = Analyzer::new()?;
    let counter = TokenCounter::new()?;

    let mut found = Vec::new();
    for rule in default_rules(&config) {
        let mut cache = HashMap::new();
        let mut ctx = DocContext {
            doc: &doc,
            leaves: &leaves,
            headings: &headings,
            analyzer: &mut analyzer,
            counter: &counter,
            detection: &DetectionConfig::default(),
            cache: &mut cache,
        };
        for diagnostic in rule.run(&mut ctx)?.diagnostics {
            if diagnostic.rule_id == "consistency/terminology" {
                found.push(diagnostic.observation.clone());
            }
        }
    }
    assert!(
        found.iter().any(|m| m.contains("カカケ")),
        "ユーザー拡張ペアを検出するはず: {found:?}"
    );
    Ok(())
}

#[test]
fn detects_repeated_quote() -> anyhow::Result<()> {
    // 30 字以上の同一文が 2 回出現したら報告する。短い文は対象外
    let sentence = "このセクションでは設定ファイルの読み込み処理について詳細に説明します。";
    let source = format!(
        "# 題名\n\n{sentence}\n\n別の話題です。まったく関係のない内容を書きます。\n\n{sentence}\n"
    );
    let (doc, leaves, headings) = Doc::parse(PathBuf::from("quote.md"), source);
    let mut analyzer = Analyzer::new()?;
    let counter = TokenCounter::new()?;

    let mut found = Vec::new();
    for rule in default_rules(&Config::default()) {
        let mut cache = HashMap::new();
        let mut ctx = DocContext {
            doc: &doc,
            leaves: &leaves,
            headings: &headings,
            analyzer: &mut analyzer,
            counter: &counter,
            detection: &DetectionConfig::default(),
            cache: &mut cache,
        };
        for diagnostic in rule.run(&mut ctx)?.diagnostics {
            if diagnostic.rule_id == "efficiency/repeated-quote" {
                found.push((diagnostic.span.line, diagnostic.observation.clone()));
            }
        }
    }
    assert_eq!(found.len(), 1, "同一文の重複を 1 件検出するはず: {found:?}");
    assert_eq!(
        found[0].0, 7,
        "2 回目の出現行(7 行目)を位置として報告するはず"
    );
    Ok(())
}

#[test]
fn allowed_terms_suppress_terminology() -> anyhow::Result<()> {
    // [lint.allow] terms に含まれる表記の用語ペアは報告しない
    // (TerminologyRule の help が [lint.allow] を案内しているため挙動を一致させる)
    let config = Config::from_toml(
        r#"[lint.allow]
terms = ["サーバ"]"#,
    )?;
    let source = "# 題名\n\nサーバーに接続します。サーバを再起動します。\n";
    let (doc, leaves, headings) = Doc::parse(PathBuf::from("allow-terms.md"), source.to_string());
    let mut analyzer = Analyzer::new()?;
    let counter = TokenCounter::new()?;

    let mut found = Vec::new();
    for rule in default_rules(&config) {
        let mut cache = HashMap::new();
        let mut ctx = DocContext {
            doc: &doc,
            leaves: &leaves,
            headings: &headings,
            analyzer: &mut analyzer,
            counter: &counter,
            detection: &DetectionConfig::default(),
            cache: &mut cache,
        };
        for diagnostic in rule.run(&mut ctx)?.diagnostics {
            if diagnostic.rule_id == "consistency/terminology" {
                found.push(diagnostic.observation.clone());
            }
        }
    }
    assert!(
        found.is_empty(),
        "許容表記を含む用語ペアは報告しないはず: {found:?}"
    );
    Ok(())
}

#[test]
fn filler_is_reported_as_separate_rule() -> anyhow::Result<()> {
    // 置き換え先が空のパターン(フィラー)は efficiency/filler として報告し、
    // redundant-expression からは出ない
    let source = "# 題名\n\n本件は重要であると言っても過言ではない。\n";
    let (doc, leaves, headings) = Doc::parse(PathBuf::from("filler.md"), source.to_string());
    let mut analyzer = Analyzer::new()?;
    let counter = TokenCounter::new()?;

    let mut found = Vec::new();
    for rule in default_rules(&Config::default()) {
        let mut cache = HashMap::new();
        let mut ctx = DocContext {
            doc: &doc,
            leaves: &leaves,
            headings: &headings,
            analyzer: &mut analyzer,
            counter: &counter,
            detection: &DetectionConfig::default(),
            cache: &mut cache,
        };
        for diagnostic in rule.run(&mut ctx)?.diagnostics {
            found.push((
                diagnostic.rule_id.to_string(),
                diagnostic.observation.clone(),
            ));
        }
    }
    assert!(
        found
            .iter()
            .any(|(id, m)| id == "efficiency/filler" && m.contains("と言っても過言ではない")),
        "フィラーは efficiency/filler で検出するはず: {found:?}"
    );
    assert!(
        !found
            .iter()
            .any(|(id, _)| id == "efficiency/redundant-expression"),
        "フィラーは redundant-expression から出ないはず: {found:?}"
    );
    Ok(())
}

#[test]
fn early_position_particle_is_captured() -> anyhow::Result<()> {
    // 複数格の文で、動詞から離れた前側の格の揺れも拾う(Issue #5)
    // 「結果をサーバーへ送信する」と「結果をサーバーに送信する」は
    // 従来の「直近 1 組のみ」ではどちらも「を」しか記録せず漏れていた
    let source = "結果をサーバーへ送信する。結果をサーバーに送信する。結果をサーバーに送信する。\n";
    let found: Vec<String> = diagnostics_on_with(source, particle_enabled())?
        .into_iter()
        .map(|d| d.rule_id.to_string())
        .collect();
    assert!(
        found.iter().any(|id| id == "consistency/particle"),
        "前側の格の揺れ(へ/に)を検出するはず: {found:?}"
    );
    Ok(())
}

#[test]
fn punctuation_counts_occurrences_not_leaves() -> anyhow::Result<()> {
    // 1 リーフ内の複数出現が件数に反映される(#42)。
    // リーフ数ベースだと「、」2 vs「，」6 で多数派が逆転していた
    let source = "設定を読み込み，保存し，検証し，出力し，記録し，報告し，完了する。そして次の処理へ、進む。\n";
    let mut analyzer = Analyzer::new()?;
    let counter = TokenCounter::new()?;

    let mut messages = Vec::new();
    for rule in default_rules(&Config::default()) {
        let (doc, leaves, headings) = Doc::parse(PathBuf::from("punct.md"), source.to_string());
        let mut cache = HashMap::new();
        let mut ctx = DocContext {
            doc: &doc,
            leaves: &leaves,
            headings: &headings,
            analyzer: &mut analyzer,
            counter: &counter,
            detection: &DetectionConfig::default(),
            cache: &mut cache,
        };
        if let Ok(run) = rule.run(&mut ctx) {
            for d in run.diagnostics {
                if d.rule_id == "consistency/punctuation" {
                    messages.push(d.observation.clone());
                }
            }
        }
    }
    // 実出現は「，」=6 個、「、」=1 個 → 多数派は「，」で、minor「、」が 1 箇所として報告される
    assert!(
        messages
            .iter()
            .any(|m| m.contains("「，」6箇所と「、」1箇所")),
        "出現数ベースの件数が報告されるはず: {messages:?}"
    );
    // リーフ数ベースの逆転(「，」を 1 箇所と数える)は出ない
    assert!(
        !messages.iter().any(|m| m.contains("「，」1箇所")),
        "リーフ数ベースの逆転表示は出ないはず: {messages:?}"
    );
    Ok(())
}

/// StyleDeviationRule を default_rules 経由で実行する(#55 検証用)。
/// jaio.toml 相当の TOML で設定した文書単位解決を試す
fn run_style_rule(
    path: &str,
    source: &str,
    config: &Config,
) -> anyhow::Result<Vec<jaio_core::Diagnostic>> {
    let (doc, leaves, headings) = Doc::parse(PathBuf::from(path), source.to_string());
    let mut analyzer = Analyzer::new()?;
    let counter = TokenCounter::new()?;
    let detection = DetectionConfig::default();
    let mut cache = HashMap::new();
    let mut found = Vec::new();
    for rule in default_rules(config) {
        if rule.id() != "consistency/style-deviation" {
            continue;
        }
        let mut ctx = DocContext {
            doc: &doc,
            leaves: &leaves,
            headings: &headings,
            analyzer: &mut analyzer,
            cache: &mut cache,
            counter: &counter,
            detection: &detection,
        };
        found.extend(rule.run(&mut ctx)?.diagnostics);
    }
    Ok(found)
}

/// frontmatter の jaio-style 宣言がリポジトリ既定より優先される(#55)
#[test]
fn frontmatter_declares_document_style() -> anyhow::Result<()> {
    let plain = Config::from_toml("[style]\ntarget = \"plain\"\n")?;
    // リポジトリ既定は plain(である体)。frontmatter で polite を宣言した
    // 文書は敬体として判定され、である体の文が逸脱になる
    let source = "---\njaio-style: polite\n---\n\n# 題名\n\n解析する。\n解析した。\n";
    let diags = run_style_rule("doc.md", source, &plain)?;
    assert!(
        diags.iter().any(|d| d.observation.contains("である体")),
        "frontmatter 宣言(polite)によりである体が逸脱になるはず: {diags:?}"
    );
    // 宣言なしの同じ文書は plain 既定どおり逸脱にならない
    let plain_source = "# 題名\n\n解析する。\n解析した。\n";
    let diags = run_style_rule("doc.md", plain_source, &plain)?;
    assert!(
        diags.is_empty(),
        "plain 既定の文書は逸脱なしのはず: {diags:?}"
    );
    // 不正な値の宣言は無視される(既定が使われる)
    let invalid = "---\njaio-style: teinei\n---\n\n# 題名\n\n解析する。\n解析した。\n";
    let diags = run_style_rule("doc.md", invalid, &plain)?;
    assert!(
        diags.is_empty(),
        "不正な jaio-style 値は無視されるはず: {diags:?}"
    );
    // ネストした mapping・ブロックスカラー内の jaio-style は宣言ではない(#67)。
    // 誤読されると plain 既定の文書が polite に切り替わり逸脱が出てしまう
    let nested = "---\nexample:\n  jaio-style: polite\n---\n\n# 題名\n\n解析する。\n解析した。\n";
    let diags = run_style_rule("doc.md", nested, &plain)?;
    assert!(
        diags.is_empty(),
        "ネスト内の jaio-style は実効 target を変えないはず: {diags:?}"
    );
    let block_scalar =
        "---\ndescription: |\n  jaio-style: polite\n---\n\n# 題名\n\n解析する。\n解析した。\n";
    let diags = run_style_rule("doc.md", block_scalar, &plain)?;
    assert!(
        diags.is_empty(),
        "ブロックスカラー内の jaio-style は実効 target を変えないはず: {diags:?}"
    );
    Ok(())
}

/// [style.overrides] のパスパターンで文書単位の文体を上書きできる(#55)。
/// frontmatter 宣言はパターンより優先する
#[test]
fn style_overrides_pattern_and_precedence() -> anyhow::Result<()> {
    let plain = Config::from_toml("[style]\ntarget = \"plain\"\n")?;
    let with_overrides = Config::from_toml(
        "[style]\ntarget = \"plain\"\n\n[style.overrides]\n\"docs/templates/**\" = \"polite\"\n",
    )?;
    // テンプレート配下の文書は敬体として判定される(既定 plain に対して逸脱が出る)
    let source = "# テンプレ\n\n解析する。\n解析した。\n";
    let diags = run_style_rule("docs/templates/report.md", source, &with_overrides)?;
    assert!(
        diags.iter().any(|d| d.observation.contains("である体")),
        "overrides パターンで敬体判定になるはず: {diags:?}"
    );
    // パターン外の文書は既定(plain)どおり
    let diags = run_style_rule("docs/design.md", source, &with_overrides)?;
    assert!(diags.is_empty(), "パターン外は既定どおり: {diags:?}");
    // frontmatter 宣言はパターンより優先する
    let declared = "---\njaio-style: plain\n---\n\n# テンプレ\n\n解析する。\n解析した。\n";
    let diags = run_style_rule("docs/templates/report.md", declared, &with_overrides)?;
    assert!(
        diags.is_empty(),
        "frontmatter 宣言(plain)がパターン(polite)に優先するはず: {diags:?}"
    );
    // 設定なしでも挙動は従来どおり
    let diags = run_style_rule("docs/templates/report.md", source, &plain)?;
    assert!(diags.is_empty(), "設定なしなら既定どおり: {diags:?}");
    Ok(())
}

/// 走査結果のパス(`./` 付き)でも overrides の照合が効くかの回帰確認(#55)
#[test]
fn style_overrides_match_leading_dot_slash_path() -> anyhow::Result<()> {
    let with_overrides = Config::from_toml(
        "[style]\ntarget = \"plain\"\n\n[style.overrides]\n\"docs/templates/**\" = \"polite\"\n",
    )?;
    let source = "# テンプレ\n\n解析する。\n解析した。\n";
    // ignore walker が `.` 基準で返す形式(./docs/...)
    let diags = run_style_rule("./docs/templates/report.md", source, &with_overrides)?;
    assert!(
        diags.iter().any(|d| d.observation.contains("である体")),
        "`./` 付きパスでも照合できるはず: {diags:?}"
    );
    Ok(())
}
