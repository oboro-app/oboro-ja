//! relation カテゴリ: リポジトリ横断の関係(設計書 §5.4)。
//! リンクグラフ(ファイル参照集合)と SimHash 重複検出に基づくルールを提供する。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context as _, Result};
use jaio_core::comments::CommentFile;
use jaio_core::config::MergeConfig;
use jaio_core::{Diagnostic, Location, Measurement, MeasurementStatus, Severity, Span};
use jaio_index::{
    PairSearchMethod, bigram_hashes, bigram_jaccard_parts, candidate_pairs,
    containment_parts_sorted, jaccard_parts_sorted, jaccard_sorted, simhash,
};
use jaio_parse::doc::{Doc, LinkKind};
use rayon::prelude::*;

use crate::{CorpusRule, Coverage, RuleRun, SectionSlice, substantive_lines};

/// リポジトリ横断解析の入力。解析済みドキュメント列(§6.1)。
pub struct Corpus<'a> {
    /// 対象ドキュメント列
    pub docs: &'a [Doc],
    /// 各ドキュメントの正規化済み本文(本文リーフのみ。CLI の解析時に構築)
    pub bodies: &'a [String],
    /// 各ドキュメントのトークン数
    pub tokens: &'a [usize],
    /// 各ドキュメントのファイル更新時刻
    pub mtimes: &'a [SystemTime],
    /// 統合提案のパラメータ(jaio.toml の `[merge]`)
    pub merge: MergeConfig,
    /// 検出感度のパラメータ(jaio.toml の `[detection]`)
    pub detection: jaio_core::config::DetectionConfig,
    /// コマンドラインで明示指定されたファイル(`comparison_key` 基準の実体同一性
    /// キー)。orphan 判定から除外する
    pub explicit: HashSet<PathBuf>,
    /// 抽出済みのソースコードコメント(§5.4 の doc-comment-drift の入力)
    pub comments: &'a [CommentFile],
    /// 各ドキュメントのセクション列(重複位置の特定に使う、§5.4)
    pub sections: &'a [Vec<SectionSlice>],
    /// 検出済みの重複ペア(構築時に一度だけ計算し、ルール間で共有する)
    pub duplicates: Vec<DuplicatePair>,
    /// 重複候補の列挙方式と件数の記録(§7.3 の context.candidate_search、#36)
    pub pair_search: PairSearch,
}

/// 重複候補の列挙に関する実行記録(#36)。近似検索での未検出は
/// 「全ペアを調べて類似なし」と同義ではないため、方式を出力に残す。
pub struct PairSearch {
    /// 実際に使った方式(全ペア / SimHash バンディング)
    pub method: PairSearchMethod,
    /// 比較対象となった文書数(本文が最小文字数を満たすもの)
    pub comparable_docs: usize,
    /// 列挙された候補ペア数(= bigram Jaccard を採点したペア数)
    pub candidates: usize,
    /// 採点に使った類似度の下限(報告・統合閾値の低い方)
    pub cutoff: f64,
}

/// 検出済みの重複ペア(ドキュメントインデックス昇順)。
pub struct DuplicatePair {
    /// 片側のドキュメントインデックス(a < b)
    pub a: usize,
    /// もう片側のドキュメントインデックス
    pub b: usize,
    /// bigram Jaccard 類似度
    pub score: f64,
    /// 類似度の分子(bigram 積集合の要素数、#34)
    pub intersection: usize,
    /// 類似度の分母(bigram 和集合の要素数、#34)
    pub union: usize,
}

impl<'a> Corpus<'a> {
    /// Corpus を構築する。重複ペアはここで一度だけ計算し、ルール間で共有する
    /// (設計書 §5.4。SimHash で候補を列挙し、n-gram Jaccard で検証する)。
    /// 候補の下限は報告閾値(`detection.doc_similarity`)と統合閾値
    /// (`merge.min_similarity`)の低い方。各ルールは自身の閾値でさらに絞る
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        docs: &'a [Doc],
        bodies: &'a [String],
        tokens: &'a [usize],
        mtimes: &'a [SystemTime],
        merge: MergeConfig,
        detection: jaio_core::config::DetectionConfig,
        explicit: HashSet<PathBuf>,
        comments: &'a [CommentFile],
        sections: &'a [Vec<SectionSlice>],
    ) -> Result<Self> {
        // bodies は docs と同じ並び・同数が前提(構築時にインデックスで
        // 対応付ける)。公開 API なので呼び出し側の不整合で panic させず
        // エラーにする(#44)。tokens/mtimes/sections はルール側が読む直前に
        // 検証・防御する(consolidation-plan の ensure、sections の get)
        anyhow::ensure!(
            bodies.len() == docs.len(),
            "bodies は docs と同数の本文を持つ必要があります(docs={}, bodies={})",
            docs.len(),
            bodies.len()
        );
        let comparable: Vec<usize> = (0..docs.len())
            .filter(|&i| bodies[i].chars().count() >= detection.doc_body_min_chars)
            .collect();
        // 文書ごとの署名・bigram 列・ペア採点は独立しているため rayon で並列化する
        // (§10。par_iter の collect はインデックス順を保持するので結果は決定的)
        let signatures: Vec<u64> = comparable
            .par_iter()
            .map(|&i| simhash(&bodies[i]))
            .collect();
        // bigram をソート済みハッシュ列として事前計算し、ペア間比較をマージに寄せる
        let fingerprints: Vec<Vec<u64>> = comparable
            .par_iter()
            .map(|&i| bigram_hashes(&bodies[i]))
            .collect();
        // 報告・統合の両ルールが使う候補集合は 2 つの閾値の低い方で切る
        let cutoff = detection.doc_similarity.min(merge.min_similarity);
        // `detection.all_pairs` が真なら規模に関わらず全ペアを採点する
        // (近似検索との参照比較・再現検証用、#36)
        let (method, candidates) = candidate_pairs(&signatures, detection.all_pairs);
        let candidate_count = candidates.len();
        let duplicates: Vec<DuplicatePair> = candidates
            .par_iter()
            .filter_map(|&(x, y)| {
                let (a, b) = (comparable[x], comparable[y]);
                let (intersection, union) =
                    jaccard_parts_sorted(&fingerprints[x], &fingerprints[y]);
                let score = if union == 0 {
                    0.0
                } else {
                    intersection as f64 / union as f64
                };
                (score >= cutoff).then_some(DuplicatePair {
                    a,
                    b,
                    score,
                    intersection,
                    union,
                })
            })
            .collect();
        let pair_search = PairSearch {
            method,
            comparable_docs: comparable.len(),
            candidates: candidate_count,
            cutoff,
        };
        Ok(Self {
            docs,
            bodies,
            tokens,
            mtimes,
            merge,
            detection,
            explicit,
            comments,
            sections,
            duplicates,
            pair_search,
        })
    }
}

/// 近似候補列挙時に重複検出系の診断へ付ける限界注記(#36)。
/// バンディングで候補外になったペアは採点されていないため、
/// 「未検出」は「全ペアを調べて類似なし」と同義ではない
const APPROX_SEARCH_NOTE: &str = "重複候補の列挙は近似検索(SimHash バンディング)です。未検出は「全ペアを調べて類似なし」と同義ではありません(detection.all_pairs で全ペア採点できます)";

impl Corpus<'_> {
    /// 近似検索を使った場合の限界注記。全ペア採点なら None(#36)
    fn approximate_search_note(&self) -> Option<&'static str> {
        (self.pair_search.method == PairSearchMethod::SimHashBanding).then_some(APPROX_SEARCH_NOTE)
    }
}

/// 候補列挙を経て採点された類似度の計測方式識別子(#36)。
/// SimHash バンディングは候補絞り込みに使われたときだけ名乗る。
/// 全ペア採点では SimHash は動かないので bigram-jaccard のみとする
fn scored_similarity_method(corpus: &Corpus<'_>) -> &'static str {
    match corpus.pair_search.method {
        PairSearchMethod::SimHashBanding => "simhash64+bigram-jaccard",
        PairSearchMethod::AllPairs => "bigram-jaccard",
    }
}

/// リンク URL が内部リンク(相対パス)かどうか。外部 URL(`scheme:` 形式)・
/// アンカーのみ・絶対パス(`/` 始まり、Web のルート相混在の恐れがあるため)は
/// 対象外(設計書 §5.4)。
fn is_internal_link(url: &str) -> bool {
    let trimmed = url.trim();
    !(trimmed.is_empty()
        || trimmed.starts_with('/')
        || trimmed.starts_with('#')
        // query-only("?view=full" 等)は現行文書内の表示切替参照であり、
        // 解決先の実在検証対象としない(#68: 候補なしと broken-link の
        // 誤接続を防ぐ。fragment(#始まり)と同じ扱い)
        || trimmed.starts_with('?')
        || has_url_scheme(trimmed))
}

/// `scheme:` 形式のプレフィックスを持つか(RFC 3986 の scheme 規則)。
/// http(s)・mailto だけでなく ftp・tel・vscode: 等も外部リンクとして扱う。
/// `:` より前が英字始まりの scheme 文字(英数字・`+`・`-`・`.`)のみの場合。
/// `:` より先に `/` が来る相対パス("docs/a:b.md" 等)は該当しない。
fn has_url_scheme(url: &str) -> bool {
    let Some(pos) = url.find(':') else {
        return false;
    };
    let mut chars = url[..pos].chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// 内部リンクの解決基準ディレクトリ。通常はリンク元ファイルの親。
/// GitHub の PR/Issue テンプレート(`.github` 配下の文書)は repo ルート基準で
/// 相対リンクを解決してレンダリングするため、その挙動に合わせて
/// `.github` の親(= repo ルート、.github は必ずルート直下に配置される)を
/// 基準にする(#84)
fn link_base(from: &Path) -> PathBuf {
    let parent = from.parent().unwrap_or(Path::new("."));
    // GitHub の PR テンプレート(pull_request_template.md)は PR 本文に展開され、
    // 相対リンクは repo ルート基準で解決される(#84)。それ以外の Markdown
    // (.github 配下の README・CONTRIBUTING 等、GitHub 公式仕様の README 相対
    // リンクを含む)は通常どおりファイル基準である(#86)
    if is_pull_request_template(from) {
        // テンプレートは .github の親(= repo ルート)基準で解決する
        let mut base = PathBuf::new();
        for component in parent.components() {
            if component.as_os_str() == ".github" {
                return base;
            }
            base.push(component.as_os_str());
        }
    }
    parent.to_path_buf()
}

/// from が GitHub が PR テンプレートとして認識する配置か(ファイル名は
/// 大小非区別):
/// - `**/.github/pull_request_template.md`
/// - `**/.github/PULL_REQUEST_TEMPLATE/**.md`
///
/// ルート直下の `pull_request_template.md` はファイル基準とルート基準が
/// 同一の解決になるため判定に含めない。Issue テンプレート(ISSUE_TEMPLATE)
/// のレンダリング文脈は未検証のため含めない(#86)
fn is_pull_request_template(from: &Path) -> bool {
    let lower: Vec<String> = from
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_ascii_lowercase())
        .collect();
    let len = lower.len();
    (len >= 2 && lower[len - 2] == ".github" && lower[len - 1] == "pull_request_template.md")
        || (len >= 3
            && lower[len - 3] == ".github"
            && lower[len - 2] == "pull_request_template"
            && lower[len - 1].ends_with(".md"))
}

/// 内部リンクの解決先候補(§5.4)。URL デコードを行い、拡張子付きリンクはそのまま、
/// 拡張子なしリンクは `.md` / `.mdx` を補完する。画像は拡張子補完をしない
/// (画像 URL に文書拡張子を補うと実在しないパスの誤解決になるため、#74)。
/// 実在チェックは呼び出し側で行う。
fn link_candidates(from: &Path, url: &str, kind: LinkKind) -> Vec<PathBuf> {
    // fragment(#)と query(?)を percent-decode の前に分離する。
    // %3F・%23 は path 内のリテラル文字であり、デコード後に区切り扱いしない(#68)。
    // query-only リンク("?view=full" 等)は空 path となり候補なし=現行文書への参照
    let path_part = url.split(['#', '?']).next().unwrap_or_default();
    let decoded = percent_encoding::percent_decode_str(path_part)
        .decode_utf8_lossy()
        .to_string();
    let decoded = decoded.trim();
    if decoded.is_empty() {
        return Vec::new();
    }
    let base = link_base(from);
    if kind == LinkKind::Image || Path::new(decoded).extension().is_some() {
        vec![normalize(&base.join(decoded))]
    } else {
        vec![
            // ディレクトリ等、拡張子なしで実在するリンク先も有効とする
            normalize(&base.join(decoded)),
            normalize(&base.join(format!("{decoded}.md"))),
            normalize(&base.join(format!("{decoded}.mdx"))),
        ]
    }
}

/// `.` / `..` を簡易解決して正規化する。解決結果が空(例: `./..`)は
/// カレントディレクトリを表すため `.` を返す。
/// `..` が解析ルートを超過する場合は潜ったまま残す(例: `a/../../x.md` → `../x.md`)。
/// 超過分を捨てると解析ルート外へのリンクがルート内ファイルと誤判定されるため。
pub fn normalize(path: &Path) -> PathBuf {
    let mut parts: Vec<std::ffi::OsString> = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => match parts.last().map(|p| p.as_os_str()) {
                // 絶対パスのファイルシステムルートは超えられない
                Some(p) if p == "/" => {}
                // 先行する `..` を打消すと 1 段潜る扱いになるため潜ったまま残す
                Some(p) if p == ".." => parts.push("..".into()),
                Some(_) => {
                    parts.pop();
                }
                // 相対パスで解析ルートを超える `..` は残す(ルート外リンクの誤解決防止)
                None => parts.push("..".into()),
            },
            other => parts.push(other.as_os_str().to_os_string()),
        }
    }
    let normalized: PathBuf = parts.iter().collect();
    if normalized.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        normalized
    }
}

/// リンク切れ(相対パス解決の失敗)を検出する(設計書 §5.4)。
pub struct BrokenLinkRule;

impl CorpusRule for BrokenLinkRule {
    fn id(&self) -> &'static str {
        "relation/broken-link"
    }

    fn description(&self) -> &'static str {
        "Markdown 内リンクのリンク切れ"
    }

    fn run(&self, corpus: &Corpus<'_>) -> Result<RuleRun> {
        let mut diags = Vec::new();
        // 判定単位は内部リンク(#36)
        let mut targets = 0usize;
        for doc in corpus.docs {
            for link in &doc.links {
                if !is_internal_link(&link.url) {
                    continue;
                }
                targets += 1;
                let candidates = link_candidates(&doc.path, &link.url, link.kind);
                // 候補のいずれかが実在すれば有効(ファイル・ディレクトリ両方を許容)
                let resolved = candidates.iter().any(|p| p.exists());
                if !resolved {
                    diags.push(
                        Diagnostic::new(
                            doc.display.clone(),
                            Span {
                                line: link.line,
                                col: 1,
                            },
                            self.id(),
                            format!("リンク切れ: 「{}」", link.url),
                        )
                        .severity(Severity::Warn)
                        .review_question(
                            "リンク先のパスを修正するか、対象ファイルを作成するか判断してください"
                                .to_string(),
                        )
                        .measurement(
                            Measurement::measured("candidate_count", candidates.len() as f64)
                                .unit("count")
                                .scope("link"),
                        ),
                    );
                }
            }
        }
        Ok(RuleRun::complete(diags, targets))
    }
}

/// `[verification.moves]` で宣言された文書移動の行単位保全を検証する
/// (§7.2.1、#56)。`verify-move` と同じ正規化で照合し、check 実行時に
/// 自動で行う。移動元が解析対象から消えていれば宣言の残骸として info、
/// 移動先が無ければ warn(いずれも宣言の棚卸しを促す)
pub struct MoveIntegrityRule {
    /// 移動先パス → 移動元パス(jaio.toml の `[verification.moves]`)
    pub moves: BTreeMap<String, String>,
    /// 宣言パス解決の基準ディレクトリ候補(#73)。jaio.toml の配置ディレクトリ、
    /// 解析ルートの順。先頭から実在チェックし、最初に実在した基準で絶対化する。
    /// いずれにも実在しなければ comparison_key の cwd 基準絶対化に委ねる
    pub path_bases: Vec<PathBuf>,
}

impl MoveIntegrityRule {
    /// 宣言パス(doc 照合と同じ実体同一性キー)へ解決する(#73)。
    /// cwd だけを基準にすると、絶対パス指定の解析(ドキュメント側は
    /// canonicalize される)と宣言側の解決が食い違い誤警告になる
    fn resolve_declared_key(&self, declared: &str) -> PathBuf {
        let path = Path::new(declared);
        if path.is_absolute() {
            return comparison_key(path);
        }
        for base in &self.path_bases {
            let candidate = base.join(path);
            if candidate.exists() {
                return comparison_key(&candidate);
            }
        }
        // どこにも実在しない: 現行互換の cwd 基準絶対化(comparison_key 内)
        comparison_key(path)
    }
}

impl CorpusRule for MoveIntegrityRule {
    fn id(&self) -> &'static str {
        "relation/move-integrity"
    }

    fn description(&self) -> &'static str {
        "移動宣言のある文書の行単位保全(verify-move と同じ正規化)"
    }

    fn run(&self, corpus: &Corpus<'_>) -> Result<RuleRun> {
        if self.moves.is_empty() {
            return Ok(RuleRun::not_applicable(
                "移動宣言([verification.moves])がありません",
            ));
        }
        // 解析対象ドキュメントを実体同一性キーで引けるようにする
        let by_key: HashMap<PathBuf, &Doc> = corpus
            .docs
            .iter()
            .map(|doc| (comparison_key(&doc.path), doc))
            .collect();
        let mut diags = Vec::new();
        for (dest, source) in &self.moves {
            let dest_doc = by_key.get(&self.resolve_declared_key(dest));
            let src_doc = by_key.get(&self.resolve_declared_key(source));
            match (dest_doc, src_doc) {
                (Some(dest_doc), Some(src_doc)) => {
                    // 行照合(§7.2.1 の正規化を共有)。同一行の複数出現は
                    // 回数で判定する(#80: 集合照合だと 2 回出る行の片方の
                    // 欠落が検出できない)
                    let src_lines = substantive_lines(&src_doc.source);
                    let dest_lines = substantive_lines(&dest_doc.source);
                    let mut dest_remaining: std::collections::HashMap<&str, usize> =
                        std::collections::HashMap::new();
                    for (_, text) in &dest_lines {
                        *dest_remaining.entry(text.as_str()).or_default() += 1;
                    }
                    let lost: Vec<&(usize, String)> = src_lines
                        .iter()
                        .filter(|(_, text)| match dest_remaining.get_mut(text.as_str()) {
                            Some(n) if *n > 0 => {
                                *n -= 1;
                                false
                            }
                            _ => true,
                        })
                        .collect();
                    if !lost.is_empty() {
                        let mut diag = Diagnostic::new(
                            dest_doc.display.clone(),
                            Span { line: 1, col: 1 },
                            self.id(),
                            format!(
                                "移動元 {source} から保全されていない行が {} 行あります(実質 {} 行中)",
                                lost.len(),
                                src_lines.len()
                            ),
                        )
                        .severity(Severity::Warn)
                        .review_question(
                            "移動先に欠落した行を補うか、意図的な削除なら移動元を削除して宣言を外してください"
                                .to_string(),
                        )
                        .measurement(
                            Measurement::measured("lost_lines", lost.len() as f64)
                                .unit("count")
                                .scope("document"),
                        )
                        .measurement(
                            Measurement::measured("substantive_lines", src_lines.len() as f64)
                                .unit("count")
                                .scope("document"),
                        );
                        for (line, text) in &lost {
                            diag.locations.push(Location::occurrence(
                                src_doc.display.clone(),
                                Span {
                                    line: *line,
                                    col: 1,
                                },
                                jaio_core::snippet(text, 40),
                            ));
                        }
                        diags.push(diag);
                    }
                }
                (Some(_), None) => {
                    // 移動元が解析対象にない: 移動が完了していれば宣言の残骸
                    diags.push(
                        Diagnostic::new(
                            dest.clone(),
                            Span { line: 1, col: 1 },
                            self.id(),
                            format!(
                                "移動宣言の移動元 {source} が解析対象にありません。移動が完了していれば宣言を削除してください"
                            ),
                        )
                        .severity(Severity::Info)
                        .review_question(
                            "移動元を削除済みなら [verification.moves] から宣言を外すか、対象パスを確認してください"
                                .to_string(),
                        ),
                    );
                }
                (None, _) => {
                    // 移動先が解析対象にない: 宣言の typo か収集対象外
                    diags.push(
                        Diagnostic::new(
                            dest.clone(),
                            Span { line: 1, col: 1 },
                            self.id(),
                            format!("移動宣言の移動先 {dest} が解析対象にありません"),
                        )
                        .severity(Severity::Warn)
                        .review_question(
                            "移動先のパスが正しいか、[paths] の include/exclude で対象外になっていないか確認してください"
                                .to_string(),
                        ),
                    );
                }
            }
        }
        Ok(RuleRun::complete(diags, self.moves.len()))
    }
}

/// 解析対象内で参照されない文書(孤立文書)を検出する(設計書 §5.4)。
/// エントリポイント(`detection.orphan_entry_points`、既定 README.md)と
/// コマンドラインで明示指定したファイルは対象外にする。
pub struct OrphanRule {
    /// 絶対パス文書の相対化基準(実効 jaio.toml の配置ディレクトリ、#76)。
    /// cwd が解析対象の外でもリポジトリ相対のパターンが効くようにする
    pub base: Option<PathBuf>,
}

impl CorpusRule for OrphanRule {
    fn id(&self) -> &'static str {
        "relation/orphan"
    }

    fn description(&self) -> &'static str {
        "解析対象内で参照されない文書(孤立文書)"
    }

    fn run(&self, corpus: &Corpus<'_>) -> Result<RuleRun> {
        // 参照されているファイルの集合を作る。リンク切れの解決候補は参照と
        // みなさない(リンク先が実在しなければ参照は成立しない。broken-link が
        // 報告するリンクで orphan 判定を緩めると同じリンクに矛盾する)
        let mut referenced: HashSet<PathBuf> = HashSet::new();
        for doc in corpus.docs {
            for link in &doc.links {
                // 画像は被リンク集計に数えない(alt テキストの参照はリンク意味では
                // ないため。broken-link の実在検証のみ対象、#74)
                if link.kind == LinkKind::Image {
                    continue;
                }
                if is_internal_link(&link.url) {
                    // 拡張子補完の複数候補は解決優先順位の先頭のみ採用する
                    if let Some(target) = link_candidates(&doc.path, &link.url, link.kind)
                        .into_iter()
                        .find(|p| p.exists())
                    {
                        referenced.insert(comparison_key(&target));
                    }
                }
            }
        }

        // エントリポイントの判定は gitignore 構文のマッチャで行う。
        // `matched_path_or_any_parents` は絶対パスで assert panic するため使わない
        let mut builder = ignore::gitignore::GitignoreBuilder::new(".");
        for pattern in &corpus.detection.orphan_entry_points {
            builder.add_line(None, pattern).with_context(|| {
                format!("detection.orphan_entry_points のパターンが無効です: {pattern}")
            })?;
        }
        let matcher = builder.build()?;

        let mut diags = Vec::new();
        for doc in corpus.docs {
            // gitignore マッチャはルート相対のパスを前提とする。jaio.toml 配置
            // ディレクトリ → cwd の順に相対化して照合する(#76。相対パスも
            // cwd 基準で絶対化するため、解析ルートをサブディレクトリにした
            // 実行でも jaio.toml 基準のパターンが効く)。`matched` は絶対パスで
            // panic しない(`matched_path_or_any_parents` は使わない)。
            // 基準外の絶対パスはルート記号を外して評価する(従来互換)
            let rel = normalize(&jaio_core::relativize_for_match(
                &doc.path,
                self.base.as_deref(),
            ));
            let rel = rel.strip_prefix("/").unwrap_or(&rel);
            let is_entry = matcher.matched(rel, false).is_ignore();
            let doc_key = comparison_key(&doc.path);
            // explicit は canonical キーで持つ(#44: 参照比較と同じ基準。
            // 表記系や symlink 経由の綴り違いで明示指定が効かなくなるのを防ぐ)
            let is_explicit = corpus.explicit.contains(&doc_key);
            if is_entry || is_explicit || referenced.contains(&doc_key) {
                continue;
            }
            diags.push(
                Diagnostic::new(
                    doc.display.clone(),
                    Span { line: 1, col: 1 },
                    self.id(),
                    format!("解析対象内で参照されていない文書です: {}", doc.display),
                )
                .severity(Severity::Info)
                .review_question(
                    "他の文書からリンクするか、統合先として扱うか、削除が適切かを確認してください"
                        .to_string(),
                )
                .measurement(
                    Measurement::measured("inbound_links", 0.0)
                        .unit("count")
                        .scope("document"),
                )
                // 「一切参照されない」ではなく今回の解析範囲内の被リンクが 0、
                // という観察。範囲外からの参照の有無は別途確認する(#34)
                .limitation(
                    "解析範囲内の被リンクのみを数えています。範囲外・外部からの参照は対象外です"
                        .to_string(),
                ),
            );
        }
        // 判定単位は文書(各文書の孤立を判定する、#36)
        Ok(RuleRun::complete(diags, corpus.docs.len()))
    }
}

/// コメントと文書の内容重複を検出する(設計書 §5.4)。
/// 乖離の判定は意味判断が必要なため LLM 委譲とし、決定論的には重複のみ検出する。
pub struct DocCommentDriftRule;

impl CorpusRule for DocCommentDriftRule {
    fn id(&self) -> &'static str {
        "relation/doc-comment-drift"
    }

    fn description(&self) -> &'static str {
        "コメントと文書の内容重複"
    }

    fn run(&self, corpus: &Corpus<'_>) -> Result<RuleRun> {
        // 比較対象となる文書本文の bigram をソート済みハッシュ列として事前計算する
        // (duplicates と同じ最適化、§5.4。コメント × 文書の全ペアで bigram 生成を
        // 繰り返さず、各テキスト 1 回に抑える)
        let doc_fingerprints: Vec<(usize, Vec<u64>)> = corpus
            .bodies
            .par_iter()
            .enumerate()
            .filter(|(_, body)| body.chars().count() >= corpus.detection.doc_body_min_chars)
            .map(|(i, body)| (i, bigram_hashes(body)))
            .collect();
        let mut diags = Vec::new();
        // 判定単位は十分な長さのコメント。比較先の文書本文がなければ判定不能(#36)
        // コメント × 文書の全ペア比較は文書数に比例する(#102)。規模ガード:
        // 比較回数が上限を超えるコーパスではコメント側を先頭から処理し、
        // 上限に達した残りは Coverage.reason に明示して対象外にする
        let mut targets = 0usize;
        let mut measured = 0usize;
        let mut comparisons = 0usize;
        let mut skipped_comments = 0usize;
        for comment in corpus.comments {
            if comment.body.chars().count() < corpus.detection.comment_min_chars {
                continue;
            }
            if comparisons + doc_fingerprints.len() > MAX_DRIFT_COMPARISONS {
                skipped_comments += 1;
                continue;
            }
            targets += 1;
            comparisons += doc_fingerprints.len();
            let comment_fingerprint = bigram_hashes(&comment.body);
            // 最も類似する文書を探す(jaccard_parts_sorted は bigram_jaccard と
            // 同一の Jaccard 係数を返すため、判定結果は変更しない)
            let mut best: Option<(usize, f64, usize, usize)> = None;
            for (i, fingerprint) in &doc_fingerprints {
                let (intersection, union) = jaccard_parts_sorted(&comment_fingerprint, fingerprint);
                let score = if union == 0 {
                    0.0
                } else {
                    intersection as f64 / union as f64
                };
                if best.is_none_or(|(_, s, ..)| score > s) {
                    best = Some((*i, score, intersection, union));
                }
            }
            let Some((i, score, intersection, union)) = best else {
                continue;
            };
            measured += 1;
            if score >= corpus.detection.comment_similarity {
                // 主位置は本文に寄与した先頭コメントの開始行(#44)。
                // 比較相手の文書側は類似箇所を一意に特定できないため近似と明示する
                let mut counterpart =
                    Location::counterpart(corpus.docs[i].display.clone(), Span { line: 1, col: 1 });
                counterpart.approximate = true;
                diags.push(
                    Diagnostic::new(
                        comment.display.clone(),
                        Span {
                            line: comment.first_line,
                            col: 1,
                        },
                        self.id(),
                        format!(
                            "コメントと文書の内容が重複しています: {} と {:.0}% 類似",
                            corpus.docs[i].display,
                            score * 100.0
                        ),
                    )
                    .severity(Severity::Info)
                    .review_question(
                        "コメントを簡潔にするか、詳細を文書側に集約するのが適切か確認してください(最終判断は人間/LLM に委譲)"
                            .to_string(),
                    )
                    .measurement(
                        Measurement::measured("bigram_jaccard", score)
                            .method("bigram-jaccard")
                            .unit("ratio")
                            .ratio(intersection as f64, union as f64)
                            .threshold(corpus.detection.comment_similarity, ">=")
                            .scope("comment_document_pair"),
                    )
                    .location(counterpart)
                    .limitation("語彙類似度は意味的同一性を保証しません(乖離の判断は委譲)。位置はファイル内コメント連結本文の先頭コメント開始行です"),
                );
            }
        }
        let coverage_reason = if skipped_comments > 0 {
            Some(format!(
                "コメント×文書の比較回数が上限({MAX_DRIFT_COMPARISONS})を超えたため、先頭から順に処理し {skipped_comments} 件のコメントを対象外にしました(#102)"
            ))
        } else if targets > 0 && measured == 0 {
            Some("比較対象となる十分な長さの文書本文がありません".to_string())
        } else {
            None
        };
        let coverage = Coverage {
            targets,
            measured,
            reason: coverage_reason,
        };
        Ok(RuleRun {
            diagnostics: diags,
            coverage,
        })
    }
}

/// doc-comment-drift の比較回数上限(コメント × 文書ペア数、#102)。
/// 文書数に比例する全ペア比較が大規模コーパスで支配的になるのを防ぐ。
/// 上限超過分は Coverage.reason に明示して対象外にする
pub(crate) const MAX_DRIFT_COMPARISONS: usize = 500_000;

/// パスの実体同一性キー(#43)。
/// 相対ルートと絶対ルートを混ぜて指定した場合でも、リンク解決結果と文書パスの
/// 表記系の違いで参照済みと判定されないよう、実在パスは canonicalize、
/// 非実在パスは cwd 基準の絶対化で統一する。
/// 収集時の重複排除・orphan の参照/明示指定照合でも同じ基準を使う(#44)
pub fn comparison_key(path: &Path) -> PathBuf {
    if let Ok(canonical) = fs::canonicalize(path) {
        return canonical;
    }
    if path.is_absolute() {
        return normalize(path);
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    normalize(&cwd.join(path))
}

/// README との用語整合が低いとみなす類似度(これ未満で報告)。
/// 実リポジトリでのキャリブレーションに基づき調整する
const DIR_MISMATCH_SIMILARITY: f64 = 0.1;

/// 列挙型ディレクトリと判定する番号プレフィックスファイルの比率のしきい値。
/// ADR(`0001-xxx.md`)や changelog のように番号付きファイルが並ぶディレクトリは
/// 「役割の子文書」ではなく「兄弟ピア」で、README は全体の索引にすぎないため
/// 用語整合の比較対象にしない(実測: 実リポジトリの ADR と決定記録のディレクトリ)
const ENUMERATION_RATIO: f64 = 0.5;

/// ファイル名が番号プレフィックス(ADR 等の規約)かどうか。
fn is_numbered_entry(path: &Path) -> bool {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let digits = stem.chars().take_while(|c| c.is_ascii_digit()).count();
    digits >= 2
        && stem
            .chars()
            .nth(digits)
            .is_some_and(|c| c == '-' || c == '_')
}

/// ディレクトリの役割アンカーとして使う README の最小本文長(正規化後)。
/// 短い README(ファイル索引のみ等)は役割の記述として不十分なため対象外。
/// 実測: 実リポジトリの ADR の索引 READMEをアンカーにすると ADR 各ファイルが
/// 偽陽性になった(ADR は独立した意思決定記録で語彙が重ならない構造)
const DIR_README_MIN_CHARS: usize = 300;

/// ディレクトリ役割と文書内容の不一致を検出する(設計書 §5.4)。
/// 同一ディレクトリの README をディレクトリの役割記述と見なし、本文の用語整合
/// (bigram 類似度)が低い文書を Info で報告する。役割に合致するかの意味判断は
/// LLM 委譲とし、ここは構造化された材料の提示に徹する(設計書 §2)。
/// リポジトリ直下(走査ルート)は対象外とする: 根の README はリポジトリ全体の
/// 概要であり、横断的に無関係な文書との比較が偽陽性になるため
pub struct DirMismatchRule;

impl CorpusRule for DirMismatchRule {
    fn id(&self) -> &'static str {
        "relation/dir-mismatch"
    }

    fn description(&self) -> &'static str {
        "ディレクトリ役割と文書内容の不一致"
    }

    fn run(&self, corpus: &Corpus<'_>) -> Result<RuleRun> {
        // 親ディレクトリごとに、本文が十分ある README を 1 件探す(役割アンカー)
        let mut readme_by_dir: HashMap<PathBuf, usize> = HashMap::new();
        for (i, doc) in corpus.docs.iter().enumerate() {
            let is_readme = doc
                .path
                .file_name()
                .is_some_and(|n| n.eq_ignore_ascii_case("readme.md"));
            if is_readme
                && corpus.bodies[i].chars().count() >= DIR_README_MIN_CHARS
                && let Some(dir) = doc.path.parent()
                && normalize(dir) != Path::new(".")
            {
                readme_by_dir.entry(normalize(dir)).or_insert(i);
            }
        }

        // 列挙型ディレクトリ(番号プレフィックスのファイルが過半)を除外する
        // キーは readme_by_dir と同じく normalize 済みで統一する(ウォーカの
        // パスが `./` 付きで得られるため、生の parent では照合が外れる)
        let mut files_by_dir: HashMap<PathBuf, (usize, usize)> = HashMap::new();
        for doc in corpus.docs {
            if let Some(dir) = doc.path.parent() {
                let entry = files_by_dir.entry(normalize(dir)).or_insert((0, 0));
                entry.0 += 1;
                if is_numbered_entry(&doc.path) {
                    entry.1 += 1;
                }
            }
        }
        let readme_by_dir: HashMap<PathBuf, usize> = readme_by_dir
            .into_iter()
            .filter(|(dir, _)| {
                files_by_dir.get(dir).is_none_or(|&(total, numbered)| {
                    (numbered as f64) < ENUMERATION_RATIO * (total as f64)
                })
            })
            .collect();

        // アンカー README がなければ比較自体が成立しない(#36)
        if readme_by_dir.is_empty() {
            return Ok(RuleRun::not_applicable(
                "役割アンカーとなる十分な長さの README がありません",
            ));
        }

        let mut diags = Vec::new();
        // 判定単位は README のあるディレクトリ内の文書
        let mut targets = 0usize;
        for (i, doc) in corpus.docs.iter().enumerate() {
            let Some(dir) = doc.path.parent() else {
                continue;
            };
            let dir = normalize(dir);
            if dir == Path::new(".") {
                continue;
            }
            let Some(&readme) = readme_by_dir.get(&dir) else {
                continue;
            };
            if readme == i {
                continue;
            }
            if corpus.bodies[i].chars().count() < corpus.detection.doc_body_min_chars {
                continue;
            }
            targets += 1;
            // 包含率(文書の語彙が README 語彙に覆われる割合)。長さ差のある対では
            // Jaccard が構造的に薄くなるため包含率を使う
            let (intersection, denominator) = containment_parts_sorted(
                &bigram_hashes(&corpus.bodies[i]),
                &bigram_hashes(&corpus.bodies[readme]),
            );
            let score = if denominator == 0 {
                0.0
            } else {
                intersection as f64 / denominator as f64
            };
            if score >= DIR_MISMATCH_SIMILARITY {
                continue;
            }
            diags.push(
                Diagnostic::new(
                    doc.display.clone(),
                    Span { line: 1, col: 1 },
                    self.id(),
                    format!(
                        "ディレクトリ「{}」の README との用語整合が低い({score:.2})",
                        dir.display()
                    ),
                )
                .severity(Severity::Info)
                .review_question(
                    "ディレクトリの役割に合う内容か、移動先の検討が適切か確認してください(最終判断は人間/LLM に委譲)"
                        .to_string(),
                )
                .measurement(
                    Measurement::measured("bigram_containment", score)
                        .method("bigram-containment")
                        .unit("ratio")
                        .ratio(intersection as f64, denominator as f64)
                        .threshold(DIR_MISMATCH_SIMILARITY, "<")
                        .scope("document_readme_pair"),
                )
                .location(Location::counterpart(
                    corpus.docs[readme].display.clone(),
                    Span { line: 1, col: 1 },
                ))
                .limitation("語彙の包含率はディレクトリ役割の意味判定を保証しません"),
            );
        }
        Ok(RuleRun::complete(diags, targets))
    }
}

/// 1 文書あたり warn で報告する重複ペアの上限。残りは 1 件の info に集約する。
/// 生成文書のように重複の多いコーパスで診断が爆発するのを防ぐ(設計書 §5.4)
const MAX_DUPLICATION_PAIRS_PER_DOC: usize = 3;

/// 文書間の重複記述を検出する(設計書 §5.4)。
pub struct CrossDocDuplicationRule;
impl CorpusRule for CrossDocDuplicationRule {
    fn id(&self) -> &'static str {
        "relation/cross-doc-duplication"
    }

    fn description(&self) -> &'static str {
        "文書間の重複記述"
    }

    fn run(&self, corpus: &Corpus<'_>) -> Result<RuleRun> {
        let mut diags = Vec::new();
        // 採点済みペアの計測方式識別子。全ペア採点では SimHash は動いていない
        // ため、実際の候補列挙方式に合わせる(#36 の candidate_search と一致)
        let scored_method = scored_similarity_method(corpus);
        // 判定単位は比較可能な文書。2 件未満ではペア比較が成立しない(#36)
        let comparable = corpus.pair_search.comparable_docs;
        let coverage = if comparable < 2 {
            Coverage {
                targets: comparable,
                measured: 0,
                reason: Some(format!("比較可能な文書が {comparable} 件しかありません")),
            }
        } else {
            Coverage {
                targets: comparable,
                measured: comparable,
                reason: None,
            }
        };
        // 報告対象のペアを文書ごとの「相手文書候補」に展開する。ペアは a < b の
        // 向きだが診断は両側の文書それぞれに付ける — 常に a 側になる文書だけ
        // 診断ゼロになる非対称を避けるため(§5.4)。類似度の分子・分母を
        // 診断へ引き渡すためペアへの参照を保持する(#34)。
        // 報告閾値は detection.doc_similarity(統合閾値とは独立、§8)
        let mut partners_by_doc: Vec<Vec<(usize, &DuplicatePair)>> =
            vec![Vec::new(); corpus.docs.len()];
        for pair in &corpus.duplicates {
            if pair.score >= corpus.detection.doc_similarity {
                partners_by_doc[pair.a].push((pair.b, pair));
                partners_by_doc[pair.b].push((pair.a, pair));
            }
        }
        // セクション本文の bigram ハッシュを事前計算し、ペア間比較を
        // ソート済みマージに寄せる(duplicates と同じ最適化、§5.4)。
        // 報告対象ペアがないコーパスでは計算自体を省く
        let section_fingerprints: Vec<Vec<Vec<u64>>> =
            if partners_by_doc.iter().any(|p| !p.is_empty()) {
                corpus
                    .sections
                    .par_iter()
                    .map(|sections| sections.iter().map(|s| bigram_hashes(&s.body)).collect())
                    .collect()
            } else {
                Vec::new()
            };
        for (d, partners) in partners_by_doc.iter_mut().enumerate() {
            if partners.is_empty() {
                continue;
            }
            // 類似度上位 MAX_DUPLICATION_PAIRS_PER_DOC 件のみ warn で報告する。
            // 同点は相手文書のパス昇順で順位を決定論的にする(§5.4)
            partners.sort_by(|x, y| {
                y.1.score
                    .total_cmp(&x.1.score)
                    .then_with(|| corpus.docs[x.0].display.cmp(&corpus.docs[y.0].display))
            });
            let skipped = partners.len().saturating_sub(MAX_DUPLICATION_PAIRS_PER_DOC);
            // 重複位置の特定は warn に残すペアにだけ実行する(ペア数支配の
            // ボトルネックなので、上限から外れるペアのセクション比較は行わない)
            for &(other, pair) in partners.iter().take(MAX_DUPLICATION_PAIRS_PER_DOC) {
                let score = pair.score;
                // 重複位置の特定(§5.4): ペアのセクションをクロス比較し、
                // 最も類似するセクション組を見出し行で報告する。
                // 「この文書」は常に d、「相手」は other。セクションが未構築・
                // 取れない文書は文書先頭のまま扱う
                let empty: Vec<SectionSlice> = Vec::new();
                let sections_self = corpus.sections.get(d).unwrap_or(&empty);
                let sections_other = corpus.sections.get(other).unwrap_or(&empty);
                let fps_self: &[Vec<u64>] =
                    section_fingerprints.get(d).map_or(&[], |v| v.as_slice());
                let fps_other: &[Vec<u64>] = section_fingerprints
                    .get(other)
                    .map_or(&[], |v| v.as_slice());
                let mut best: Option<(f64, &SectionSlice, &SectionSlice)> = None;
                for (i, section_self) in sections_self.iter().enumerate() {
                    for (j, section_other) in sections_other.iter().enumerate() {
                        let Some(fs) = fps_self.get(i) else { continue };
                        let Some(fo) = fps_other.get(j) else { continue };
                        let s = jaccard_sorted(fs, fo);
                        if best.as_ref().is_none_or(|(bs, ..)| s > *bs) {
                            best = Some((s, section_self, section_other));
                        }
                    }
                }
                let (line, target_line, where_note) = match best {
                    Some((_, ss, so)) => (
                        ss.line,
                        so.line,
                        format!(
                            "重複位置: この文書の「{}」と相手の「{}」",
                            ss.title, so.title
                        ),
                    ),
                    None => (1, 1, String::new()),
                };
                let mut diag = Diagnostic::new(
                    corpus.docs[d].display.clone(),
                    Span { line, col: 1 },
                    self.id(),
                    format!(
                        "{} と本文が {:.0}% 類似しています。{}",
                        corpus.docs[other].display,
                        score * 100.0,
                        where_note
                    ),
                )
                .severity(Severity::Warn)
                .review_question(
                    "統合するか、それぞれの役割に分担するのが適切か確認してください(統合提案は merge/consolidation-plan)"
                        .to_string(),
                )
                .measurement(
                    Measurement::measured("similarity", score)
                        .method(scored_method)
                        .unit("ratio")
                        .ratio(pair.intersection as f64, pair.union as f64)
                        .threshold(corpus.detection.doc_similarity, ">=")
                        .scope("document_pair"),
                )
                .location(Location::counterpart(
                    corpus.docs[other].display.clone(),
                    Span {
                        line: target_line,
                        col: 1,
                    },
                ))
                .limitation("語彙類似度は意味的同一性を保証しません");
                if let Some(note) = corpus.approximate_search_note() {
                    diag = diag.limitation(note.to_string());
                }
                diags.push(diag);
            }
            if skipped > 0 {
                let shown = partners.len().min(MAX_DUPLICATION_PAIRS_PER_DOC);
                diags.push(
                    Diagnostic::new(
                        corpus.docs[d].display.clone(),
                        Span { line: 1, col: 1 },
                        self.id(),
                        format!(
                            "類似する文書がほかに {} 件あります(類似度上位 {} 件のみ warn で表示)",
                            skipped, MAX_DUPLICATION_PAIRS_PER_DOC
                        ),
                    )
                    .severity(Severity::Info)
                    .review_question(
                        "jaio stats --similarity でペア一覧を確認し、統合対象を判断してください"
                            .to_string(),
                    )
                    // 表示上限による省略は解析不完全ではないため、reported/omitted を
                    // 別々の測定値として記録する(#36)
                    .measurement(
                        Measurement::measured("reported_pairs", shown as f64)
                            .unit("count")
                            .scope("document"),
                    )
                    .measurement(
                        Measurement::measured("additional_similar_docs", skipped as f64)
                            .unit("count")
                            .scope("document"),
                    )
                    .limitation("warn として報告されなかった類似ペアの位置は個別に示していません"),
                );
            }
        }
        Ok(RuleRun {
            diagnostics: diags,
            coverage,
        })
    }
}

/// 重複グループの統合提案(正候補スコアとトークン削減見積り、設計書 §5.5)。
/// 統合の実行は行わず、判断材料の提示に徹する(設計書 §2 の LLM 委譲)。
pub struct ConsolidationPlanRule;

/// 素朴な union-find(重複ペアのグループ化)。
struct DisjointSet {
    parent: Vec<usize>,
}

impl DisjointSet {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
        }
    }

    fn find(&mut self, x: usize) -> usize {
        if self.parent[x] == x {
            return x;
        }
        let root = self.find(self.parent[x]);
        self.parent[x] = root;
        root
    }

    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.parent[rb] = ra;
        }
    }
}

/// ファイル役割スコア(設計書 §5.5)。README を正候補側に傾ける。
/// README=1.0 / その他=0.3 の 2 値。recency・inbound と同スケール(0〜1)で
/// 重み weights.role(既定 0.8)を掛けて加算する。0.3 は初期
/// ヒューリスティック(実測による調整履歴はなし、#109): README の正候補性を
/// 「被リンク数が同程度のグループで覆せるほどには弱く、ないよりは強く」なる
/// 値として設定している
fn role_score(doc: &Doc) -> f64 {
    if doc
        .path
        .file_name()
        .is_some_and(|n| n.eq_ignore_ascii_case("readme.md"))
    {
        1.0
    } else {
        0.3
    }
}

/// SystemTime をエポック秒の f64 に変換する(新しさの正規化用)。
fn epoch_secs(t: SystemTime) -> f64 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

impl CorpusRule for ConsolidationPlanRule {
    fn id(&self) -> &'static str {
        "merge/consolidation-plan"
    }

    fn description(&self) -> &'static str {
        "重複グループの統合提案(正候補と直接類似度)"
    }

    fn run(&self, corpus: &Corpus<'_>) -> Result<RuleRun> {
        // 統合提案は「報告対象の重複」のうち merge.min_similarity 以上のもの。
        // 報告されていない重複への提案は利用者を混乱させるため、
        // doc_similarity 未満のペアは対象外とする(§8)
        let threshold = corpus
            .merge
            .min_similarity
            .max(corpus.detection.doc_similarity);
        let pairs: Vec<&DuplicatePair> = corpus
            .duplicates
            .iter()
            .filter(|p| p.score >= threshold)
            .collect();
        // 判定単位は閾値を超える重複ペア(#36)。比較可能な文書自体が足りない
        // 場合だけ判定不能とし、ペアが単になかった場合は正常な 0 件とする
        if corpus.pair_search.comparable_docs < 2 {
            return Ok(RuleRun::not_applicable(format!(
                "比較可能な文書が {} 件しかありません",
                corpus.pair_search.comparable_docs
            )));
        }
        if pairs.is_empty() {
            return Ok(RuleRun::complete(Vec::new(), 0));
        }
        // tokens / mtimes も docs と同数が前提(重複がある場合だけ参照する)
        anyhow::ensure!(
            corpus.tokens.len() == corpus.docs.len() && corpus.mtimes.len() == corpus.docs.len(),
            "tokens と mtimes は docs と同数の要素を持つ必要があります"
        );

        // 重複ペアをグループへ統合する(連鎖的な重複も同一グループにまとめる)
        let mut dsu = DisjointSet::new(corpus.docs.len());
        for pair in &pairs {
            dsu.union(pair.a, pair.b);
        }
        let roots: HashSet<usize> = pairs.iter().map(|p| dsu.find(p.a)).collect();
        let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
        for i in 0..corpus.docs.len() {
            let root = dsu.find(i);
            if roots.contains(&root) {
                groups.entry(root).or_default().push(i);
            }
        }

        // 被リンク数(内部リンクの解決先が一致する数。設計書 §5.5)。
        // 自己リンクは被リンクとして数えない(自分自身への参照で正候補スコアを
        // 持ち上げられないようにする)。経路の索引を作り O(文書数^2) の走査を避ける
        let mut by_path: HashMap<PathBuf, usize> = HashMap::new();
        for (j, doc) in corpus.docs.iter().enumerate() {
            by_path.entry(normalize(&doc.path)).or_insert(j);
        }
        let mut inbound = vec![0usize; corpus.docs.len()];
        for (i, doc) in corpus.docs.iter().enumerate() {
            for link in &doc.links {
                // 画像は被リンク(inbound)に数えない(#74)
                if link.kind == LinkKind::Image {
                    continue;
                }
                if !is_internal_link(&link.url) {
                    continue;
                }
                for target in link_candidates(&doc.path, &link.url, link.kind) {
                    if let Some(&j) = by_path.get(&target)
                        && i != j
                    {
                        inbound[j] += 1;
                        break;
                    }
                }
            }
        }

        let weights = &corpus.merge.weights;
        // 直接ペアの計測方式識別子は実際の候補列挙方式に合わせる
        let scored_method = scored_similarity_method(corpus);
        let mut diags = Vec::new();
        for members in groups.values() {
            if members.len() < 2 {
                continue;
            }
            // 正候補スコア = 被リンク数・新しさ・役割の重み付き和(設計書 §5.5)
            let times: Vec<f64> = members
                .iter()
                .map(|&i| epoch_secs(corpus.mtimes[i]))
                .collect();
            let min_t = times.iter().copied().fold(f64::INFINITY, f64::min);
            let max_t = times.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            let max_in = members.iter().map(|&i| inbound[i]).max().unwrap_or(0) as f64;
            let score_of = |pos: usize| -> f64 {
                let recency = if max_t > min_t {
                    (times[pos] - min_t) / (max_t - min_t)
                } else {
                    1.0
                };
                let inbound_norm = if max_in > 0.0 {
                    inbound[members[pos]] as f64 / max_in
                } else {
                    0.0
                };
                weights.inbound_links * inbound_norm
                    + weights.recency * recency
                    + weights.role * role_score(&corpus.docs[members[pos]])
            };
            // 同点は members 末尾側(文書インデックス=パス順で後方)が選ばれる。
            // members は文書インデックス昇順なので選定は実行間で決定的
            let candidate_pos = (0..members.len())
                .max_by(|&x, &y| score_of(x).total_cmp(&score_of(y)))
                .expect("グループは空でない");
            let candidate = members[candidate_pos];

            for &member in members {
                if member == candidate {
                    continue;
                }
                // 正候補との類似度は直接ペアの採点値を使う。連鎖で繋がっただけで
                // 直接ペアとして採点されていない組合せは本文から直接計算する。
                // グループ内の別辺の類似度をこの組合せへ転用しない(#34)
                let direct = pairs.iter().find(|p| {
                    (p.a == member && p.b == candidate) || (p.b == member && p.a == candidate)
                });
                let (sim, sim_method, inter, union) = match direct {
                    Some(p) => (p.score, scored_method, p.intersection, p.union),
                    None => {
                        let (inter, union) =
                            bigram_jaccard_parts(&corpus.bodies[member], &corpus.bodies[candidate]);
                        let sim = if union == 0 {
                            0.0
                        } else {
                            inter as f64 / union as f64
                        };
                        (sim, "bigram-jaccard", inter, union)
                    }
                };
                let current = corpus.tokens[member];
                let mut diag = Diagnostic::new(
                    corpus.docs[member].display.clone(),
                    Span { line: 1, col: 1 },
                    self.id(),
                    format!(
                        "{} を統合する候補。正候補: {}(スコア {:.2})",
                        corpus.docs[member].display,
                        corpus.docs[candidate].display,
                        score_of(candidate_pos)
                    ),
                )
                .severity(Severity::Info)
                .review_question(
                    "正候補への統合が適切か確認してください(最終判断は人間/LLM に委譲)".to_string(),
                )
                .measurement(
                    // 被リンク・新しさ・役割の重み付き和。比率ではない(§5.5)
                    Measurement::measured("canonical_score", score_of(candidate_pos))
                        .unit("weighted_score")
                        .scope("merge_candidate"),
                )
                .measurement(
                    Measurement::measured("similarity", sim)
                        .method(sim_method)
                        .unit("ratio")
                        .ratio(inter as f64, union as f64)
                        .threshold(threshold, ">=")
                        .scope("document_pair"),
                )
                .measurement(
                    Measurement::measured("token_count", current as f64)
                        .unit("tokens")
                        .scope("document"),
                )
                // 文書全体のトークン削減量は本文の語彙類似度からは推計できない
                // (対象範囲が一致しないため)。0 や仮の推計で埋めない(#34、§5.5)
                .measurement(Measurement::unmeasured(
                    "estimated_token_reduction",
                    MeasurementStatus::Unavailable,
                    "文書全体のトークン削減量は本文の語彙類似度からは推計できません(対象範囲が一致しません)",
                ))
                .location(Location::counterpart(
                    corpus.docs[candidate].display.clone(),
                    Span { line: 1, col: 1 },
                ));
                if let Some(note) = corpus.approximate_search_note() {
                    diag = diag.limitation(note.to_string());
                }
                diags.push(diag);
            }
        }
        Ok(RuleRun::complete(diags, pairs.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_to_current_dir() {
        assert_eq!(normalize(Path::new("docs/guide/..")), PathBuf::from("docs"));
        // 解析ルートを超える `..` は潜ったまま残す(ルート内ファイルへの誤解決を防ぐ)
        assert_eq!(normalize(Path::new("./..")), PathBuf::from(".."));
        assert_eq!(
            normalize(Path::new("a/../../x.md")),
            PathBuf::from("../x.md")
        );
        assert_eq!(
            normalize(Path::new("../../x.md")),
            PathBuf::from("../../x.md")
        );
        // 絶対パスはファイルシステムルートで止まる
        assert_eq!(normalize(Path::new("/a/../../x")), PathBuf::from("/x"));
    }

    #[test]
    fn internal_link_classification() {
        assert!(is_internal_link("./config.md"));
        assert!(is_internal_link("docs/design.md"));
        assert!(!is_internal_link("https://example.com/"));
        assert!(!is_internal_link("/etc/hosts"));
        assert!(!is_internal_link("#section"));
        assert!(!is_internal_link(""));
        // query-only は現行文書内の表示切替参照(#68: broken-link にしない)
        assert!(!is_internal_link("?view=full"));
        assert!(!is_internal_link("?view=full#setup"));
        // http(s) 以外のスキームも外部リンクとして扱う
        assert!(!is_internal_link("ftp://example.com/f.txt"));
        assert!(!is_internal_link("tel:0120-000-000"));
        assert!(!is_internal_link("vscode://file/src/main.rs"));
        // `:` より先に `/` が来る相対パスは内部リンクのまま
        assert!(is_internal_link("docs/a:b.md"));
    }

    #[test]
    fn resolves_relative_links() {
        let from = Path::new("docs/guide/setup.md");
        // 拡張子付きリンクはそのまま(実在チェックは呼び出し側)
        assert_eq!(
            link_candidates(from, "../design.md", LinkKind::Link),
            vec![PathBuf::from("docs/design.md")]
        );
        // 拡張子なしリンクは素のパス・.md・.mdx を候補にする
        assert_eq!(
            link_candidates(from, "./intro", LinkKind::Link),
            vec![
                PathBuf::from("docs/guide/intro"),
                PathBuf::from("docs/guide/intro.md"),
                PathBuf::from("docs/guide/intro.mdx"),
            ]
        );
        // URL デコードを行う
        assert_eq!(
            link_candidates(from, "my%20file.md", LinkKind::Link),
            vec![PathBuf::from("docs/guide/my file.md")]
        );
        // アンカーのみは候補なし
        assert!(link_candidates(from, "#section", LinkKind::Link).is_empty());
        // 画像は拡張子補完をしない(#74): URL をそのまま解決するのみ
        assert_eq!(
            link_candidates(from, "../images/fig.png", LinkKind::Image),
            vec![PathBuf::from("docs/images/fig.png")]
        );
        assert_eq!(
            link_candidates(from, "./photo", LinkKind::Image),
            vec![PathBuf::from("docs/guide/photo")]
        );
    }

    #[test]
    fn resolves_github_templates_from_repo_root() {
        // #84: GitHub の PR/Issue テンプレートは repo ルート基準で
        // 相対リンクを解決してレンダリングする。jaio も .github の親を基準にする
        let from = Path::new(".github/pull_request_template.md");
        assert_eq!(
            link_candidates(from, "docs/x.md", LinkKind::Link),
            vec![PathBuf::from("docs/x.md")],
            ".github 配下は repo ルート基準(ファイル基準なら .github/docs/x.md)"
        );
        // .github 配下のサブディレクトリ(ISSUE_TEMPLATE 等)は PR テンプレートで
        // ないため通常どおりファイル基準(Issue テンプレートのレンダリング文脈は
        // 未検証のため特別扱いしない、#86)
        let nested = Path::new(".github/ISSUE_TEMPLATE/x.md");
        assert_eq!(
            link_candidates(nested, "docs/x.md", LinkKind::Link),
            vec![PathBuf::from(".github/ISSUE_TEMPLATE/docs/x.md")]
        );
        // 通常の文書は従来どおりファイル基準(#86)
        let normal = Path::new("docs/guide/setup.md");
        assert_eq!(
            link_candidates(normal, "docs/x.md", LinkKind::Link),
            vec![PathBuf::from("docs/guide/docs/x.md")]
        );
    }

    #[test]
    fn github_templates_only_get_repo_root_base() {
        // #86: 「パス中に .github がある」ことだけでルート基準にしてはならない。
        // .github 配下の通常文書(README・CONTRIBUTING 等)は GitHub 公式仕様の
        // とおりファイル基準で解決する
        assert!(is_pull_request_template(Path::new(
            ".github/pull_request_template.md"
        )));
        assert!(is_pull_request_template(Path::new(
            ".github/PULL_REQUEST_TEMPLATE/fix.md"
        )));
        assert!(!is_pull_request_template(Path::new(".github/README.md")));
        assert!(!is_pull_request_template(Path::new(
            ".github/CONTRIBUTING.md"
        )));
        assert!(!is_pull_request_template(Path::new("docs/x.md")));
    }
    #[test]
    fn link_candidates_split_query_and_fragment_before_decoding() {
        let from = Path::new("docs/guide/setup.md");
        // query・fragment はファイル解決から除外する(#68)
        assert_eq!(
            link_candidates(from, "guide.md?view=full#setup", LinkKind::Link),
            vec![PathBuf::from("docs/guide/guide.md")]
        );
        assert_eq!(
            link_candidates(from, "../design?v=1", LinkKind::Link),
            vec![
                PathBuf::from("docs/design"),
                PathBuf::from("docs/design.md"),
                PathBuf::from("docs/design.mdx"),
            ]
        );
        // query-only は現行文書への参照(候補なし)
        assert!(link_candidates(from, "?view=full", LinkKind::Link).is_empty());
        // %3F・%23・%20 は path 内のリテラル文字(分離は decode 前に済んでいる)
        assert_eq!(
            link_candidates(from, "a%3Fb%20c.md", LinkKind::Link),
            vec![PathBuf::from("docs/guide/a?b c.md")]
        );
        assert_eq!(
            link_candidates(from, "a%23b.md", LinkKind::Link),
            vec![PathBuf::from("docs/guide/a#b.md")]
        );
    }
}
