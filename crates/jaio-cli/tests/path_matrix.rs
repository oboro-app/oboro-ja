//! 実行形態マトリクス検証(#87)。
//!
//! cwd と解析ルートの表記の組合せを変えて `jaio check` を実行し、
//! 診断集合が同一であることを検証する。パス解決・パターン照合
//! ([style.overrides] / [lint.rule_excludes] / orphan_entry_points)を
//! 変更したときは、このテストを必ず通す(表記依存の回帰をここで捕捉する)。
//!
//! 子プロセスとして jaio バイナリを起動するため、テストプロセスの cwd は
//! 変更しない(他テストとの競合を避ける)。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context as _;

/// 統合テストから実行する jaio バイナリ。CARGO_BIN_EXE が設定されない
/// 環境でも debug ビルドの成果物で動くようにフォールバックする
fn jaio_bin() -> PathBuf {
    std::env::var("CARGO_BIN_EXE_jaio-cli")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            // ワークスペースルートの target/debug/jaio-cli(CARGO_MANIFEST_DIR は
            // crates/jaio-cli を指すため 2 段上の target を参照する)
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .and_then(|p| p.parent())
                .map(|root| root.join("target/debug/jaio"))
                .expect("ワークスペースルートが解決できません")
        })
}

/// 検証用 fixture リポジトリを生成する。診断集合を意図的に差が分かる構成にする:
/// - README.md: 敬体文書に常体 1 文混入 → polite 扱いなら style-deviation warn 1 件
/// - AGENTS.md: 常体文書 → plain 扱いなら診断なし
/// - docs/design.md: 敬体文書 → docs/** = polite が効けば診断なし
/// - docs/x.md: 常体文書 → plain 扱いなら診断なし、PR テンプレートから被リンク
/// - .github/pull_request_template.md: docs/x.md への相対リンク(repo ルート基準)
/// - README から docs/planned.md へのリンクは rule_excludes で抑制対象
fn make_fixture(dir: &Path) -> anyhow::Result<PathBuf> {
    let toml = r#"
[style]
target = "plain"

[style.overrides]
"docs/**" = "polite"

[lint.rule_excludes]
"relation/broken-link" = ["docs/planned.md"]

[detection]
orphan_entry_points = ["README.md"]
"#;
    let cfg = dir.join("jaio.toml");
    std::fs::create_dir_all(dir)?;
    std::fs::write(&cfg, toml)?;

    let docs = dir.join("docs");
    std::fs::create_dir_all(&docs)?;
    std::fs::create_dir_all(dir.join(".github"))?;
    std::fs::write(
        dir.join("README.md"),
        "# README\n\nこれはREADMEです。ただし、ここはですます体で書いています。\n\n[予定の文書](docs/planned.md)はまだありません。\n",
    )?;
    std::fs::write(
        dir.join("AGENTS.md"),
        "# エージェント指示\n\nこの文書は常体である。解析ではplainとして扱われる。\n",
    )?;
    std::fs::write(
        docs.join("design.md"),
        "# 設計\n\nこの文書はですます体で書かれています。docs配下はpoliteです。\n",
    )?;
    std::fs::write(docs.join("x.md"), "# X\n\nこの文書は常体です。\n")?;
    std::fs::write(
        dir.join(".github/pull_request_template.md"),
        "# PR テンプレート\n\n[x文書](docs/x.md)を確認してください。\n",
    )?;
    Ok(cfg)
}

/// check を実行して JSON を返す。`cwd` は実行形態、`target` は解析ルートの表記。
fn run_check(cwd: &Path, target: &Path, config: &Path) -> anyhow::Result<serde_json::Value> {
    let output = Command::new(jaio_bin())
        .arg("check")
        .arg(target)
        .arg("--config")
        .arg(config)
        .arg("--format")
        .arg("json")
        .current_dir(cwd)
        .output()
        .context("jaio の起動に失敗しました")?;
    anyhow::ensure!(
        output.status.success(),
        "jaio check が失敗しました(exit {:?}): {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    Ok(report)
}

/// 診断を「解析対象実パスからの fixture 相対パス」に正規化した集合にする。
/// 実行形態による file 表記の差分(cwd 相対・絶対・../ 付き)を吸収する
fn normalized_diagnostics(
    report: &serde_json::Value,
    fixture: &Path,
    cwd: &Path,
) -> anyhow::Result<BTreeSet<String>> {
    let mut out = BTreeSet::new();
    let fixture_real = fixture.canonicalize()?;
    for diag in report["diagnostics"]
        .as_array()
        .context("diagnostics が配列ではありません")?
    {
        let Some(file) = diag["file"].as_str() else {
            anyhow::bail!("diagnostics[].file が文字列ではありません");
        };
        let abs = if Path::new(file).is_absolute() {
            PathBuf::from(file)
        } else {
            cwd.join(file)
        };
        let rel = abs
            .canonicalize()?
            .strip_prefix(&fixture_real)?
            .to_string_lossy()
            .to_string();
        out.insert(format!(
            "{}\t{}\t{}",
            diag["rule_id"].as_str().unwrap_or("?"),
            rel,
            diag["span"]["line"]
        ));
    }
    Ok(out)
}

#[test]
fn diagnostics_are_identical_across_execution_forms() -> anyhow::Result<()> {
    let tmp = std::env::temp_dir().join(format!("jaio-path-matrix-{}", std::process::id()));
    let cfg = make_fixture(&tmp)?;
    let fixture = tmp.canonicalize()?;
    let docs = fixture.join("docs");
    let abs = fixture.canonicalize()?;

    // 解析対象が同一になる形態のみを比較する。
    // (cwd=docs・check "." のような「サブセット解析」は解析対象が変わるため対象外)
    let forms: Vec<(&str, PathBuf, PathBuf)> = vec![
        // (ラベル, 子プロセス cwd, 解析ルート)
        ("P1 ルート相対", fixture.clone(), PathBuf::from(".")),
        ("P2 絶対パス", fixture.clone(), abs.clone()),
        ("P3 cwd=docs・絶対ルート", docs.clone(), abs.clone()),
        ("P4 cwd=docs・親相対", docs.clone(), PathBuf::from("..")),
        // cwd が解析対象の外(実行ディレクトリと解析対象が無関係)の形態。
        // relativize_for_match の cwd 剥ぎが失敗し base 剥ぎのみが効く経路を
        // 担保する(#107)。LLM 委譲ワークフロー等で別ディレクトリから実行する
        // 利用に対応する挙動
        ("P5 cwd=解析対象外", std::env::temp_dir(), abs.clone()),
    ];

    let mut baseline: Option<(String, BTreeSet<String>)> = None;
    for (label, process_cwd, target) in &forms {
        let report = run_check(process_cwd, target, &cfg)
            .with_context(|| format!("{label} の実行に失敗"))?;
        let set = normalized_diagnostics(&report, &fixture, process_cwd)
            .with_context(|| format!("{label} の正規化に失敗"))?;
        match &baseline {
            None => {
                baseline = Some(((*label).to_string(), set));
            }
            Some((base_label, base_set)) => {
                if base_set != &set {
                    let missing: Vec<_> = base_set.difference(&set).collect();
                    let added: Vec<_> = set.difference(base_set).collect();
                    anyhow::bail!(
                        "実行形態で診断が異なります(基準: {base_label} / 比較: {label})。\n消失({}):\n{}\n新規({}):\n{}",
                        missing.len(),
                        missing
                            .iter()
                            .map(|s| format!("  - {s}\n"))
                            .collect::<String>(),
                        added.len(),
                        added
                            .iter()
                            .map(|s| format!("  + {s}\n"))
                            .collect::<String>()
                    );
                }
            }
        }
    }
    std::fs::remove_dir_all(&tmp).ok();
    Ok(())
}
