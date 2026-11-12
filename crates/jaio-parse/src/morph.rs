//! 形態素解析(lindera + SudachiDict(埋め込み、または `[analysis] dictionary` の
//! 外部辞書))。設計書 §4.2 の層 2 の基盤。

use anyhow::{Context as _, Result};
use std::path::Path;

use jaio_core::config::AnalysisMode;
use lindera::dictionary::load_dictionary;
use lindera::mode::{Mode, Penalty};
use lindera::segmenter::Segmenter;
use lindera::worker::SegmentWorker;

/// 形態素の簡易表現(ルールで必要なフィールドのみ)。
/// `jaio dump morph`(Issue #16)で JSON 出力するため Serialize を持つ。
#[derive(Debug, Clone, serde::Serialize)]
pub struct Morph {
    /// 表層形
    pub surface: String,
    /// 正規化表記(Sudachi の normalized_form。原形の代用として使う)
    pub lemma: String,
    /// 品詞1(大分類)
    pub pos1: String,
    /// 品詞2(中分類)
    pub pos2: String,
    /// 活用形
    pub conjugation_form: String,
    /// 読み(カタカナ)
    pub reading: String,
}

// Sudachi 辞書スキーマの details 内インデックス(設計書 §4.2)。
// lindera の details は先頭に surface を含む配置になる:
// 0=surface、1=品詞1、2=品詞2、3=品詞3、4=品詞4、5=活用型、6=活用形、
// 7=読み、8=正規化表記(左文脈 ID・右文脈 ID・コストは details に含まれない)。
// 変更時は `dump morph` の実出力で品詞・読み・正規化表記が正しく出ることを確認する
const POS1: usize = 1;
const POS2: usize = 2;
const CONJUGATION_FORM: usize = 6;
const READING: usize = 7;
const NORMALIZED_FORM: usize = 8;

/// 形態素解析器。SegmentWorker で Viterbi ラティスを再利用する。
pub struct Analyzer {
    worker: SegmentWorker,
}

impl Analyzer {
    /// 埋め込み Sudachi 辞書で解析器を作る(既定モード)。
    pub fn new() -> Result<Self> {
        Self::with_mode(AnalysisMode::default())
    }

    /// 解析モードを指定して作る(設計書 §8 `[analysis] mode`)。
    /// `decompose` は検索用途の分割抑制モードで、長い複合語を割りやすくなる。
    pub fn with_mode(mode: AnalysisMode) -> Result<Self> {
        Self::with_options(mode, None)
    }

    /// 解析モードと辞書の指定で作る(設計書 §8 `[analysis]`)。
    /// `dictionary` は lindera 辞書ディレクトリのパス(`file://` URI も可)。
    /// None のときは埋め込み辞書(Cargo feature `embedded-dict`)を使い、
    /// 埋め込みなしのビルドでは設定エラーにする
    pub fn with_options(mode: AnalysisMode, dictionary: Option<&str>) -> Result<Self> {
        let mode = match mode {
            AnalysisMode::Normal => Mode::Normal,
            AnalysisMode::Decompose => Mode::Decompose(Penalty::default()),
        };
        let dictionary = match dictionary {
            Some(path) => {
                // Sudachi 形式以外の lindera 辞書(ipadic 等)を渡すと
                // details のフィールド配置が変わり、品詞・読み・正規化表記が
                // 静かに別フィールドの値になる。metadata.json の存在で
                // Sudachi 形式かを確認する(#98)
                verify_sudachi_dictionary(path)
                    .with_context(|| format!("辞書の形式検証に失敗しました: {path}"))?;
                load_dictionary(path)
                    .with_context(|| format!("辞書の読み込みに失敗しました: {path}"))?
            }
            None if cfg!(feature = "embedded-dict") => load_dictionary("embedded://sudachidict")?,
            None => anyhow::bail!(
                "埋め込み辞書なしでビルドされています。jaio.toml の [analysis] dictionary に lindera 辞書ディレクトリを指定してください"
            ),
        };
        let segmenter = Segmenter::new(mode, dictionary, None);
        Ok(Self {
            worker: segmenter.into_worker(),
        })
    }

    /// テキストを形態素解析する。
    pub fn analyze(&mut self, text: &str) -> Result<Vec<Morph>> {
        let mut out = Vec::new();
        let tokens = self.worker.segment(text)?;
        for mut token in tokens {
            let surface = token.surface.to_string();
            let details = token.details();
            out.push(Morph {
                lemma: field(&details, NORMALIZED_FORM, &surface).to_string(),
                pos1: field(&details, POS1, "*").to_string(),
                pos2: field(&details, POS2, "*").to_string(),
                conjugation_form: field(&details, CONJUGATION_FORM, "*").to_string(),
                reading: field(&details, READING, "").to_string(),
                surface,
            });
        }
        Ok(out)
    }
}

/// 埋め込み SudachiDict の版識別(lindera-sudachidict crate 版、#64)。
/// 上流 SudachiDict の版は build 時に vendored され実行時には取得できないため、
/// 同梱辞書データを一意に決める crate 版を辞書の版として使う。
/// 埋め込みなしビルドでは None
pub fn embedded_dict_version() -> Option<&'static str> {
    #[cfg(feature = "embedded-dict")]
    {
        Some(lindera_sudachidict::get_version())
    }
    #[cfg(not(feature = "embedded-dict"))]
    {
        None
    }
}

/// details フィールドを範囲外アクセスなしで取り出す。
fn field<'a>(details: &[&'a str], index: usize, fallback: &'a str) -> &'a str {
    details.get(index).copied().unwrap_or(fallback)
}

/// 辞書ディレクトリが Sudachi 形式であることを確認する(#98)。
/// `file://` URI でも指定できるため剥いでから検査する
fn verify_sudachi_dictionary(path: &str) -> Result<()> {
    let fs_path = path.strip_prefix("file://").unwrap_or(path);
    let metadata = Path::new(fs_path).join("metadata.json");
    anyhow::ensure!(
        metadata.is_file(),
        "辞書ディレクトリに metadata.json がありません。jaio は Sudachi 形式の辞書(lindera-sudachidict で展開されたもの)にのみ対応しています: {}",
        fs_path
    );
    Ok(())
}

/// lindera のファイル辞書ローダが実際に読み込むファイル集合の正本(#36)。
/// `scripts/package-dict.sh` の必須ファイル検査と同じ集合に保つ。
/// 外部辞書の内容識別はこの集合を対象にし、sys.dic のような
/// 別形式の固定ファイル名には依存しない
pub const DICTIONARY_FILES: [&str; 9] = [
    "char_def.bin",
    "dict.trie",
    "dict.vals",
    "dict.valsidx",
    "dict.words",
    "dict.wordsidx",
    "matrix.mtx",
    "metadata.json",
    "unk.bin",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analyzes_japanese_text() -> Result<()> {
        let mut analyzer = Analyzer::new()?;
        let morphs = analyzer.analyze("日本語の文書を解析する。")?;
        assert!(!morphs.is_empty());
        // 名詞「文書」と動詞「解析」が含まれること(表層の連結で確認)
        let surfaces: String = morphs.iter().map(|m| m.surface.as_str()).collect();
        assert!(surfaces.contains("文書"));
        assert!(surfaces.contains("解析"));
        let verb = morphs.iter().find(|m| m.pos1 == "動詞");
        assert!(
            verb.is_some(),
            "動詞が少なくとも 1 つ検出される: {morphs:?}"
        );
        Ok(())
    }

    #[test]
    fn non_sudachi_dictionary_is_rejected() {
        // Sudachi 形式以外(metadata.json が無いディレクトリ)は
        // 設定エラーとして弾く(#98)
        let dir = std::env::temp_dir().join(format!("jaio-baddict-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let err = Analyzer::with_options(AnalysisMode::Normal, Some(dir.to_str().unwrap()))
            .err()
            .expect("metadata.json 無しの辞書はエラーのはず");
        // 原因チェーン末端に Sudachi 形式の案内が含まれる
        assert!(format!("{err:#}").contains("Sudachi"), "{err:#}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn external_dictionary_load_failure_is_config_error() {
        // 存在しない辞書パスは設定エラーとして Err になる(§8 [analysis] dictionary)。
        // 形式検証(#98)が不在ディレクトリを先に弾くため、検証側のエラーになる
        let err = Analyzer::with_options(AnalysisMode::Normal, Some("/nonexistent/jaio-dict"))
            .err()
            .expect("存在しない辞書パスはエラーのはず");
        assert!(
            format!("{err:#}").contains("metadata.json がありません"),
            "{err:#}"
        );
    }
}
