use super::*;

/// 設定ファイルを読み込む。優先順位は
/// 1. `--config` の明示指定
/// 2. `JAIO_CONFIG` 環境変数(インライン TOML。LLM 委譲ワークフロー向け、release.md)
/// 3. カレントディレクトリから上位方向に `jaio.toml` を探索
/// 4. 3. が見つからなければ解析対象パス(先頭)から同様に探索
/// 5. いずれもなければ既定値(設計書 §8)
///
/// 対象パスからのフォールバックにより、別ディレクトリから絶対パス指定で
/// 実行した場合(LLM 委譲ワークフロー)も対象リポジトリの設定が使われる。
pub fn load_config(explicit: Option<&Path>, paths: &[PathBuf]) -> Result<Config> {
    let Some(explicit) = explicit else {
        if let Ok(inline) = env::var("JAIO_CONFIG") {
            return config_from_inline(&inline);
        }
        let cwd = env::current_dir().context("カレントディレクトリの取得に失敗しました")?;
        for dir in cwd.ancestors() {
            let candidate = dir.join("jaio.toml");
            if candidate.is_file() {
                return parse_config_file(&candidate);
            }
        }
        // カレント側に見つからない場合のみ対象パス側を探索する(カレント優先)
        if let Some(target) = paths.iter().find(|p| p.is_absolute() || p.exists()) {
            let anchor = if target.is_absolute() {
                target.clone()
            } else {
                env::current_dir()?.join(target)
            };
            for dir in anchor.ancestors() {
                let candidate = dir.join("jaio.toml");
                if candidate.is_file() {
                    return parse_config_file(&candidate);
                }
            }
        }
        return Ok(Config::default());
    };
    parse_config_file(explicit)
}

/// `JAIO_CONFIG` 環境変数のインライン TOML をパースする。
/// ファイル経路(parse_config_file)と同じ追加検証を通す
pub(crate) fn config_from_inline(inline: &str) -> Result<Config> {
    let config = Config::from_toml(inline).context("JAIO_CONFIG の解析に失敗しました")?;
    validate_config(&config)?;
    Ok(config)
}

/// パース済み設定の追加検証。設定ファイル・環境変数の両経路で使う
pub(crate) fn validate_config(config: &Config) -> Result<()> {
    // [style.overrides] の無効パターンも黙って無効化されないよう早期エラーにする(#55)。
    // 相対化基準は jaio.toml の配置ディレクトリ(#76)
    StyleOverrides::build(&config.style.overrides, config.toml_base())?;
    // [lint.rule_excludes] の構築エラーも読み込み時に出す(#75)。緩い構文は
    // gitignore 準拠でリテラル扱いになるため、未知ルール ID 検証が主盾。
    // 相対化基準は jaio.toml の配置ディレクトリ(#76)
    RuleExcludes::build(&config.lint.rule_excludes, config.toml_base())?;
    // lint.rules / lint.rule_excludes のルール ID typo は黙って無効化されるため
    // 早期エラーにする(#36)。「既知」の一覧は辞書順(HashSet 反復に依存しない
    // 決定的な表示、#110)
    let mut known: Vec<&'static str> = known_rule_ids();
    known.sort_unstable();
    let known_list = known.join(", ");
    for id in config.lint.rules.keys() {
        if !known.contains(&id.as_str()) {
            anyhow::bail!("lint.rules に未知のルール ID があります: {id}(既知: {known_list})");
        }
    }
    for id in config.lint.rule_excludes.keys() {
        if !known.contains(&id.as_str()) {
            anyhow::bail!(
                "lint.rule_excludes に未知のルール ID があります: {id}(既知: {known_list})"
            );
        }
    }
    Ok(())
}

pub(crate) fn parse_config_file(path: &Path) -> Result<Config> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("設定ファイルの読み込みに失敗しました: {}", path.display()))?;
    let mut config = Config::from_toml(&text)
        .with_context(|| format!("設定ファイルの解析に失敗しました: {}", path.display()))?;
    validate_config(&config)?;
    // ファイルから読んだ場合のみ配置ディレクトリを記録する(#73)。
    // JAIO_CONFIG(インライン)や既定値には対応するファイルが存在しない。
    // 相対パス指定でも toml_base が空にならないよう絶対化する(#79:
    // 空の基準は strip_prefix が常に成功し、絶対パスが照合に漏れる)
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        match env::current_dir() {
            Ok(cwd) => cwd.join(path),
            Err(_) => path.to_path_buf(),
        }
    };
    config.source_path = Some(crate::normalize_path(&absolute));
    Ok(config)
}

#[cfg(test)]
mod inline_config_tests {
    use super::*;

    /// JAIO_CONFIG 環境変数のインライン TOML がパース・検証される
    /// (release.md のリリースノートが案内する形式、#58 の検証で発見した未実装)
    #[test]
    fn inline_config_parses_and_validates() -> Result<()> {
        let config = config_from_inline("[paths]\nexclude = [\"tmp/\"]\n")?;
        assert!(config.paths.exclude.contains(&"tmp/".to_string()));
        // 既定は上書きされない
        assert_eq!(config.style.target, jaio_core::config::StyleTarget::Polite);
        // ファイル経路と同じ追加検証: lint.rules の未知 ID は早期エラー
        let err = config_from_inline("[lint.rules]\n\"no/such-rule\" = \"warn\"\n");
        assert!(err.is_err(), "未知のルール ID はエラーになるはず");
        // lint.rule_excludes も同じ検証を受ける(#75)
        let err =
            config_from_inline("[lint.rule_excludes]\n\"no/such-rule\" = [\"templates/**\"]\n");
        assert!(err.is_err(), "rule_excludes の未知 ID もエラーになるはず");
        Ok(())
    }
}
