//! リポジトリ横断ルールの統合テスト(SimHash 重複検出・統合提案、設計書 §5.4/§5.5)。

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use jaio_core::config::{Config, DetectionConfig, MergeConfig};
use jaio_core::{Diagnostic, MeasurementStatus, Severity};
use jaio_index::normalize_text;
use jaio_parse::doc::Doc;
use jaio_rules::{Corpus, DocContext, default_corpus_rules};

/// 重複検出の比較対象になる本文(正規化後 200 文字超)。
const BODY: &str = "このドキュメントでは設定ファイルの読み込み処理とその運用について説明します。\
設定ファイルは起動時に一度だけ読み込まれ、変更は再起動後に反映されます。\
設定項目の一覧、既定値、上書き方法、および運用上の注意点を順に整理します。\
設定の検証は読み込み直後に行われ、不正な項目がある場合は起動を中断します。\
大規模リポジトリでの運用を想定し、除外パターンと設定の継承規則についても説明します。\
必要に応じて環境変数による値の上書きも可能です。";

/// 内容が異なる本文(偽陽性確認用。こちらも 200 文字超)。
const OTHER_BODY: &str = "このドキュメントではコマンドライン引数の解析と終了コードの扱いについて説明します。\
引数はサブコマンド形式で解析され、不明な指定は即座にエラーになります。\
終了コードは問題の重大度から決まり、深刻な診断がある場合に限って失敗を表します。\
出力形式は端末向けと機械可読の二種類があり、後者は他ツールからの利用を想定します。\
大量のファイルを扱うため、走査は並列化され、対象は設定で絞り込めます。\
これらの組合せによって自動化パイプラインへの組み込みを可能にします。";

/// 全横断ルールを実行し、診断をそのまま返す。
/// severity や help を検証するテストはこちらを使う。
fn collect_corpus_diagnostics(sources: &[(&str, String)]) -> anyhow::Result<Vec<Diagnostic>> {
    let mtimes = vec![SystemTime::UNIX_EPOCH; sources.len()];
    collect_corpus_diagnostics_with(sources, &mtimes, MergeConfig::default())
}

/// mtimes と merge 設定を指定できる実行ヘルパ(#34 の mtime 依存検証用)。
fn collect_corpus_diagnostics_with(
    sources: &[(&str, String)],
    mtimes: &[SystemTime],
    merge: MergeConfig,
) -> anyhow::Result<Vec<Diagnostic>> {
    let mut docs = Vec::new();
    let mut bodies = Vec::new();
    let tokens = vec![100usize; sources.len()];
    for (name, source) in sources {
        let (doc, leaves, _headings) = Doc::parse(PathBuf::from(*name), source.clone());
        // CLI と同じ方法で正規化済み本文を作る(本文リーフのみ)
        let mut body = String::new();
        for leaf in leaves.iter().filter(|l| DocContext::is_body_leaf(l)) {
            body.push_str(&normalize_text(&leaf.text));
        }
        bodies.push(body);
        docs.push(doc);
    }
    let corpus = Corpus::new(
        &docs,
        &bodies,
        &tokens,
        mtimes,
        merge,
        DetectionConfig::default(),
        HashSet::new(),
        &[],
        &[],
    )?;
    let mut found = Vec::new();
    for rule in default_corpus_rules(&Config::default(), &[]) {
        found.append(&mut rule.run(&corpus)?.diagnostics);
    }
    Ok(found)
}

/// (ルール ID、対象ファイル、メッセージ)の組で全横断ルールを実行する。
fn run_corpus_rules(sources: &[(&str, String)]) -> anyhow::Result<Vec<(String, String, String)>> {
    Ok(collect_corpus_diagnostics(sources)?
        .into_iter()
        .map(|d| (d.rule_id.to_string(), d.file, d.observation))
        .collect())
}

#[test]
fn detects_cross_doc_duplication_and_proposes_consolidation() -> anyhow::Result<()> {
    let sources = [
        ("docs/README.md", format!("# 概要\n\n{BODY}\n")),
        ("docs/detail.md", format!("# 詳細\n\n{BODY}\n")),
    ];
    let found = run_corpus_rules(&sources)?;
    assert!(
        found
            .iter()
            .any(|(id, _, _)| id == "relation/cross-doc-duplication"),
        "文書間重複を検出するはず: {found:?}"
    );
    let plan = found
        .iter()
        .find(|(id, _, _)| id == "merge/consolidation-plan")
        .expect("統合提案が出るはず");
    assert_eq!(plan.1, "docs/detail.md", "統合される側に診断が出るはず");
    assert!(
        plan.2.contains("docs/README.md"),
        "README が正候補として提示されるはず: {}",
        plan.2
    );
    Ok(())
}

#[test]
fn short_duplicated_documents_are_detected() -> anyhow::Result<()> {
    // 短い文書(閾値 50 文字超)でも完全重複は検出される。
    // 旧実装の 200 文字下限では静かに検出対象から漏れていた(回帰試験)
    let body = "設定ファイルは起動時に一度だけ読み込まれます。変更は再起動後に反映されます。検証は読み込み直後に行います。";
    let sources = [
        ("docs/README.md", format!("# 概要\n\n{body}\n")),
        ("docs/short.md", format!("# 短い\n\n{body}\n")),
    ];
    let found = run_corpus_rules(&sources)?;
    assert!(
        found
            .iter()
            .any(|(id, _, _)| id == "relation/cross-doc-duplication"),
        "短い文書の完全重複も検出するはず: {found:?}"
    );
    Ok(())
}

#[test]
fn cross_doc_duplication_caps_pairs_per_document() -> anyhow::Result<()> {
    // 1 文書への重複ペアは類似度上位 3 件まで warn、残りは「ほか N 件」の
    // info 1 件に集約される(設計書 §5.4)。dupN 同士も互いに重複ペアに
    // なるため、base を配列の最後に置いて base 側(ペアの b)のペアを
    // 5 件(上位 3 + 残り 2)にする
    let sources = [
        ("docs/dup1.md", format!("# 重複1\n\n{BODY}\n")),
        ("docs/dup2.md", format!("# 重複2\n\n{BODY}\n")),
        ("docs/dup3.md", format!("# 重複3\n\n{BODY}\n")),
        ("docs/dup4.md", format!("# 重複4\n\n{BODY}\n")),
        ("docs/dup5.md", format!("# 重複5\n\n{BODY}\n")),
        ("docs/base.md", format!("# 概要\n\n{BODY}\n")),
    ];
    let diags = collect_corpus_diagnostics(&sources)?;
    let for_base: Vec<_> = diags
        .iter()
        .filter(|d| d.rule_id == "relation/cross-doc-duplication" && d.file == "docs/base.md")
        .collect();
    let warns: Vec<_> = for_base
        .iter()
        .filter(|d| d.severity == Severity::Warn)
        .collect();
    assert_eq!(
        warns.len(),
        3,
        "warn は類似度上位 3 件だけのはず: {for_base:?}"
    );
    let infos: Vec<_> = for_base
        .iter()
        .filter(|d| d.severity == Severity::Info)
        .collect();
    assert_eq!(
        infos.len(),
        1,
        "残りは 1 件の info に集約されるはず: {for_base:?}"
    );
    assert!(
        infos[0].observation.contains("ほかに 2 件"),
        "warn に残らなかった 2 件を案内するはず: {}",
        infos[0].observation
    );
    assert!(
        infos[0]
            .review_questions
            .iter()
            .any(|q| q.contains("jaio stats --similarity")),
        "review_questions にペア一覧の確認方法が入るはず: {:?}",
        infos[0].review_questions
    );
    // ペアは a < b の向きだが報告は両側に付く。最小インデックスの dup1 は
    // 常に a 側だが、warn 3 件 + 集約 info 1 件が出るはず(#22)
    let for_dup1: Vec<_> = diags
        .iter()
        .filter(|d| d.rule_id == "relation/cross-doc-duplication" && d.file == "docs/dup1.md")
        .collect();
    assert_eq!(
        for_dup1
            .iter()
            .filter(|d| d.severity == Severity::Warn)
            .count(),
        3,
        "a 側にしかならない文書にも warn が出るはず: {for_dup1:?}"
    );
    assert!(
        for_dup1
            .iter()
            .any(|d| d.severity == Severity::Info && d.observation.contains("ほかに 2 件")),
        "a 側の文書にも集約 info が出るはず: {for_dup1:?}"
    );
    Ok(())
}

#[test]
fn cross_doc_duplication_reports_both_sides_of_a_pair() -> anyhow::Result<()> {
    // 2 文書の重複ペアでは両方のファイルに warn が付く(#22:
    // b 側だけ報告だとクラスタ先頭の文書が診断ゼロになる)
    let sources = [
        ("docs/README.md", format!("# 概要\n\n{BODY}\n")),
        ("docs/detail.md", format!("# 詳細\n\n{BODY}\n")),
    ];
    let diags = collect_corpus_diagnostics(&sources)?;
    for file in ["docs/README.md", "docs/detail.md"] {
        assert!(
            diags
                .iter()
                .any(|d| d.rule_id == "relation/cross-doc-duplication"
                    && d.file == file
                    && d.severity == Severity::Warn),
            "{file} にも warn が出るはず: {diags:?}"
        );
    }
    Ok(())
}

#[test]
fn detects_dir_mismatch_against_directory_readme() -> anyhow::Result<()> {
    // 同一ディレクトリの README を役割アンカーとし、用語整合が低い文書を報告する。
    // README は DIR_README_MIN_CHARS(300 字)超の本文が要る(アンカー条件)。
    // 各文書の本文は doc_body_min_chars(50 字)超にする(未満だと比較対象から外れる)
    let readme_body = "このディレクトリでは決済基盤の支払いフローと精算処理の設計を管理します。取引は作成後に与信確認を経て確定し、失敗時は返金処理へ遷移します。精算は日次のバッチで取引を集計し、手数料を差し引いて入金指示を出します。状態遷移の各段階で重複実行を防ぐための識別子を記録し、監査ログに残します。与信は外部サービスの判定結果をキャッシュし、精算処理の締め切り時刻を過ぎた取引は翌日に繰り越します。返金は一部返金と全額返金の両方を扱い、精算済みの取引には負の金額で反映します。精算処理の結果は支払いフロー全体に反映され、取引の状態遷移と与信判定の記録を監査証跡として保存します。入金指示の生成と手数料計算は分離し、返金処理との整合を毎回検証します。";
    let related = "支払いフローでは取引ごとに与信を確認し、精算処理で確定します。返金は状態遷移の一部として扱います。与信の判定は決済基盤が担い、失敗時は取引を中断します。";
    let unrelated = "画面の配色とタイポグラフィの指定です。余白の取り方と角丸の寸法を定めます。アイコンの形状と余白の比率もここで管理します。";
    let sources = [
        (
            "design/pay/README.md",
            format!("# 決済設計\n\n{readme_body}\n"),
        ),
        (
            "design/pay/flow.md",
            format!("# 支払いフロー\n\n{related}\n"),
        ),
        ("design/pay/theme.md", format!("# テーマ\n\n{unrelated}\n")),
    ];
    let found = run_corpus_rules(&sources)?;
    let mismatched: Vec<_> = found
        .iter()
        .filter(|(id, _, _)| id == "relation/dir-mismatch")
        .collect();
    assert_eq!(
        mismatched.len(),
        1,
        "整合が低い文書のみ報告するはず: {found:?}"
    );
    assert!(
        mismatched[0].1.contains("theme.md"),
        "無関係な文書を指すはず"
    );
    assert!(
        mismatched[0].2.contains("README"),
        "ヘルプが README への言及を含むはず: {}",
        mismatched[0].2
    );
    Ok(())
}

#[test]
fn enumerated_directory_is_not_anchored() -> anyhow::Result<()> {
    // 番号プレフィックスのファイルが過半を占める列挙型ディレクトリ(ADR 等)は
    // 兄弟ピアであり、README(全体の索引)との用語整合で評価しない
    let readme = "このディレクトリでは製品の意思決定記録の運用について説明します。各記録は採用した判断の背景と根拠と結果を保存し、将来の見直しの材料にします。記録は番号順に追記され、一度確定した内容は原則として撤回せず、変更が必要な場合は新規の記録で上書きします。";
    let sources = vec![
        (
            "decisions/README.md",
            format!("# 意思決定記録\n\n{readme}{readme}\n"),
        ),
        (
            "decisions/0001-first.md",
            "# 判断1\n\n音声認識エンジンの選定と精度評価について記録します。マイク配列の構成を定めます。\n".to_string(),
        ),
        (
            "decisions/0002-second.md",
            "# 判断2\n\nデータ保存の方式と暗号化の方針について記録します。バックアップの頻度を定めます。\n".to_string(),
        ),
        (
            "decisions/0003-third.md",
            "# 判断3\n\n通知の配信経路と再試行の方針について記録します。失敗時の扱いを定めます。\n".to_string(),
        ),
    ];
    let found = run_corpus_rules(&sources)?;
    assert!(
        !found.iter().any(|(id, _, _)| id == "relation/dir-mismatch"),
        "列挙型ディレクトリは報告しないはず: {found:?}"
    );
    Ok(())
}

#[test]
fn short_index_readme_is_not_an_anchor() -> anyhow::Result<()> {
    // 短い README(ファイル索引程度)は役割記述として不十分なため報告しない
    let unrelated = "画面の配色とタイポグラフィの指定です。余白の取り方と角丸の寸法を定めます。";
    let sources = [
        (
            "design/README.md",
            "# 索引\n\n設計文書の一覧です。\n".to_string(),
        ),
        ("design/theme.md", format!("# テーマ\n\n{unrelated}\n")),
    ];
    let found = run_corpus_rules(&sources)?;
    assert!(
        !found.iter().any(|(id, _, _)| id == "relation/dir-mismatch"),
        "索引 README はアンカーにしないはず: {found:?}"
    );
    Ok(())
}

#[test]
fn dir_mismatch_requires_directory_readme() -> anyhow::Result<()> {
    // README のないディレクトリは役割アンカーがなく報告しない
    let unrelated = "画面の配色とタイポグラフィの指定です。余白の取り方と角丸の寸法を定めます。";
    let sources = [
        ("design/a.md", format!("# A\n\n{unrelated}\n")),
        ("design/b.md", format!("# B\n\n{unrelated}\n")),
    ];
    let found = run_corpus_rules(&sources)?;
    assert!(
        !found.iter().any(|(id, _, _)| id == "relation/dir-mismatch"),
        "README がなければ報告しないはず: {found:?}"
    );
    Ok(())
}

#[test]
fn distinct_documents_are_not_flagged_as_duplicates() -> anyhow::Result<()> {
    let sources = [
        ("docs/README.md", format!("# 概要\n\n{BODY}\n")),
        ("docs/cli.md", format!("# CLI\n\n{OTHER_BODY}\n")),
    ];
    let found = run_corpus_rules(&sources)?;
    assert!(
        !found
            .iter()
            .any(|(id, _, _)| id == "relation/cross-doc-duplication"),
        "内容が異なる文書は報告しないはず: {found:?}"
    );
    assert!(
        !found
            .iter()
            .any(|(id, _, _)| id == "merge/consolidation-plan"),
        "重複がない文書に統合提案は出ないはず: {found:?}"
    );
    Ok(())
}

/// U+4E00 から連続する漢字 len 文字(#34 の回帰 fixture 用)。
/// bigram 数が独立に計算できるよう、意味を持たない漢字列にする。
fn kanji_seq(start: usize, len: usize) -> String {
    (start..start + len)
        .map(|i| char::from_u32(0x4E00 + i as u32).expect("CJK 統合漢字の範囲内"))
        .collect()
}

/// A–B・B–C だけが類似する連鎖グループで、A–C へ別辺の類似度を転用しない
/// ことの回帰試験(#34)。
/// B = U+4E00 から 101 字、A = B の先頭 71 字、C = B の 31 字目以降 71 字。
/// 文字 bigram は A=70・B=100・C=70 個で、J(A,B)=0.7, J(B,C)=0.7,
/// J(A,C)=0.4。閾値 0.65 では (A,C) は検出されず連結だけでグループに入る。
fn chained_group_sources() -> [(&'static str, String); 3] {
    let b = kanji_seq(0, 101);
    let a: String = b.chars().take(71).collect();
    let c: String = b.chars().skip(30).collect();
    [
        ("docs/a.md", format!("# A\n\n{a}\n")),
        ("docs/b.md", format!("# B\n\n{b}\n")),
        // README は役割スコアが高く、既定(被リンク 0・recency 0)で正候補に固定される
        ("docs/README.md", format!("# C\n\n{c}\n")),
    ]
}

fn plan_diagnostic<'a>(diags: &'a [Diagnostic], file: &str) -> &'a Diagnostic {
    diags
        .iter()
        .find(|d| d.rule_id == "merge/consolidation-plan" && d.file == file)
        .unwrap_or_else(|| panic!("{file} 向けの統合提案があるはず: {diags:?}"))
}

fn similarity_of(diag: &Diagnostic) -> f64 {
    diag.measurements
        .iter()
        .find(|m| m.metric == "similarity")
        .and_then(|m| m.value)
        .unwrap_or_else(|| panic!("similarity の測定値があるはず: {diag:?}"))
}

#[test]
fn consolidation_plan_uses_direct_similarity_not_borrowed() -> anyhow::Result<()> {
    let diags = collect_corpus_diagnostics(&chained_group_sources())?;
    let for_a = plan_diagnostic(&diags, "docs/a.md");
    assert!(
        (similarity_of(for_a) - 0.4).abs() < 1e-9,
        "A–C の直接類似度 0.4 を出すはず(A–B の 0.7 を転用しない): {for_a:?}"
    );
    assert!(
        for_a
            .measurements
            .iter()
            .any(|m| m.metric == "similarity" && m.method == Some("bigram-jaccard")),
        "直接計算した値は採点方式が区別できるはず: {for_a:?}"
    );
    let for_b = plan_diagnostic(&diags, "docs/b.md");
    assert!(
        (similarity_of(for_b) - 0.7).abs() < 1e-9,
        "B–C は検出済みペアの 0.7 を出すはず: {for_b:?}"
    );
    // 少数文書では候補列挙は全ペア採点なので、直接ペア側も SimHash を
    // 名乗らない(実際に動いた方式だけを計測方式として記録する)
    assert!(
        for_b
            .measurements
            .iter()
            .any(|m| m.metric == "similarity" && m.method == Some("bigram-jaccard")),
        "全ペア採点では直接ペアも bigram-jaccard と名乗るはず: {for_b:?}"
    );
    Ok(())
}

#[test]
fn consolidation_plan_does_not_estimate_document_wide_reduction() -> anyhow::Result<()> {
    // 本文が同一でコード例だけ異なる文書でも、文書全体の削減量は出さない(#34)。
    // 本文リーフだけが比較対象なので類似度は 1.0 になり、従来なら
    // token_count*similarity で文書全体の削減と誤読できる値が出ていた
    let code_a = "```rust\nfn alpha() {}\n```";
    let code_b = "```rust\nfn completely_different_implementation() {}\n```";
    let sources = [
        ("docs/README.md", format!("# 概要\n\n{BODY}\n\n{code_a}\n")),
        ("docs/detail.md", format!("# 詳細\n\n{BODY}\n\n{code_b}\n")),
    ];
    let diags = collect_corpus_diagnostics(&sources)?;
    let diag = plan_diagnostic(&diags, "docs/detail.md");
    let reduction = diag
        .measurements
        .iter()
        .find(|m| m.metric == "estimated_token_reduction")
        .expect("estimated_token_reduction は指標として残すはず");
    assert_eq!(
        reduction.status,
        MeasurementStatus::Unavailable,
        "文書全体の削減量は unavailable にするはず: {reduction:?}"
    );
    assert!(reduction.value.is_none(), "未計測に値を載せないはず");
    assert!(
        reduction.reason.as_deref().is_some_and(|r| !r.is_empty()),
        "未計測の理由を示すはず: {reduction:?}"
    );
    Ok(())
}

#[test]
fn consolidation_plan_ignores_mtime_by_default() -> anyhow::Result<()> {
    // recency 重みの既定は 0 で mtime を入力に使わない。内容と設定を固定して
    // mtime だけを変えても正候補・canonical_score は変わらない(#34)
    let sources = chained_group_sources();
    let diags_default = collect_corpus_diagnostics(&sources)?;
    // A を最新・C(README)を最古にしても既定では結果が変わらない
    let mtimes = [
        SystemTime::UNIX_EPOCH + Duration::from_secs(3_000_000),
        SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000),
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000),
    ];
    let diags_shifted = collect_corpus_diagnostics_with(&sources, &mtimes, MergeConfig::default())?;
    // グループ内の診断列挙順はマップ反復順に依存し得るため、ファイル単位で比較する
    for file in ["docs/a.md", "docs/b.md"] {
        let before = plan_diagnostic(&diags_default, file);
        let after = plan_diagnostic(&diags_shifted, file);
        assert_eq!(
            before.observation, after.observation,
            "mtime だけの変化で正候補が変わらないはず({file})"
        );
        assert_eq!(
            before
                .measurements
                .iter()
                .map(|m| (m.metric, m.status, m.value))
                .collect::<Vec<_>>(),
            after
                .measurements
                .iter()
                .map(|m| (m.metric, m.status, m.value))
                .collect::<Vec<_>>(),
            "mtime だけの変化で測定値が変わらないはず({file})"
        );
    }
    Ok(())
}

#[test]
fn consolidation_plan_recency_opt_in_can_change_candidate() -> anyhow::Result<()> {
    // recency を opt-in(>0)にした場合だけ mtime がスコアへ効く(#34)。
    // A を最新・C(README)を最古にすると正候補が C から A に変わる
    let sources = chained_group_sources();
    let mtimes = [
        SystemTime::UNIX_EPOCH + Duration::from_secs(3_000_000),
        SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000),
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000),
    ];
    let mut merge = MergeConfig::default();
    merge.weights.recency = 1.0;
    let diags = collect_corpus_diagnostics_with(&sources, &mtimes, merge)?;
    let for_b = plan_diagnostic(&diags, "docs/b.md");
    assert!(
        for_b.observation.contains("正候補: docs/a.md"),
        "recency 有効時は最新の A が正候補になるはず: {}",
        for_b.observation
    );
    // C–A は直接ペアではないため、転用でなく直接計算した 0.4 が出る
    let for_c = plan_diagnostic(&diags, "docs/README.md");
    assert!(
        (similarity_of(for_c) - 0.4).abs() < 1e-9,
        "C–A の直接類似度 0.4 を出すはず: {for_c:?}"
    );
    Ok(())
}

#[test]
fn mixed_relative_and_absolute_roots_do_not_false_orphan() -> anyhow::Result<()> {
    // 相対ルートと絶対ルートを混ぜた指定で、リンクされている文書が
    // orphan として誤検出されないことを検証する(#43)
    let base = std::env::temp_dir().join("jaio-mixed-roots-test");
    let rel_docs = base.join("rel").join("docs");
    let abs_other = base.join("abs").join("other");
    fs::create_dir_all(&rel_docs)?;
    fs::create_dir_all(&abs_other)?;
    // 相対側の文書から絶対側の文書へリンクする
    fs::write(
        rel_docs.join("linker.md"),
        "# リンク元\n\n[相手](../../abs/other/target.md)へのリンクです。\n",
    )?;
    fs::write(
        abs_other.join("target.md"),
        "# リンク先\n\n参照される文書です。\n",
    )?;

    let mut docs = Vec::new();
    for (path, source) in [
        (
            rel_docs.join("linker.md"),
            fs::read_to_string(rel_docs.join("linker.md"))?,
        ),
        (
            abs_other.join("target.md"),
            fs::read_to_string(abs_other.join("target.md"))?,
        ),
    ] {
        docs.push(Doc::parse(path, source).0);
    }
    // 相対パスと絶対パスが混在する状態を再現
    let empty_bodies: Vec<String> = vec![String::new(), String::new()];
    let corpus = Corpus::new(
        &docs,
        &empty_bodies,
        &[],
        &[SystemTime::UNIX_EPOCH; 2],
        MergeConfig::default(),
        DetectionConfig::default(),
        HashSet::new(),
        &[],
        &[],
    )?;
    let mut orphan_files = Vec::new();
    for rule in default_corpus_rules(&Config::default(), &[]) {
        if rule.id() != "relation/orphan" {
            continue;
        }
        for d in rule.run(&corpus)?.diagnostics {
            orphan_files.push(d.file.clone());
        }
    }
    // target.md は linker.md からリンクされている → orphan ではない
    assert!(
        !orphan_files.iter().any(|f| f.contains("target")),
        "リンクされている文書が orphan になるのは誤検出: {orphan_files:?}"
    );
    fs::remove_dir_all(&base).ok();
    Ok(())
}

/// `Corpus::new` は bodies と docs の件数不一致で panic せずエラーを返す(#44)。
/// 公開 API が呼び出し側の不整合でプロセスを落とさないことを固定する
#[test]
fn corpus_new_rejects_mismatched_bodies() {
    let (doc, _leaves, _headings) = Doc::parse(PathBuf::from("a.md"), "# 題名\n".to_string());
    let docs = vec![doc];
    let bodies: Vec<String> = Vec::new(); // docs と不一致
    let result = Corpus::new(
        &docs,
        &bodies,
        &[],
        &[],
        MergeConfig::default(),
        DetectionConfig::default(),
        HashSet::new(),
        &[],
        &[],
    );
    assert!(result.is_err(), "件数不一致は panic ではなく Err のはず");
}

/// MoveIntegrityRule を [verification.moves] 宣言付きで実行する(#56 検証用)。
/// 宣言ペアは (移動先, 移動元)。`bases` は宣言パスの解決基準候補(解析ルート相当)、
/// `toml_path` は実効 jaio.toml の位置(宣言パス解決の第 1 基準、#73)
fn run_move_integrity(
    sources: &[(&str, String)],
    moves: &[(&str, &str)],
    bases: &[std::path::PathBuf],
    toml_path: Option<&std::path::Path>,
) -> anyhow::Result<Vec<Diagnostic>> {
    let mut docs = Vec::new();
    let mut bodies = Vec::new();
    let mut tokens = Vec::new();
    for (name, source) in sources {
        let (doc, leaves, _) = Doc::parse(PathBuf::from(*name), source.clone());
        let mut body = String::new();
        for leaf in leaves.iter().filter(|l| DocContext::is_body_leaf(l)) {
            body.push_str(&normalize_text(&leaf.text));
        }
        bodies.push(body);
        tokens.push(100);
        docs.push(doc);
    }
    let mtimes = vec![SystemTime::UNIX_EPOCH; docs.len()];
    let corpus = Corpus::new(
        &docs,
        &bodies,
        &tokens,
        &mtimes,
        MergeConfig::default(),
        DetectionConfig::default(),
        HashSet::new(),
        &[],
        &[],
    )?;
    let mut toml = String::from("[verification.moves]\n");
    for (dest, source) in moves {
        toml.push_str(&format!("\"{dest}\" = \"{source}\"\n"));
    }
    let mut config = Config::from_toml(&toml)?;
    if let Some(path) = toml_path {
        config.source_path = Some(path.to_path_buf());
    }
    let mut found = Vec::new();
    for rule in default_corpus_rules(&config, bases) {
        if rule.id() != "relation/move-integrity" {
            continue;
        }
        found.append(&mut rule.run(&corpus)?.diagnostics);
    }
    Ok(found)
}

/// 移動宣言のある文書移動で、未保全行が warn で報告される(#56)
#[test]
fn move_integrity_reports_lost_lines() -> anyhow::Result<()> {
    let sources = [
        (
            "old/location.md",
            "# 手順\n\n一行目の説明です。\n二行目の説明です。\n".to_string(),
        ),
        (
            "new/location.md",
            "### 手順\n\n一行目の説明です。\n".to_string(),
        ),
    ];
    let found = run_move_integrity(
        &sources,
        &[("new/location.md", "old/location.md")],
        &[],
        None,
    )?;
    assert_eq!(found.len(), 1, "{found:?}");
    let diag = &found[0];
    assert_eq!(diag.severity, Severity::Warn);
    // 診断は移動先(修正すべき側)に出る
    assert_eq!(diag.file, "new/location.md");
    assert!(
        diag.observation.contains("保全されていない行が 1 行"),
        "{}",
        diag.observation
    );
    // 位置根拠は移動元の失われた行
    assert_eq!(diag.locations.len(), 1);
    assert_eq!(diag.locations[0].file, "old/location.md");
    assert_eq!(diag.locations[0].span.line, 4);
    Ok(())
}

/// コードブロック内のコメント・文字列の変更は正規化で吸収されず
/// 未保全として報告される(#66)。同じ正規化を verify-move と共有する
#[test]
fn move_integrity_detects_code_block_changes() -> anyhow::Result<()> {
    let sources = [
        (
            "old/doc.md",
            "# 手順\n\n```js\nconst rule = \"[a](b)\";\n```\n\n本文です。\n".to_string(),
        ),
        (
            "new/doc.md",
            "## 手順\n\n```js\nconst rule = \"a\";\n```\n\n本文です。\n".to_string(),
        ),
    ];
    let found = run_move_integrity(&sources, &[("new/doc.md", "old/doc.md")], &[], None)?;
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].observation.contains("保全されていない行が 1 行"),
        "コード内の文字列変更は未保全のはず: {}",
        found[0].observation
    );
    Ok(())
}

/// 完全保全なら診断が出ない(#56)
#[test]
fn move_integrity_full_preservation_is_silent() -> anyhow::Result<()> {
    let sources = [
        (
            "old/location.md",
            "# 手順\n\n一行目の説明です。\n".to_string(),
        ),
        (
            "new/location.md",
            "### 手順\n\n一行目の説明です。\n".to_string(),
        ),
    ];
    let found = run_move_integrity(
        &sources,
        &[("new/location.md", "old/location.md")],
        &[],
        None,
    )?;
    assert!(found.is_empty(), "{found:?}");
    Ok(())
}

/// 移動元が解析対象から消えた宣言は残骸として info(#56)
#[test]
fn move_integrity_stale_declaration_is_info() -> anyhow::Result<()> {
    let sources = [(
        "new/location.md",
        "# 手順\n\n一行目の説明です。\n".to_string(),
    )];
    let found = run_move_integrity(
        &sources,
        &[("new/location.md", "old/location.md")],
        &[],
        None,
    )?;
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].severity, Severity::Info);
    assert!(
        found[0]
            .observation
            .contains("移動元 old/location.md が解析対象にありません"),
        "{}",
        found[0].observation
    );
    Ok(())
}

/// 移動先が解析対象にない宣言は warn(typo・対象外の疑い)(#56)
#[test]
fn move_integrity_missing_destination_is_warn() -> anyhow::Result<()> {
    let sources = [(
        "old/location.md",
        "# 手順\n\n一行目の説明です。\n".to_string(),
    )];
    let found = run_move_integrity(
        &sources,
        &[("new/location.md", "old/location.md")],
        &[],
        None,
    )?;
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].severity, Severity::Warn);
    assert!(
        found[0]
            .observation
            .contains("移動先 new/location.md が解析対象にありません"),
        "{}",
        found[0].observation
    );
    Ok(())
}

/// 宣言パスの解決が cwd ではなく基準ディレクトリから行われる(#73)。
/// 絶対パスのドキュメント(実行 cwd の外)+ 相対宣言の組み合わせで、
/// 基準が無ければ誤警告になり、jaio.toml の位置・解析ルートのいずれかの
/// 基準があれば正常に照合される
#[test]
fn move_integrity_resolves_declared_paths_from_bases() -> anyhow::Result<()> {
    // テスト cwd(リポジトリルート)とは別の一時ディレクトリに移動元・先を置く
    let dir = std::env::temp_dir().join(format!("jaio-move-bases-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let body = "# 手順\n\n一行目の説明です。\n二行目の説明です。\n";
    std::fs::write(dir.join("old.md"), body)?;
    std::fs::write(dir.join("new.md"), body)?;
    let old_path = dir.join("old.md").display().to_string();
    let new_path = dir.join("new.md").display().to_string();
    let sources = [
        (old_path.as_str(), body.to_string()),
        (new_path.as_str(), body.to_string()),
    ];

    // 基準なし(cwd のみ)は従来の誤警告: cwd に new.md は存在しない
    let found = run_move_integrity(&sources, &[("new.md", "old.md")], &[], None)?;
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].severity, Severity::Warn);
    assert!(
        found[0]
            .observation
            .contains("移動先 new.md が解析対象にありません"),
        "{}",
        found[0].observation
    );

    // 基準 ① jaio.toml の配置ディレクトリ: 完全保全として沈黙する
    let toml = dir.join("jaio.toml");
    std::fs::write(&toml, "")?;
    let found = run_move_integrity(&sources, &[("new.md", "old.md")], &[], Some(&toml))?;
    assert!(found.is_empty(), "{found:?}");

    // 基準 ② 解析ルート: 同様に沈黙する
    let found = run_move_integrity(
        &sources,
        &[("new.md", "old.md")],
        std::slice::from_ref(&dir),
        None,
    )?;
    assert!(found.is_empty(), "{found:?}");

    std::fs::remove_dir_all(&dir)?;
    Ok(())
}
