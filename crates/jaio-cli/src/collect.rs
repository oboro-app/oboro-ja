use super::*;

/// 診断の表示用パスを正規化する。`.` / `..` セグメントの解決は
/// `jaio_rules::normalize_path` に委譲し、ここでは表示用の整形だけを行う。
/// 絶対パスはカレントディレクトリ配下なら相対パスへ寄せ、それ以外はそのまま返す。
pub(crate) fn normalize_display(path: &Path) -> String {
    if path.is_absolute() {
        if let Ok(cwd) = env::current_dir()
            && let Ok(rel) = path.strip_prefix(&cwd)
        {
            let rel_text = rel.display().to_string();
            if !rel_text.is_empty() {
                return rel_text;
            }
            return ".".to_string();
        }
        return path.display().to_string();
    }
    normalize_path(path).display().to_string()
}

/// 明示指定ファイルの exclude 照合用パス(#41、#79)。
/// 実体を canonicalize してから **① jaio.toml の配置ディレクトリ → ② cwd** の
/// 順に相対化する(#76 と同じ多段基準。cwd 固定だと cwd 外のファイルで
/// exclude が効かず、同じ設定なのに検査対象が変わる)。どちらの基準にも
/// 属さないファイルは照合不能として None を返し、呼び出し側は
/// 「照合不能 = 許可」として扱う(#44)
pub(crate) fn explicit_relative_path(path: &Path, base: Option<&Path>) -> Option<PathBuf> {
    let cwd = env::current_dir().ok()?;
    // canonicalize で symlink・`..` を実体解決する。失敗した場合は語彙上の
    // 絶対化で内外を判定する(判定自体は安全側に倒す)
    let absolute = fs::canonicalize(path).unwrap_or_else(|_| {
        let joined = if path.is_absolute() {
            path.to_path_buf()
        } else {
            cwd.join(path)
        };
        normalize_path(&joined)
    });
    // cwd 自体が symlink 経由(macOS の /tmp 等)でも剥がせるよう両方試す
    let bases = [
        base.map(|b| b.to_path_buf()),
        Some(cwd.clone()),
        fs::canonicalize(&cwd).ok(),
    ];
    for root in bases.into_iter().flatten() {
        if root.as_os_str().is_empty() {
            // 空の基準は strip_prefix が常に成功して絶対パスを返してしまう
            continue;
        }
        let root = fs::canonicalize(&root).unwrap_or(root);
        if let Ok(rel) = absolute.strip_prefix(&root) {
            return Some(normalize_path(rel));
        }
    }
    None
}

/// 対象の Markdown ファイルを収集する(.gitignore と jaio.toml の exclude に準拠)。
/// 走査中の致命的でないエラーは捨てる。完全性の記録が必要な場合は
/// `collect_files_reporting` を使う(§7.3 の context.targets、#36)。
pub fn collect_md_files(paths: &[PathBuf], config: &Config) -> Result<Vec<PathBuf>> {
    Ok(collect_files_reporting(paths, config, is_markdown)?.0)
}

pub(crate) fn is_markdown(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("mdx"))
}

/// 対象のソースコードファイルを収集する(コメント抽出対象、§6.3)。
/// `[comments] languages` で抽出対象言語を絞れる(空なら全言語)。
/// 未知の言語名は設定エラーにする(typo の黙殺を避けるため)。
pub(crate) fn collect_source_files(
    paths: &[PathBuf],
    config: &Config,
) -> Result<(Vec<PathBuf>, Vec<JsonOperationalError>)> {
    for lang in &config.comments.languages {
        if !jaio_collect::SUPPORTED_LANGUAGES.contains(&lang.as_str()) {
            anyhow::bail!(
                "comments.languages に未対応の言語があります: {lang}(対応: {})",
                jaio_collect::SUPPORTED_LANGUAGES.join(", ")
            );
        }
    }
    collect_files_reporting(paths, config, |path| {
        jaio_collect::language_name(path).is_some_and(|name| {
            config.comments.languages.is_empty()
                || config.comments.languages.iter().any(|l| l == name)
        })
    })
}

/// 条件に一致するファイルを走査で収集する共通処理。
/// 隠しエントリは `include_hidden` で制御する(エージェント運用リポジトリの
/// `.agents/` 等を対象にする)。`.git` は常に除外する。
/// include/exclude は gitignore 準拠の `PathFilter` で評価する(§8 [paths])。
/// `collect_files` + 致命的でない走査エラーの記録(#36)。
/// ディレクトリを走査できなかった場合は operational_errors に残し、
/// 発見できなかったファイル数は「不明」として 0 で埋めない
/// (`JsonTargets.discovery_complete` で表す)。
pub(crate) fn collect_files_reporting(
    paths: &[PathBuf],
    config: &Config,
    predicate: impl Fn(&Path) -> bool,
) -> Result<(Vec<PathBuf>, Vec<JsonOperationalError>)> {
    let mut errors = Vec::new();
    if paths.is_empty() {
        return Ok((Vec::new(), errors));
    }
    let filter = config.path_filter()?;
    // パターン照合の基準は jaio.toml の配置ディレクトリ(運用上 PJ ルート、
    // #79)。cwd を変えても同じ設定が同じ効き方になる
    let toml_base = config.toml_base().map(PathBuf::from);
    let mut out = Vec::new();
    for path in paths {
        // 存在しないパスはタイポ等の可能性が高く、静かに 0 件扱いしない
        if !path
            .try_exists()
            .with_context(|| format!("対象パスへのアクセスに失敗しました: {}", path.display()))?
        {
            anyhow::bail!("対象パスが存在しません: {}", path.display());
        }
        if path.is_file() {
            // 明示指定ファイルは include より前に exclude だけ見る
            // (利用者が直接指定したものを include glob が黙って捨てるのは不親切)。
            // フィルタは解析ルート相対のパスで評価する(絶対パスを渡すと
            // gitignore のルート検査に違反して panic するため相対化する、#41)。
            // ルート外(cwd 外)のファイルは exclude パターンの適用範囲外なので
            // 照合不能(None)は許可として扱う(#44)
            let allowed = explicit_relative_path(path, toml_base.as_deref())
                .is_none_or(|relative| filter.allows_explicit(&relative));
            if predicate(path) && allowed {
                out.push(path.clone());
            } else {
                // 存在するが対象外の明示指定は黙って 0 件扱いにしない(#97)。
                // タイポや拡張子違いが「問題なし(exit 0)」に見えるのを防ぐ
                let reason = if !predicate(path) {
                    "Markdown 以外の拡張子"
                } else {
                    "jaio.toml の [paths] exclude に一致"
                };
                eprintln!(
                    "warning: 指定されたファイルは解析対象外です({reason}): {}",
                    path.display()
                );
            }
            continue;
        }
        // 走査ルートを設定基準(jaio.toml 配置ディレクトリ → cwd)相対に寄せた
        // プレフィックス。include/exclude は jaio.toml 相対パターンで評価される
        // ため、引数がサブディレクトリでも cwd を変えても設定のパターンが
        // そのまま効くようにする(#49、#79)。基準外は走査ルート相対の
        // ベストエフォート照合にフォールバックする(明示指定ファイルの
        // 「照合不能 = 許可」とは異なり、ベース名パターンはルート外でも一致させる)
        let root_prefix = explicit_relative_path(path, toml_base.as_deref());
        let walker = WalkBuilder::new(path)
            .hidden(!config.paths.include_hidden)
            .git_ignore(true)
            // .git の有無に依らず .gitignore を適用する(#58)。
            // ignore クレートの既定(require_git = true)は git リポジトリ内でしか
            // .gitignore を効かせないため、エクスポートツリー等で
            // node_modules/ 等が解析対象に漏れ出ていた
            .require_git(false)
            .filter_entry(|entry| {
                // .git は include_hidden によらず常に除外する
                !matches!(entry.file_name().to_str(), Some(".git"))
            })
            .build();
        for entry in walker {
            let entry = match entry {
                Ok(entry) => entry,
                Err(err) => {
                    // 走査エラーは解析を止めず operational error として記録する
                    let path = match &err {
                        ignore::Error::WithPath { path, .. } => Some(normalize_display(path)),
                        _ => None,
                    };
                    errors.push(JsonOperationalError {
                        path,
                        stage: "scan".to_string(),
                        reason: err.to_string(),
                    });
                    continue;
                }
            };
            // フィルタは解析ルート相対で評価する(gitignore と同じ基準)。
            // 走査ルートが解析ルートそのもの(例: `.`)や解析ルート外の場合は
            // 走査ルート相対で評価する(ルート外では解析ルート相対のプレフィックスが
            // 取れないため。ベース名パターンはどちらの基準でも一致する)
            let rel = entry.path().strip_prefix(path).unwrap_or(entry.path());
            let allowed = match &root_prefix {
                Some(prefix) if prefix != Path::new(".") => filter.allows(&prefix.join(rel)),
                _ => filter.allows(rel),
            };
            if predicate(entry.path()) && allowed {
                out.push(entry.into_path());
            }
        }
    }
    out.sort();
    // 表記系の異なる同一ファイル(走査結果 + 明示指定、相対/絶対混在、
    // symlink 経由)は実体同一性キーで重複排除する(#44: 文字列 dedup では
    // `./docs/a.md` と `docs/a.md` が残り、自己との重複ペアが報告されていた)
    let mut seen = HashSet::new();
    out.retain(|p| seen.insert(comparison_key(p)));
    // 走査順(readdir 順)に依存しない決定的な並びにする(#110)。
    // diagnostics(§7.3)・sources と同様に、operational_errors も
    // 実行間・環境間で同じ順序を出力する
    errors.sort_by(|a, b| {
        (a.path.as_deref(), a.stage.as_str()).cmp(&(b.path.as_deref(), b.stage.as_str()))
    });
    Ok((out, errors))
}

/// ソースコードから日本語コメントを抽出して正規化済み本文として返す(§5.4)。
/// 読み込み・抽出の失敗は operational error として記録し、戻り値の第 2 要素で
/// 返す(doc-comment-drift の入力不足の検出に使う、#36)。
pub(crate) fn collect_comment_files(
    paths: &[PathBuf],
    config: &Config,
) -> Result<(Vec<CommentFile>, Vec<JsonOperationalError>)> {
    let mut out = Vec::new();
    // コメント抽出対象の走査失敗は md 走査と区別できるよう stage を分ける(#36)
    let (files, mut errors) = collect_source_files(paths, config)?;
    for error in &mut errors {
        error.stage = "comment_scan".to_string();
    }
    for file in files {
        let source = match fs::read_to_string(&file) {
            Ok(source) => source,
            Err(err) => {
                errors.push(JsonOperationalError {
                    path: Some(normalize_display(&file)),
                    stage: "comment_read".to_string(),
                    reason: err.to_string(),
                });
                continue;
            }
        };
        let raw_comments = match jaio_collect::extract_comments(&file, &source) {
            Ok(raw_comments) => raw_comments,
            Err(err) => {
                errors.push(JsonOperationalError {
                    path: Some(normalize_display(&file)),
                    stage: "comment_extract".to_string(),
                    reason: err.to_string(),
                });
                continue;
            }
        };
        // 日本語を含むコメントだけを本文に連結し、先頭コメントの行を
        // 診断の根拠位置として保持する(#44: 診断が常に 1:1 になるのを防ぐ)
        let mut first_line = None;
        let mut cleaned = Vec::new();
        for raw in &raw_comments {
            let c = jaio_collect::clean_comment(&raw.text);
            if jaio_collect::contains_japanese(&c) {
                first_line.get_or_insert(raw.line);
                cleaned.push(c);
            }
        }
        let body = normalize_text(&cleaned.join(" "));
        if !body.is_empty() {
            out.push(CommentFile {
                path: file.clone(),
                display: normalize_display(&file),
                body,
                first_line: first_line.unwrap_or(1),
                sha256: format!("{:x}", sha2::Sha256::digest(source.as_bytes())),
            });
        }
    }
    Ok((out, errors))
}

#[cfg(test)]
mod unreadable_tests {
    use super::*;
    use std::fs;

    /// 無効な UTF-8 を含むファイルがあっても解析全体を中止しない(Issue #21)
    #[test]
    fn invalid_utf8_file_is_skipped_not_fatal() -> Result<()> {
        let dir = std::env::temp_dir().join("jaio-invalid-utf8-test");
        fs::create_dir_all(&dir)?;
        // 意図的に無効なバイト列を含む .md
        fs::write(dir.join("broken.md"), b"# \xff\xfe broken\n")?;
        fs::write(dir.join("ok.md"), "# 題名\n\n本文です。\n")?;

        let mut buf = Vec::new();
        let summary = run_check(
            std::slice::from_ref(&dir),
            &mut buf,
            &Config::default(),
            &OutputFormat::Terminal { color: false },
            false,
            false,
        )?;
        assert_eq!(summary.files, 1, "読める 1 ファイルだけ解析するはず");
        fs::remove_dir_all(&dir).ok();
        Ok(())
    }
}

#[cfg(test)]
mod parallel_tests {
    use super::*;
    use std::fs;

    /// ファイル単位の並列実行でも診断内容と並びが実行間で一致する(§10、Issue #14)
    #[test]
    fn parallel_check_is_deterministic() -> Result<()> {
        let dir = std::env::temp_dir().join("jaio-parallel-determinism-test");
        fs::create_dir_all(&dir)?;
        // 各ファイルに冗長表現と文書間の共通本文を持たせ、DocumentRule と
        // CorpusRule の両方が発火するコーパスを作る
        let boiler = "本書はセットアップの手順を説明する文書です。対象読者は初めて利用する開発者です。前提条件として Rust と Git のインストールが必要です。";
        for i in 0..16 {
            let body = format!("# 文書{i}\n\n{boiler}\n\n手順{i}では設定することができます。\n");
            fs::write(dir.join(format!("doc{i:02}.md")), body)?;
        }

        let config = Config::default();
        let mut runs = Vec::new();
        for _ in 0..2 {
            let mut buf = Vec::new();
            let summary = run_check(
                std::slice::from_ref(&dir),
                &mut buf,
                &config,
                &OutputFormat::Json,
                false,
                false,
            )?;
            assert_eq!(summary.files, 16);
            let value: serde_json::Value = serde_json::from_slice(&buf)?;
            runs.push(value["diagnostics"].clone());
        }
        assert_eq!(runs[0], runs[1], "並列実行の診断が実行間で一致しない");

        // 診断の並びは (file, line, col, rule_id, observation) でソート済み(§7.3)
        let diags = runs[0].as_array().expect("diagnostics は配列");
        assert!(!diags.is_empty(), "fixture が診断を生むはず");
        let key = |d: &serde_json::Value| {
            (
                d["file"].as_str().unwrap().to_string(),
                d["span"]["line"].as_u64().unwrap(),
                d["span"]["col"].as_u64().unwrap(),
                d["rule_id"].as_str().unwrap().to_string(),
                d["observation"].as_str().unwrap().to_string(),
            )
        };
        let keys: Vec<_> = diags.iter().map(key).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted, "診断の並びがソート順ではない");
        fs::remove_dir_all(&dir).ok();
        Ok(())
    }
}

#[cfg(test)]
mod exclude_explicit_tests {
    use super::*;

    /// 明示指定ファイルでも exclude パターンが効く(#41)。
    /// テストは cwd 相対で完結させる(chdir は並列テストを壊すため使わない)
    #[test]
    fn explicit_file_respects_exclude_pattern() -> Result<()> {
        let cwd = env::current_dir()?;
        let docs = cwd.join("target").join("jaio-exclude-test").join("docs");
        fs::create_dir_all(&docs)?;
        fs::write(docs.join("draft.md"), "# 下書き\n\n本文です。\n")?;

        let config =
            Config::from_toml("[paths]\nexclude = [\"target/jaio-exclude-test/docs/draft.md\"]")?;
        let spec = "target/jaio-exclude-test/docs/draft.md";
        // 相対パス指定
        let files = collect_md_files(&[PathBuf::from(spec)], &config)?;
        assert!(files.is_empty(), "相対指定でも除外されるはず: {files:?}");
        // ./ 付き
        let files = collect_md_files(&[PathBuf::from(format!("./{spec}"))], &config)?;
        assert!(files.is_empty(), "./ 付きでも除外されるはず: {files:?}");
        // 絶対パス指定(cwd 配下)
        let files = collect_md_files(&[cwd.join(spec)], &config)?;
        assert!(files.is_empty(), "絶対指定でも除外されるはず: {files:?}");
        // 対象外のファイルは影響を受けない
        fs::write(docs.join("keep.md"), "# 保持\n\n本文です。\n")?;
        let files = collect_md_files(&[docs.join("keep.md")], &config)?;
        assert_eq!(files.len(), 1, "除外対象外は解析されるはず: {files:?}");
        fs::remove_dir_all(cwd.join("target").join("jaio-exclude-test")).ok();
        // テストが作った空の target/ 親ディレクトリも片付ける(中身があれば
        // remove_dir は失敗するだけなので既存のビルド成果物には触れない)
        fs::remove_dir(cwd.join("target")).ok();
        Ok(())
    }

    /// cwd 外の絶対パスを明示指定しても panic しない(#44)。
    /// gitignore マッチャのルート(".")外は exclude パターンの適用範囲外なので
    /// 許可される。非空の exclude で `matched_path_or_any_parents` の
    /// assert(!has_root) に到達しないことを確認する
    #[test]
    fn absolute_explicit_file_outside_cwd_is_allowed() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("jaio-outside-{}", std::process::id()));
        fs::create_dir_all(&dir)?;
        fs::write(dir.join("a.md"), "# 題名\n\n本文です。\n")?;
        let config = Config::from_toml("[paths]\nexclude = [\"never-match/**\"]")?;
        let files = collect_md_files(&[dir.join("a.md")], &config)?;
        assert_eq!(files.len(), 1, "ルート外の指定は解析されるはず: {files:?}");
        fs::remove_dir_all(&dir).ok();
        Ok(())
    }

    /// 走査結果と別表記の明示指定が同一実体を指す場合は実体同一性キーで
    /// 重複排除される(#44。`docs/../docs/a.md` のような `..` 経由表記が
    /// 文字列 dedup では残り、自己との重複ペアが報告されていた)
    #[test]
    fn overlapping_inputs_dedup_by_file_identity() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("jaio-dedup-{}", std::process::id()));
        let docs = dir.join("docs");
        fs::create_dir_all(&docs)?;
        fs::write(docs.join("a.md"), "# A\n\n本文です。\n")?;
        fs::write(docs.join("b.md"), "# B\n\n別の本文です。\n")?;
        let config = Config::default();
        let alias = dir.join("docs").join("..").join("docs").join("a.md");
        let files = collect_md_files(&[dir.clone(), alias], &config)?;
        assert_eq!(
            files.len(),
            2,
            "a.md は実体同一で 1 件に重複排除されるはず: {files:?}"
        );
        fs::remove_dir_all(&dir).ok();
        Ok(())
    }
}

#[cfg(test)]
mod gitignore_without_git_tests {
    use super::*;

    /// .git を持たないディレクトリでも .gitignore が適用される(#58)。
    /// テンポラリ(リポジトリ外)で .gitignore + node_modules を作って検証する
    #[test]
    fn gitignore_applies_without_git_directory() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("jaio-nogit-{}", std::process::id()));
        // 前回実行の残骸があれば掃除しておく
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("node_modules/pkg"))?;
        fs::write(dir.join(".gitignore"), "node_modules/\n")?;
        fs::write(dir.join("doc.md"), "# 文書\n\n本文です。\n")?;
        fs::write(
            dir.join("node_modules/pkg/LICENSE.md"),
            "# ライセンス\n\n本文です。\n",
        )?;
        // 前提検証: テンポラリが git リポジトリ内だとこのテストの意味がない
        assert!(
            !dir.ancestors().any(|a| a.join(".git").exists()),
            "テンポラリディレクトリが git リポジトリ内です: {}",
            dir.display()
        );
        let config = Config::default();
        let files = collect_md_files(std::slice::from_ref(&dir), &config)?;
        assert!(
            files
                .iter()
                .all(|f| !f.display().to_string().contains("node_modules")),
            ".git なしでも .gitignore が適用されるはず: {files:?}"
        );
        assert_eq!(files.len(), 1, "doc.md のみのはず: {files:?}");
        fs::remove_dir_all(&dir).ok();
        Ok(())
    }
}
