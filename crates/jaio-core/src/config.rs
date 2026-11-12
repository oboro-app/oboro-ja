//! 設定モデル(`jaio.toml`)。設計書 §8 の設定項目を実装する。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

use crate::{Diagnostic, Severity};

/// jaio の設定。未指定の項目は既定値を使う。
/// 未知のキーはエラーにする(linter の設定 typo の黙殺は最悪の失敗形態のため)。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// 形態素解析・トークン計測
    #[serde(default)]
    pub analysis: AnalysisConfig,
    /// 対象パス
    #[serde(default)]
    pub paths: PathsConfig,
    /// コメント抽出
    #[serde(default)]
    pub comments: CommentsConfig,
    /// 重大度制御
    #[serde(default)]
    pub lint: LintConfig,
    /// 統合提案
    #[serde(default)]
    pub merge: MergeConfig,
    /// 検出感度
    #[serde(default)]
    pub detection: DetectionConfig,
    /// 文体(§1.3 の既定規約)
    #[serde(default)]
    pub style: StyleConfig,
    /// 用語ペア
    #[serde(default)]
    pub terms: TermsConfig,
    /// ユーザー拡張パターン
    #[serde(default)]
    pub patterns: PatternsConfig,
    /// 保全検証(§7.2.1 の check 連動)
    #[serde(default)]
    pub verification: VerificationConfig,
    /// 実効設定ファイル(jaio.toml)のパス。`--config`・ファイル探索で読んだ
    /// 場合のみ CLI が設定し、JAIO_CONFIG(インライン)や既定値では None。
    /// `[verification.moves]` の宣言パス解決基準に使う(#73)。
    /// `config_fingerprint` には含めない(内容が同じなら配置場所の違いで
    /// 再現性識別を変えない)
    #[serde(skip)]
    pub source_path: Option<std::path::PathBuf>,
}

/// 形態素解析・トークン計測の設定(`[analysis]`、設計書 §8)。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AnalysisConfig {
    /// lindera の解析モード(§4.2)。`normal` または `decompose`
    #[serde(default)]
    pub mode: AnalysisMode,
    /// トークン計測用エンコーディング
    #[serde(default)]
    pub encoding: TokenEncoding,
    /// 外部の lindera 辞書ディレクトリ(Sudachi スキーマ)。未指定なら埋め込み辞書を使う。
    /// 埋め込み辞書なしでビルドした場合(`--no-default-features`)は必須
    #[serde(default)]
    pub dictionary: Option<String>,
}

/// lindera の解析モード。Sudachi 本家の粒度 A/B/C ではなく
/// lindera の Mode 種別を指す(§4.2)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AnalysisMode {
    /// 通常の Viterbi 最適分割(既定)
    #[default]
    Normal,
    /// 検索用途の分割抑制解除モード(長い複合語を割りやすい)
    Decompose,
}

/// トークン計測用エンコーディング。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
pub enum TokenEncoding {
    /// o200k_base(GPT-4o 系、既定)
    #[default]
    #[serde(rename = "o200k_base")]
    O200kBase,
    /// cl100k_base(GPT-4 系)
    #[serde(rename = "cl100k_base")]
    Cl100kBase,
}

/// コメント抽出の設定(`[comments]`、設計書 §8)。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CommentsConfig {
    /// 抽出対象の言語名(`rust` / `typescript` / `python` / `go`)。
    /// 空なら対応全言語を対象にする
    #[serde(default)]
    pub languages: Vec<String>,
}

/// 用語ペアの設定(`[terms]`、設計書 §5.2)。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TermsConfig {
    /// 揃えたい同義語ペア(`pairs = [["推奨表記", "別表記"], ...]`)。
    /// 要素数が 2 でない・空の表記を含むペアは `validate` でエラーになる
    #[serde(default)]
    pub pairs: Vec<Vec<String>>,
}

/// 文体の設定(`[style]`、設計書 §8)。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StyleConfig {
    /// 文書の既定文体。`polite`(ですます体、§1.3 の既定)または `plain`(である体)。
    /// 非公開の実運用試用でである調中心のリポジトリに info が大量に出る
    /// 観察があり、リポジトリごとの選択を可能にする
    #[serde(default)]
    pub target: StyleTarget,
    /// 文書単位の文体上書き(#55)。パスパターン(gitignore 構文)→ 文体文字列
    /// (`polite` / `plain`)。素材・テンプレート・プロンプト系など、リポジトリ
    /// 既定と異なる文体が正当な文書を構造的に許容する。
    /// frontmatter の `jaio-style` 宣言はこれより優先する。
    /// BTreeMap でキー順を決定的にし、設定ハッシュ(§7.3)を安定させる
    #[serde(default)]
    pub overrides: BTreeMap<String, StyleTarget>,
}

/// 文書の既定文体。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StyleTarget {
    /// ですます体(敬体)。§1.3 の既定
    #[default]
    Polite,
    /// である体(常体)
    Plain,
}

/// `[style.overrides]` をコンパイルしたもの(gitignore 構文 → StyleTarget)。
/// `[paths]` の PathFilter と同じく、glob コンパイルは重いため使い回す
pub struct StyleOverrides {
    matcher: ignore::gitignore::Gitignore,
    /// パターン文字列 → 文体。matched が返す Glob から `original()` で引く
    by_pattern: BTreeMap<String, StyleTarget>,
    /// 絶対パス文書の相対化基準(実効 jaio.toml の配置ディレクトリ、#76)。
    /// None は cwd 基準のみ
    base: Option<PathBuf>,
}

impl Default for StyleOverrides {
    fn default() -> Self {
        Self {
            matcher: ignore::gitignore::Gitignore::empty(),
            by_pattern: BTreeMap::new(),
            base: None,
        }
    }
}

impl StyleOverrides {
    /// 上書きマップをコンパイルする。無効なパターンは設定エラーにする。
    /// `base` は絶対パスの文書をリポジトリ相対パターンと照合するための
    /// 基準ディレクトリ(#76)
    pub fn build(overrides: &BTreeMap<String, StyleTarget>, base: Option<&Path>) -> Result<Self> {
        let mut builder = ignore::gitignore::GitignoreBuilder::new(".");
        let mut by_pattern = BTreeMap::new();
        for (pattern, target) in overrides {
            builder
                .add_line(None, pattern)
                .with_context(|| format!("style.overrides のパターンが無効です: {pattern}"))?;
            by_pattern.insert(pattern.clone(), *target);
        }
        Ok(Self {
            matcher: builder.build()?,
            by_pattern,
            // base は実行中に変わらないため、canonicalize は構築時に 1 回だけ
            // 行う(#106。照合ごとの再実行は数千文書で数千回の無駄な syscall)
            base: base.map(resolved_base),
        })
    }

    /// パスに一致するパターンの文体を返す。一致しなければ None。
    /// 絶対パスは基準ディレクトリ(jaio.toml 親 → cwd)で相対化して照合する
    /// (#76: cwd が解析対象の外でもリポジトリ相対のパターンが効く)
    pub fn get(&self, path: &Path) -> Option<StyleTarget> {
        if self.by_pattern.is_empty() {
            return None;
        }
        let effective = relativize_for_match(path, self.base.as_deref());
        match self.matcher.matched(&effective, false) {
            ignore::Match::Ignore(glob) => self.by_pattern.get(glob.original()).copied(),
            _ => None,
        }
    }
}

/// ユーザー拡張パターンの設定(`[patterns]`、設計書 §8)。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PatternsConfig {
    /// 冗長表現パターン(表現 → 置き換え先)。空の置き換え先は削除候補。
    /// 内蔵辞書と同じ表現を指定した場合はユーザー指定が優先される。
    /// BTreeMap でキー順を決定的にし、設定ハッシュ(§7.3)を安定させる
    #[serde(default)]
    pub redundant: BTreeMap<String, String>,
}

/// 保全検証の設定(`[verification]`、設計書 §8・§7.2.1)。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VerificationConfig {
    /// 文書移動の保全検証宣言(#56)。移動先パス → 移動元パス。
    /// `relation/move-integrity` が check 実行時に行単位の照合を行う。
    /// 移動元を削除したら宣言も削除する(残ると info が出る)。
    /// BTreeMap でキー順を決定的にし、設定ハッシュ(§7.3)を安定させる
    #[serde(default)]
    pub moves: BTreeMap<String, String>,
}

/// 統合提案の設定(`[merge]`、設計書 §8)。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct MergeConfig {
    /// 統合候補とする最低類似度
    pub min_similarity: f64,
    /// 正候補スコアの重み
    pub weights: MergeWeights,
}

impl Default for MergeConfig {
    fn default() -> Self {
        Self {
            // 実運用測定(非公開のリポジトリ)に基づく既定値。0.75 は正当な重複を
            // 見逃すため 0.65 とする(0.5 帯は同一主題の別文書が大量に混入する)
            min_similarity: 0.65,
            weights: MergeWeights::default(),
        }
    }
}

/// 正候補スコアの重み(設計書 §8 の既定値)。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct MergeWeights {
    /// 被リンク数の重み
    pub inbound_links: f64,
    /// 更新の新しさの重み。既定は 0(opt-in)。0 のとき mtime を取得しない
    pub recency: f64,
    /// ファイル役割の重み
    pub role: f64,
}

impl Default for MergeWeights {
    fn default() -> Self {
        Self {
            inbound_links: 1.0,
            recency: 0.0,
            role: 0.8,
        }
    }
}

/// 検出感度の設定(`[detection]`、設計書 §8)。
/// 既定値はチューニング前の基準値。調整は scripts/eval-detection.py で検証して行う。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct DetectionConfig {
    /// 文書重複の比較対象とする本文の最小文字数(正規化後)
    pub doc_body_min_chars: usize,
    /// 文書重複(`relation/cross-doc-duplication`)として報告する類似度。
    /// 統合提案の閾値(`merge.min_similarity`)とは別に持つ
    pub doc_similarity: f64,
    /// doc-comment-drift の対象とするコメント本文の最小文字数(正規化後)
    pub comment_min_chars: usize,
    /// doc-comment-drift の重複判定類似度。
    /// 非公開の実運用測定 (7 リポジトリ試用)では文字 bigram 類似度 0.3 超
    /// がほぼ発生せず、検出対象はコピペレベルの重複に限るため高めに保っている
    pub comment_similarity: f64,
    /// セクション重複判定類似度
    pub section_similarity: f64,
    /// セクション重複の比較対象とする本文の最小文字数(正規化後)
    pub section_body_min_chars: usize,
    /// リスト・引用のネストとして報告する深さ(この深さを超えるもの)
    pub max_nest_depth: u8,
    /// 同一文の重複出現として報告する文の最小文字数(正規化後)
    pub repeat_quote_min_chars: usize,
    /// 長い文として報告する和文文字数(ASCII は読み負荷が軽いため数えない、#48)
    pub long_sentence_min_chars: usize,
    /// 読点を含まない長文として報告する和文文字数(#48)
    pub no_comma_min_chars: usize,
    /// particle が報告する助詞ペア(opt-in、既定は空)。
    /// 実測(#23)で「に/へ」等の自動判定は正当な方向表現の使い分けと
    /// 区別できず大量の誤報になるため、既定では報告しない
    pub particle_pairs: Vec<(String, String)>,
    /// 散文前提ルール(token-density / repeated-quote)の対象とする
    /// 本文に占める段落の割合の下限。リスト主導の文書(課題管理ファイル等)は
    /// 統計と重複判定の前提が崩れるため対象外にする(実測: 実リポジトリの課題一覧の文書)
    pub prose_min_share: f64,
    /// orphan 判定から除くエントリポイント(gitignore 構文)。`/` を含まない
    /// パターンは任意階層のファイル名に一致する。既定は README.md
    pub orphan_entry_points: Vec<String>,
    /// 文書数に関わらず重複候補を全ペア採点する。既定は false で、規模が
    /// `ALL_PAIRS_MAX_DOCS` を超えると近似検索(SimHash バンディング)に
    /// 切り替わる。近似と全ペアの参照比較・再現検証用(#36)
    pub all_pairs: bool,
}

impl Default for DetectionConfig {
    fn default() -> Self {
        Self {
            doc_body_min_chars: 50,
            doc_similarity: 0.65,
            comment_min_chars: 40,
            comment_similarity: 0.6,
            section_similarity: 0.75,
            section_body_min_chars: 30,
            max_nest_depth: 3,
            repeat_quote_min_chars: 30,
            long_sentence_min_chars: 100,
            no_comma_min_chars: 60,
            particle_pairs: Vec::new(),
            prose_min_share: 0.5,
            orphan_entry_points: vec!["README.md".to_string()],
            all_pairs: false,
        }
    }
}

/// 対象パスの設定。
/// `Default` を手書きしているのは `include_hidden` を Rust 側既定でも
/// true に揃えるため(serde の `default_true` との不一致を防ぐ、#36)。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct PathsConfig {
    /// 収集対象の glob(gitignore 構文)。空なら全件対象。
    /// `!` 始まりは include 結果からの除外を表す
    pub include: Vec<String>,
    /// 除外パターン。gitignore 完全準拠(`!` ネガーション・`**`・
    /// ディレクトリ限定 `foo/` 等)。解析ルートからの相対パスで評価する
    pub exclude: Vec<String>,
    /// 隠しエントリ(ドットで始まるファイル・ディレクトリ)を解析対象に
    /// 含める。エージェント運用リポジトリでは `.agents/` 等が主要な文書置き場に
    /// なるため既定は true。`.git` はこの設定によらず常に除外する
    #[serde(default = "default_true")]
    pub include_hidden: bool,
}

impl Default for PathsConfig {
    fn default() -> Self {
        Self {
            include: Vec::new(),
            exclude: Vec::new(),
            include_hidden: true,
        }
    }
}

/// serde 既定値用: true。
fn default_true() -> bool {
    true
}

/// `[paths]` の include/exclude をコンパイルしたマッチャ。
/// `Config::path_filter` で構築して使い回す(glob コンパイルは重いため)。
pub struct PathFilter {
    include: ignore::overrides::Override,
    exclude: ignore::gitignore::Gitignore,
}

impl PathFilter {
    /// 解析ルート相対パスが収集対象か。include が空なら全件、
    /// 非空ならいずれかの glob に一致するものだけが対象になる。
    /// exclude は親ディレクトリの一致も除外する(gitignore 挙動)。
    pub fn allows(&self, path: &Path) -> bool {
        if self.excluded(path) {
            return false;
        }
        !self.include.matched(path, path.is_dir()).is_ignore()
    }

    /// 明示指定されたファイル用の判定。include glob は適用せず
    /// exclude のみ見る(利用者が直接指定したファイルを include が
    /// 黙って捨てるのは不親切なため)。
    pub fn allows_explicit(&self, path: &Path) -> bool {
        !self.excluded(path)
    }

    fn excluded(&self, path: &Path) -> bool {
        self.exclude
            .matched_path_or_any_parents(path, path.is_dir())
            .is_ignore()
    }
}

impl Config {
    /// `[paths]` のパターンをコンパイルする。無効な glob は設定エラーにする。
    pub fn path_filter(&self) -> Result<PathFilter> {
        let mut include = ignore::overrides::OverrideBuilder::new(".");
        for glob in &self.paths.include {
            include
                .add(glob)
                .with_context(|| format!("paths.include の glob が無効です: {glob}"))?;
        }
        let mut exclude = ignore::gitignore::GitignoreBuilder::new(".");
        for pattern in &self.paths.exclude {
            exclude
                .add_line(None, pattern)
                .with_context(|| format!("paths.exclude のパターンが無効です: {pattern}"))?;
        }
        Ok(PathFilter {
            include: include.build()?,
            exclude: exclude.build()?,
        })
    }
}

/// 重大度制御の設定。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LintConfig {
    /// 個別指定がないルールに適用する既定の重大度。未指定ならルール内蔵の
    /// 重大度をそのまま使う(設計書 §8 の `[lint] default`)
    #[serde(default)]
    pub default: Option<SeveritySetting>,
    /// ルール ID ごとの重大度上書き。`allow` で診断を抑制する。
    /// BTreeMap でキー順を決定的にし、設定ハッシュ(§7.3)を安定させる
    #[serde(default)]
    pub rules: BTreeMap<String, SeveritySetting>,
    /// ルール ID ごとの適用除外パス(gitignore 構文、#75)。テンプレート文書の
    /// 見出し枠など「特定パスでは正当」とみなされる診断を抑える。
    /// ルール全体の抑制([lint.rules] の allow)と違い、対象外のパスには
    /// 通常どおり報告される
    #[serde(default)]
    pub rule_excludes: BTreeMap<String, Vec<String>>,
    /// 意図的な揺れなどの許容
    #[serde(default)]
    pub allow: AllowConfig,
}

/// `[lint.rule_excludes]` をコンパイルしたもの(gitignore 構文、#75)。
/// `[style.overrides]` の StyleOverrides と同じく、glob コンパイルは重いため
/// 使い回う
#[derive(Default)]
pub struct RuleExcludes {
    /// ルール ID → 除外パターンのマッチャ
    by_rule: BTreeMap<String, ignore::gitignore::Gitignore>,
    /// 絶対パス診断の相対化基準(実効 jaio.toml の配置ディレクトリ、#76)。
    /// 運用上 PJ ルート。None は cwd 基準のみ
    base: Option<PathBuf>,
}

impl RuleExcludes {
    /// 除外マップをコンパイルする。`base` は絶対パスの診断をリポジトリ相対の
    /// パターンと照合するための基準ディレクトリ(#76)。無効なパターンは
    /// 設定エラーにする
    pub fn build(excludes: &BTreeMap<String, Vec<String>>, base: Option<&Path>) -> Result<Self> {
        let mut by_rule = BTreeMap::new();
        for (rule_id, patterns) in excludes {
            let mut builder = ignore::gitignore::GitignoreBuilder::new(".");
            for pattern in patterns {
                builder.add_line(None, pattern).with_context(|| {
                    format!("lint.rule_excludes のパターンが無効です: {pattern}(rule: {rule_id})")
                })?;
            }
            by_rule.insert(rule_id.clone(), builder.build()?);
        }
        Ok(Self {
            by_rule,
            base: base.map(resolved_base),
        })
    }

    /// ルールの診断が対象パスで除外されるか。絶対パスの診断は
    /// jaio.toml の配置ディレクトリ基準 → cwd 基準の順に相対化して照合する
    /// (#76: cwd 固定だと絶対パス指定の解析で除外が効かない)。
    /// どちらにも属さなければ照合不能として除外しない(安全側)
    pub fn is_suppressed(&self, rule_id: &str, path: &Path) -> bool {
        let Some(matcher) = self.by_rule.get(rule_id) else {
            return false;
        };
        let effective = relativize_for_match(path, self.base.as_deref());
        matches!(matcher.matched(&effective, false), ignore::Match::Ignore(_))
    }
}

/// 許容設定(`[lint.allow]`、設計書 §8)。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AllowConfig {
    /// 許容する表記(表記ゆれ検出でこの表記を含むペアは報告しない)
    #[serde(default)]
    pub terms: Vec<String>,
}

/// 照合基準ディレクトリを実パス表記に解決する(#89)。symlink(/var →
/// /private/var 等)で base と path の表記が分かれると strip_prefix の
/// 字句比較が壊れるため、実在するパスは canonicalize で統一する。
/// 解決できない場合(不在等)は字句解決のみで比べる(安全側)。
/// `relativize_for_match` に渡す base は、構築時にこの関数を通しておく
/// こと(#106: path 側の canonicalize は照合ごとに行われるが base 側は
/// 実行中に変わらないため、解決は構築時に 1 回だけ行う)
pub fn resolved_base(base: &Path) -> PathBuf {
    std::fs::canonicalize(base).unwrap_or_else(|_| normalize_lexical(base))
}

/// パターン系設定(`[style.overrides]`・`[lint.rule_excludes]`・
/// `detection.orphan_entry_points`)の照合用パスを解決する(#76)。
/// 相対パスは cwd 基準と解釈して絶対化した上で、**① 実効 jaio.toml の配置
/// ディレクトリ → ② cwd** の順に相対化する(解析ルートを jaio.toml 配下の
/// サブディレクトリにした実行でも jaio.toml 基準のパターンが一貫して効く)。
/// どちらの基準にも属さなければ元の表記をそのまま返す(呼び出し側の
/// matcher は不一致として扱い、除外・上書きしない安全側になる)。
/// `base` は [`resolved_base`] を通した実パス表記を渡すこと(#106)
pub fn relativize_for_match(path: &Path, base: Option<&Path>) -> PathBuf {
    let cwd = std::env::current_dir().ok();
    let absolute = if path.is_absolute() {
        Some(path.to_path_buf())
    } else {
        cwd.as_ref().map(|c| c.join(path))
    };
    let Some(absolute) = absolute else {
        // cwd が取れない環境では元の表記のまま照合させる
        return path.to_path_buf();
    };
    // cwd 結合で残る `./`・`../` を字句解決する。解決しないと
    // strip_prefix が字句不一致で失敗し、基準剥ぎが効かない(#87)
    let absolute = normalize_lexical(&absolute);
    // macOS の /var → /private/var 等の symlink で base と path の表記が
    // 分かれると strip_prefix の字句比較が壊れ、基準剥ぎが効かなくなる(#89)。
    // 実在するパスは canonicalize で実パス表記に統一してから比較する。
    // 存在しない等で解決できない場合は字句解決のみで比べる(安全側)
    let absolute =
        std::fs::canonicalize(&absolute).unwrap_or_else(|_| normalize_lexical(&absolute));
    // base の実パス解決は構築時(resolved_base)に済ませてある(#106)
    // base(toml_base)を優先的に剥ぎ、次に cwd を剥ぐ(#87: どちらの基準でも
    // 一貫した照合になる)。どちらにも一致しない場合は元表記のまま返す
    if let Some(base) = base
        && let Ok(rel) = absolute.strip_prefix(base)
    {
        return rel.to_path_buf();
    }
    if let Some(cwd) = &cwd
        && let Ok(rel) = absolute.strip_prefix(cwd)
    {
        return rel.to_path_buf();
    }
    path.to_path_buf()
}

/// パスの `.`・`..` を字句解決する(symlink は解決しない)。
/// jaio-core 内は依存を最小にするためローカル実装とする
pub fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if out.as_os_str().is_empty()
                    || out.components().next_back().is_some_and(|c| {
                        matches!(
                            c,
                            std::path::Component::ParentDir | std::path::Component::RootDir
                        )
                    })
                {
                    out.push("..")
                } else {
                    out.pop();
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

/// 重大度の設定値。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SeveritySetting {
    /// error に引き上げる(設計書 §8 の `deny` 表記も許容)
    #[serde(alias = "deny")]
    Error,
    /// warn に設定する
    Warn,
    /// info に落とす
    Info,
    /// 診断を抑制する
    Allow,
}

impl Config {
    /// パターン系設定(`[style.overrides]`・`[lint.rule_excludes]`・
    /// `detection.orphan_entry_points`)の照合・`[verification.moves]` の
    /// 宣言パス解決に使う基準ディレクトリ。実効 jaio.toml の配置ディレクトリ
    /// (運用上 PJ ルート、#73/#76)。ファイルから読んでいない
    /// (JAIO_CONFIG・既定値)場合は None
    pub fn toml_base(&self) -> Option<&Path> {
        self.source_path.as_deref().and_then(Path::parent)
    }

    /// TOML テキストから設定を読む。構文に加えて意味的な検証(閾値の範囲・
    /// 有限性・用語ペアの形)も行い、無効な設定は早期エラーにする(#36)。
    /// ルール ID の既知チェックはルール一覧を持つ CLI 側で行う。
    pub fn from_toml(text: &str) -> Result<Self> {
        let config: Config = toml::from_str(text)?;
        config.validate()?;
        Ok(config)
    }

    /// 設定値の意味的検証。閾値の範囲外・非有限・形の壊れたペアは
    /// 黙って既定値計測に進ませず、ここでエラーにする(#36)。
    pub fn validate(&self) -> Result<()> {
        let check_ratio = |name: &str, value: f64| -> Result<()> {
            anyhow::ensure!(
                value.is_finite() && (0.0..=1.0).contains(&value),
                "{name} は 0.0〜1.0 の有限値にしてください: {value}"
            );
            Ok(())
        };
        check_ratio("detection.doc_similarity", self.detection.doc_similarity)?;
        check_ratio(
            "detection.comment_similarity",
            self.detection.comment_similarity,
        )?;
        check_ratio(
            "detection.section_similarity",
            self.detection.section_similarity,
        )?;
        check_ratio("detection.prose_min_share", self.detection.prose_min_share)?;
        check_ratio("merge.min_similarity", self.merge.min_similarity)?;
        for (name, value) in [
            (
                "merge.weights.inbound_links",
                self.merge.weights.inbound_links,
            ),
            ("merge.weights.recency", self.merge.weights.recency),
            ("merge.weights.role", self.merge.weights.role),
        ] {
            anyhow::ensure!(
                value.is_finite() && value >= 0.0,
                "{name} は 0 以上の有限値にしてください: {value}"
            );
        }
        for pair in &self.terms.pairs {
            anyhow::ensure!(
                pair.len() == 2 && pair.iter().all(|t| !t.is_empty()),
                "terms.pairs の各要素は空でない表記 2 つの組にしてください: {pair:?}"
            );
            // 別表記が推奨表記を含む場合(preferred="サーバ" / other="サーバー"
            // のような逆包含)は、和文に単語境界がないため both の出現を
            // 排他的に数えられず、統計が必ず不正になる(#100)。内蔵ペアは
            // すべて推奨が長い(包含の向きが正)ため、この向きだけを弾く
            let (preferred, other) = (&pair[0], &pair[1]);
            anyhow::ensure!(
                !other.contains(preferred.as_str()),
                "terms.pairs の別表記に推奨表記が含まれています。推奨表記は別表記の部分文字列にできません: {pair:?}"
            );
        }
        for (from, to) in &self.detection.particle_pairs {
            anyhow::ensure!(
                !from.is_empty() && !to.is_empty(),
                "detection.particle_pairs の助詞は空にできません: ({from:?}, {to:?})"
            );
        }
        Ok(())
    }

    /// 診断に重大度設定を適用する。`allow` の診断は除外される。
    /// 個別指定がないルールには `[lint] default` が効く。どちらもなければ
    /// ルール内蔵の重大度をそのまま通す。
    pub fn apply(&self, mut diagnostic: Diagnostic) -> Option<Diagnostic> {
        let Some(setting) = self
            .lint
            .rules
            .get(diagnostic.rule_id)
            .or(self.lint.default.as_ref())
        else {
            return Some(diagnostic);
        };
        match setting {
            SeveritySetting::Allow => None,
            SeveritySetting::Error => {
                diagnostic.severity = Severity::Error;
                Some(diagnostic)
            }
            SeveritySetting::Warn => {
                diagnostic.severity = Severity::Warn;
                Some(diagnostic)
            }
            SeveritySetting::Info => {
                diagnostic.severity = Severity::Info;
                Some(diagnostic)
            }
        }
    }

    /// 除外パターンに一致するか。gitignore 準拠の `path_filter` の簡易版で、
    /// テスト等の単発評価向け。大量ファイルの走査では `path_filter` を
    /// 一度構築して使い回すこと。
    pub fn is_excluded(&self, display_path: &str) -> bool {
        self.path_filter()
            .is_ok_and(|filter| !filter.allows(Path::new(display_path)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Span;

    fn diagnostic(rule_id: &'static str) -> Diagnostic {
        Diagnostic::new(
            "a.md".to_string(),
            Span { line: 1, col: 1 },
            rule_id,
            "m".to_string(),
        )
        .severity(Severity::Warn)
    }

    #[test]
    fn parses_minimal_toml() -> Result<()> {
        let config = Config::from_toml(
            r#"
[paths]
exclude = ["AGENTS.md"]

[lint.rules]
"consistency/particle" = "info"
"#,
        )?;
        assert_eq!(config.paths.exclude, vec!["AGENTS.md"]);
        assert_eq!(
            config.lint.rules.get("consistency/particle"),
            Some(&SeveritySetting::Info)
        );
        Ok(())
    }

    #[test]
    fn empty_toml_uses_defaults() -> Result<()> {
        let config = Config::from_toml("")?;
        assert!(config.paths.exclude.is_empty());
        assert!(config.lint.rules.is_empty());
        Ok(())
    }

    #[test]
    fn terms_reversed_containment_pair_is_rejected() -> Result<()> {
        // 別表記が推奨表記を含む逆包含ペアは、出現を排他的に数えられない
        // ため設定エラーにする(#100)。内蔵ペアと同じ向き(推奨が長い)は有効
        let err = Config::from_toml("[terms]\npairs = [[\"サーバ\", \"サーバー\"]]\n");
        assert!(
            err.is_err(),
            "逆包含ペアはエラーになるはず: {:?}",
            err.err()
        );
        let ok = Config::from_toml("[terms]\npairs = [[\"サーバー\", \"サーバ\"]]\n");
        assert!(ok.is_ok(), "推奨が長いペアは有効のはず: {:?}", ok.err());
        Ok(())
    }

    #[test]
    fn terms_unknown_key_is_rejected() -> Result<()> {
        // [terms] も他のセクションと同じく typo を黙殺しない(設定方針)
        let err = Config::from_toml("[terms]\npair = [[\"a\", \"b\"]]\n");
        assert!(err.is_err(), "未知キー pair はエラーになるはず");
        Ok(())
    }

    #[test]
    fn parses_particle_pairs_opt_in() -> Result<()> {
        // 既定は空(報告しない)。ペア指定で opt-in する(#23)
        let empty = Config::from_toml("[detection]\n")?;
        assert!(empty.detection.particle_pairs.is_empty());
        let enabled = Config::from_toml("[detection]\nparticle_pairs = [[\"に\", \"へ\"]]\n")?;
        assert_eq!(
            enabled.detection.particle_pairs,
            vec![("に".to_string(), "へ".to_string())]
        );
        Ok(())
    }

    #[test]
    fn parses_detection_section() -> Result<()> {
        let config = Config::from_toml(
            r#"
[detection]
doc_body_min_chars = 80
comment_similarity = 0.7
max_nest_depth = 5
"#,
        )?;
        assert_eq!(config.detection.doc_body_min_chars, 80);
        assert_eq!(config.detection.comment_similarity, 0.7);
        assert_eq!(config.detection.max_nest_depth, 5);
        // 未指定の項目は既定値
        assert_eq!(config.detection.section_similarity, 0.75);
        assert_eq!(config.detection.doc_similarity, 0.65);
        // 報告閾値は [detection]、統合閾値は [merge] に分離されている(§8)
        let config = Config::from_toml("[detection]\ndoc_similarity = 0.4\n")?;
        assert_eq!(config.detection.doc_similarity, 0.4);
        Ok(())
    }

    #[test]
    fn parses_orphan_entry_points() -> Result<()> {
        let config = Config::from_toml(
            r#"
[detection]
orphan_entry_points = ["README.md", "docs/SUMMARY.md"]
"#,
        )?;
        assert_eq!(
            config.detection.orphan_entry_points,
            vec!["README.md", "docs/SUMMARY.md"]
        );
        // 未指定なら既定は README.md(従来挙動)
        assert_eq!(
            Config::from_toml("")?.detection.orphan_entry_points,
            vec!["README.md"]
        );
        Ok(())
    }

    #[test]
    fn parses_style_patterns_and_allow() -> Result<()> {
        let config = Config::from_toml(
            r#"
[style]
target = "plain"

[patterns.redundant]
"独自の冗長表現です" = "簡潔な表現"

[lint.allow]
terms = ["行なう"]
"#,
        )?;
        assert_eq!(config.style.target, StyleTarget::Plain);
        assert_eq!(
            config
                .patterns
                .redundant
                .get("独自の冗長表現です")
                .map(String::as_str),
            Some("簡潔な表現")
        );
        assert_eq!(config.lint.allow.terms, vec!["行なう"]);
        // 未指定なら既定は polite
        assert_eq!(Config::from_toml("")?.style.target, StyleTarget::Polite);
        Ok(())
    }

    #[test]
    fn apply_overrides_severity() -> Result<()> {
        let config = Config::from_toml(
            r#"[lint.rules]
"r/x" = "info""#,
        )?;
        let d = config.apply(diagnostic("r/x")).unwrap();
        assert_eq!(d.severity, Severity::Info);
        Ok(())
    }

    #[test]
    fn apply_all_suppresses_diagnostic() -> Result<()> {
        let config = Config::from_toml(
            r#"[lint.rules]
"consistency/particle" = "allow""#,
        )?;
        assert!(config.apply(diagnostic("consistency/particle")).is_none());
        // 対象外のルールは素通し
        assert!(config.apply(diagnostic("r/y")).is_some());
        Ok(())
    }

    #[test]
    fn rule_excludes_suppress_only_matching_paths() -> Result<()> {
        // #75: [lint.rule_excludes] はルール×パス単位の抑制。対象外パスには
        // 報告が残る([lint.rules] の allow との違い)
        let config = Config::from_toml(
            r#"[lint.rule_excludes]
"structure/empty-section" = [".opencode/commands/**"]"#,
        )?;
        assert!(
            config
                .lint
                .rule_excludes
                .contains_key("structure/empty-section")
        );
        // base なし: cwd 基準の相対化のみ
        let excludes = RuleExcludes::build(&config.lint.rule_excludes, None)?;
        assert!(
            excludes.is_suppressed(
                "structure/empty-section",
                Path::new(".opencode/commands/handover.md")
            ),
            "対象パスの診断は抑制される"
        );
        assert!(
            !excludes.is_suppressed(
                "structure/empty-section",
                Path::new(".agents/skills/check/SKILL.md")
            ),
            "対象外パスには通常どおり報告される"
        );
        assert!(
            !excludes.is_suppressed("relation/orphan", Path::new(".opencode/commands/x.md")),
            "対象外ルールには効かない"
        );
        Ok(())
    }

    #[test]
    fn rule_excludes_resolve_absolute_path_from_base() {
        // #76: 絶対パスの診断は基準ディレクトリ(jaio.toml の親 → cwd)で
        // 相対化して照合する。cwd が解析対象の外でも除外が効く
        let config = Config::from_toml(
            r#"[lint.rule_excludes]
"structure/empty-section" = [".opencode/commands/**"]"#,
        )
        .expect("有効な設定のはず");
        let base = PathBuf::from("/repo");
        let excludes = RuleExcludes::build(&config.lint.rule_excludes, Some(&base))
            .expect("パターンは有効のはず");
        assert!(
            excludes.is_suppressed(
                "structure/empty-section",
                &base.join(".opencode/commands/handover.md")
            ),
            "基準ディレクトリ配下の絶対パスは相対化して一致する"
        );
        assert!(
            !excludes.is_suppressed(
                "structure/empty-section",
                &base.join(".agents/skills/check/SKILL.md")
            ),
            "基準配下でも対象外パスは除外されない"
        );
        assert!(
            !excludes.is_suppressed(
                "structure/empty-section",
                Path::new("/elsewhere/.opencode/commands/x.md")
            ),
            "基準外の絶対パスは照合不能として除外しない(安全側)"
        );
    }

    #[test]
    fn style_overrides_resolve_absolute_path_from_base() {
        // #76: [style.overrides] の照合も jaio.toml 配置ディレクトリ基準で
        // 絶対パスを相対化する(cwd が解析対象の外でもパターンが効く)
        let overrides = BTreeMap::from([("docs/**".to_string(), StyleTarget::Polite)]);
        let base = PathBuf::from("/repo");
        let compiled = StyleOverrides::build(&overrides, Some(&base)).expect("有効なパターン");
        assert_eq!(
            compiled.get(&base.join("docs/design.md")),
            Some(StyleTarget::Polite),
            "基準配下の絶対パスは相対化して一致する"
        );
        assert_eq!(
            compiled.get(Path::new("docs/design.md")),
            Some(StyleTarget::Polite),
            "相対パスは従来どおり一致する"
        );
        // 基準外の絶対パスは照合不能(上書きしない安全側)
        assert_eq!(compiled.get(Path::new("/elsewhere/docs/x.md")), None);
    }

    #[test]
    fn rule_excludes_empty_map_matches_nothing() {
        // パターン検証は ignore(gitignore 準拠)に委ねる。git と同じく緩い
        // 構文はリテラル扱いになるため、設定検証の主盾は未知ルール ID の
        // 早期エラー(cli の validate_config)。ここでは空設定の挙動を確認する
        let empty = RuleExcludes::build(&BTreeMap::new(), None).expect("空マップは常に有効");
        assert!(!empty.is_suppressed("structure/empty-section", Path::new("a.md")));
    }

    #[test]
    fn normalize_lexical_resolves_dots() {
        // #87: cwd 結合後の字句解決。../ を含む相対パスが正しく剥がれることを
        // 担保する(パターン照合の基準剥ぎはこの結果に対して字句比較で行う)
        let n = |p: &str| normalize_lexical(Path::new(p)).display().to_string();
        assert_eq!(n("/jaio/docs/../README.md"), "/jaio/README.md");
        assert_eq!(n("/jaio/docs/./x.md"), "/jaio/docs/x.md");
        assert_eq!(n("docs/./x.md"), "docs/x.md");
        // 解析ルートを超える `..` は潜ったまま残す(jaio_rules::normalize と同じ)
        assert_eq!(n("../x.md"), "../x.md");
        assert_eq!(n("a/../../b"), "../b");
        assert_eq!(n("/a/b/"), "/a/b");
        // cwd 結合済みの絶対パスでの ../ 解決(#87: パターン照合の基準剥ぎ)
        assert_eq!(n("/repo/docs/../AGENTS.md"), "/repo/AGENTS.md");
        assert_eq!(n("/repo/docs/../docs/design.md"), "/repo/docs/design.md");
        // relativize_for_match の ../ 保持ケース(cwd 外の .. は解析ルート外)
        assert_eq!(n("../AGENTS.md"), "../AGENTS.md");
        assert_eq!(n("../docs/design.md"), "../docs/design.md");
    }

    #[test]
    #[cfg(unix)]
    fn relativize_for_match_unifies_symlink_form() {
        // #89: macOS の /var → /private/var 等の symlink で base と path の
        // 表記が分かれても、canonicalize による表記統一で基準剥ぎが効く。
        // canonicalize された絶対パス(テストが canonicalize 経由でルートを
        // 渡す形態)と symlink 経由の base の混在を再現する
        use std::os::unix::fs::symlink;

        let tmp = std::env::temp_dir().join(format!("jaio-symlink-{}", std::process::id()));
        let real = tmp.join("real");
        std::fs::create_dir_all(&real).expect("ディレクトリを作成できません");
        let link = tmp.join("link");
        let _ = std::fs::remove_file(&link);
        symlink(&real, &link).expect("symlink を作成できません");
        std::fs::write(real.join("a.md"), "x").expect("ファイルを書き込めません");

        // path は symlink 経由・base は canonicalize 表記でも剥げる
        // (base は呼び出し側の構築時に resolved_base を通す契約、#106)
        let via_link =
            super::relativize_for_match(&link.join("a.md"), Some(&super::resolved_base(&real)));
        assert_eq!(via_link, Path::new("a.md"));
        // 逆に path が canonicalize 表記・base が symlink 経由でも剥げる
        let real_file = real
            .join("a.md")
            .canonicalize()
            .expect("canonicalize できる");
        let via_real = super::relativize_for_match(&real_file, Some(&super::resolved_base(&link)));
        assert_eq!(via_real, Path::new("a.md"));

        let _ = std::fs::remove_file(&link);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn lint_default_applies_to_unlisted_rules() -> Result<()> {
        // [lint] default は個別指定のないルールに効く(設計書 §8)
        let config = Config::from_toml(
            r#"[lint]
default = "info"
[lint.rules]
"r/x" = "deny"
"#,
        )?;
        // 個別指定のあるルールはそちらが優先
        assert_eq!(
            config.apply(diagnostic("r/x")).map(|d| d.severity),
            Some(Severity::Error)
        );
        // ないルールは default が効く
        assert_eq!(
            config.apply(diagnostic("r/y")).map(|d| d.severity),
            Some(Severity::Info)
        );
        // default = "allow" は指定のない診断を抑制する
        let config = Config::from_toml("[lint]\ndefault = \"allow\"\n")?;
        assert!(config.apply(diagnostic("r/z")).is_none());
        // 未指定ならルール内蔵の重大度を通す
        let config = Config::from_toml("")?;
        assert_eq!(
            config.apply(diagnostic("r/z")).map(|d| d.severity),
            Some(Severity::Warn)
        );
        Ok(())
    }

    #[test]
    fn deny_is_accepted_as_error() -> Result<()> {
        // 設計書 §8 の表記(rustfmt/clippy 文化の deny)を許容する
        let config = Config::from_toml(
            r#"[lint.rules]
"r/x" = "deny""#,
        )?;
        assert_eq!(
            config.apply(diagnostic("r/x")).map(|d| d.severity),
            Some(Severity::Error)
        );
        Ok(())
    }

    #[test]
    fn unknown_keys_are_rejected() {
        // linter の設定 typo を黙殺しない
        assert!(Config::from_toml("[paths]\nexcludes = [\"target/\"]\n").is_err());
        assert!(Config::from_toml("[detection]\nmin_similarity = 0.9\n").is_err());
        // 実装済みキーは受理される
        assert!(Config::from_toml("[analysis]\nmode = \"decompose\"\n").is_ok());
        // 値のバリデーションも効く
        assert!(Config::from_toml("[analysis]\nmode = \"split\"\n").is_err());
    }

    #[test]
    fn parses_analysis_comments_and_include() -> Result<()> {
        let config = Config::from_toml(
            r#"
[analysis]
mode = "decompose"
encoding = "cl100k_base"

[paths]
include = ["docs/**/*.md"]

[comments]
languages = ["rust", "python"]
"#,
        )?;
        assert_eq!(config.analysis.mode, AnalysisMode::Decompose);
        assert_eq!(config.analysis.encoding, TokenEncoding::Cl100kBase);
        assert_eq!(config.paths.include, vec!["docs/**/*.md"]);
        assert_eq!(config.comments.languages, vec!["rust", "python"]);
        // 既定値
        let default = Config::from_toml("")?;
        assert_eq!(default.analysis.mode, AnalysisMode::Normal);
        assert_eq!(default.analysis.encoding, TokenEncoding::O200kBase);
        assert!(default.comments.languages.is_empty());
        Ok(())
    }

    #[test]
    fn parses_analysis_dictionary() -> Result<()> {
        let config = Config::from_toml("[analysis]\ndictionary = \"/tmp/dict\"\n")?;
        assert_eq!(config.analysis.dictionary.as_deref(), Some("/tmp/dict"));
        // 未指定なら None(埋め込み辞書を使う)
        assert!(Config::from_toml("")?.analysis.dictionary.is_none());
        Ok(())
    }

    #[test]
    fn exclusion_matches_prefix_and_basename() {
        let config = Config {
            paths: PathsConfig {
                include: vec![],
                exclude: vec![
                    "crates/jaio-rules/tests/fixtures".to_string(),
                    "AGENTS.md".to_string(),
                    "target/".to_string(),
                ],
                include_hidden: true,
            },
            ..Default::default()
        };
        assert!(config.is_excluded("crates/jaio-rules/tests/fixtures/sample.md"));
        assert!(config.is_excluded("AGENTS.md"));
        assert!(config.is_excluded("docs/AGENTS.md"));
        assert!(config.is_excluded("target/debug/jaio"));
        assert!(!config.is_excluded("docs/design.md"));
        assert!(!config.is_excluded("docs/AGENTS-extra.md"));
        assert!(!config.is_excluded("docs/my-fixtures/a.md"));

        // ディレクトリ名のみのパターンはパス途中のセグメント一致で除外する
        let mid = Config {
            paths: PathsConfig {
                include: vec![],
                exclude: vec!["fixtures".to_string()],
                include_hidden: false,
            },
            ..Default::default()
        };
        assert!(mid.is_excluded("jaio-rules/tests/fixtures/sample.md"));
        assert!(!mid.is_excluded("docs/my-fixtures/a.md"));
    }

    #[test]
    fn include_glob_restricts_collection() -> Result<()> {
        // [paths] include: glob に一致するものだけ収集対象(§8)
        let config = Config::from_toml("[paths]\ninclude = [\"docs/**/*.md\"]\n")?;
        let filter = config.path_filter()?;
        assert!(filter.allows(Path::new("docs/design.md")));
        assert!(filter.allows(Path::new("docs/sub/a.md")));
        assert!(!filter.allows(Path::new("README.md")));
        assert!(!filter.allows(Path::new("src/main.rs")));
        // include が空なら全件対象
        let all = Config::from_toml("")?.path_filter()?;
        assert!(all.allows(Path::new("README.md")));
        // include と exclude の併用: include 一致でも exclude なら外れる
        let config =
            Config::from_toml("[paths]\ninclude = [\"docs/**/*.md\"]\nexclude = [\"docs/tmp\"]\n")?;
        let filter = config.path_filter()?;
        assert!(filter.allows(Path::new("docs/design.md")));
        assert!(!filter.allows(Path::new("docs/tmp/draft.md")));
        // 明示指定ファイルは include を適用しない(exclude は効く)
        assert!(filter.allows_explicit(Path::new("README.md")));
        assert!(!filter.allows_explicit(Path::new("docs/tmp/draft.md")));
        // gitignore のネガーションも効く
        let config = Config::from_toml("[paths]\nexclude = [\"docs/**\", \"!docs/keep.md\"]\n")?;
        let filter = config.path_filter()?;
        assert!(filter.allows(Path::new("docs/keep.md")));
        assert!(!filter.allows(Path::new("docs/other.md")));
        Ok(())
    }
}
