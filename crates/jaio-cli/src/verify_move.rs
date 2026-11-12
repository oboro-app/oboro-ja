use super::*;

/// 文書移動の lossless 検証(`jaio verify-move`、§7.2.1、#51)。
/// before の正規化済み実質行が after 側のどこかに存在するかを行単位で照合し、
/// 未保全行を報告する。完全保全なら true を返す(終了コード 0)
pub fn run_verify_move(
    before: &Path,
    after: &[PathBuf],
    out: &mut impl Write,
    format: StatsFormat,
) -> Result<bool> {
    if after.is_empty() {
        anyhow::bail!("移動先(after)を 1 つ以上指定してください");
    }
    let before_source = fs::read_to_string(before)
        .with_context(|| format!("移動元の読み込みに失敗しました: {}", before.display()))?;
    let before_display = normalize_display(before);
    let before_lines = substantive_lines(&before_source);

    let mut after_lines: Vec<(String, usize, String)> = Vec::new(); // (file, line, text)
    // 同じ after ファイルの重複指定は実体 ID で排除する(#80: 重複指定があると
    // after 側の出現数が水増しされ、多重集合照合の不足行検出が無効になる)
    let mut seen_after: std::collections::HashSet<String> = std::collections::HashSet::new();
    for path in after {
        let display = normalize_display(path);
        let entity = comparison_key(path);
        if !seen_after.insert(entity.display().to_string()) {
            continue;
        }
        let source = fs::read_to_string(path)
            .with_context(|| format!("移動先の読み込みに失敗しました: {}", path.display()))?;
        for (line, text) in substantive_lines(&source) {
            after_lines.push((display.clone(), line, text));
        }
    }

    // 実質行の照合は多重集合(同一行の複数出現は回数で保全を判定、#80)。
    // before に n 回出る行が after に m < n 回しか無いとき、超過分 n-m 行を
    // 未保全として報告する(集合照合だと同一行の欠落が埋まって検出できない)
    let mut after_remaining: std::collections::HashMap<&str, usize> =
        std::collections::HashMap::new();
    for (_, _, t) in &after_lines {
        *after_remaining.entry(t.as_str()).or_default() += 1;
    }
    let mut before_remaining: std::collections::HashMap<&str, usize> =
        std::collections::HashMap::new();
    for (_, t) in &before_lines {
        *before_remaining.entry(t.as_str()).or_default() += 1;
    }
    let lost: Vec<(usize, &String)> = before_lines
        .iter()
        .filter(|(_, text)| match after_remaining.get_mut(text.as_str()) {
            Some(n) if *n > 0 => {
                *n -= 1;
                false
            }
            _ => true,
        })
        .map(|(line, text)| (*line, text))
        .collect();
    let added: Vec<&(String, usize, String)> = after_lines
        .iter()
        .filter(
            |(_, _, text)| match before_remaining.get_mut(text.as_str()) {
                Some(n) if *n > 0 => {
                    *n -= 1;
                    false
                }
                _ => true,
            },
        )
        .collect();

    // 表示する after は照合に使ったファイルのユニーク列(実体同一ファイルの
    // 重複指定をそのまま表示すると判定根拠とずれて見える、#110)
    let after_display_list: Vec<String> = {
        let mut seen = std::collections::HashSet::new();
        after_lines
            .iter()
            .map(|(d, _, _)| d.clone())
            .filter(|d| seen.insert(d.clone()))
            .collect()
    };
    match format {
        StatsFormat::Terminal => {
            writeln!(
                out,
                "verify-move: {before_display} -> {}",
                after_display_list.join(", ")
            )?;
            let preserved = before_lines.len() - lost.len();
            writeln!(
                out,
                "保全: {}/{} 行(未保全 {} 行、追加 {} 行)",
                preserved,
                before_lines.len(),
                lost.len(),
                added.len()
            )?;
            if !lost.is_empty() {
                writeln!(out, "未保全行(before にあって after に無い):")?;
                for (line, text) in &lost {
                    writeln!(out, "  {before_display}:{line}  {text}")?;
                }
            }
            if !added.is_empty() {
                writeln!(out, "追加行(after にのみ存在、参考):")?;
                for (file, line, text) in &added {
                    writeln!(out, "  {file}:{line}  {text}")?;
                }
            }
        }
        StatsFormat::Json => {
            let value = serde_json::json!({
                "kind": "jaio/verify-move",
                "version": 1,
                "tool": { "name": "jaio", "version": env!("CARGO_PKG_VERSION") },
                "before": before_display,
                "after": after_display_list,
                "preserved": before_lines.len() - lost.len(),
                "total": before_lines.len(),
                "lost_lines": lost.iter().map(|(line, text)| serde_json::json!({
                    "file": before_display, "line": line, "text": text,
                })).collect::<Vec<_>>(),
                "added_lines": added.iter().map(|(file, line, text)| serde_json::json!({
                    "file": file, "line": line, "text": text,
                })).collect::<Vec<_>>(),
                "limitations": [
                    "行単位の一致であって意味の同一性は保証しません(行の並び替え・分割は検出しません)",
                    "見出しレベルの変化とリンク先パスは正規化で吸収します",
                ],
            });
            writeln!(
                out,
                "{}",
                serde_json::to_string_pretty(&value).map_err(|e| anyhow::anyhow!(e))?
            )?;
        }
    }
    Ok(lost.is_empty())
}

#[cfg(test)]
mod verify_move_tests {
    use super::*;

    /// 移動の lossless 検証(#51): 全行が移動先にあれば完全保全
    #[test]
    fn verify_move_reports_full_preservation() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("jaio-vm-{}", std::process::id()));
        fs::create_dir_all(&dir)?;
        fs::write(
            dir.join("before.md"),
            "# 手順\n\n## 準備\n\nまず設定を読み込みます。\n\n## 実行\n\n次に実行します。\n",
        )?;
        // 見出しレベルが変わっていても(## → ###)行としては保全と見なす
        fs::write(
            dir.join("after.md"),
            "### 手順\n\n### 準備\n\nまず設定を読み込みます。\n\n### 実行\n\n次に実行します。\n",
        )?;
        let mut buf = Vec::new();
        let preserved = run_verify_move(
            &dir.join("before.md"),
            &[dir.join("after.md")],
            &mut buf,
            StatsFormat::Terminal,
        )?;
        let out = String::from_utf8(buf)?;
        assert!(preserved, "全行保全のはず: {out}");
        assert!(out.contains("保全: 5/5 行"), "{out}");
        fs::remove_dir_all(&dir).ok();
        Ok(())
    }

    /// 未保全行がある場合は false になり、行番号付きで報告される
    #[test]
    fn verify_move_reports_lost_lines() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("jaio-vm-lost-{}", std::process::id()));
        fs::create_dir_all(&dir)?;
        fs::write(
            dir.join("before.md"),
            "# 手順\n\n一行目の説明です。\n二行目の説明です。\n三行目の説明です。\n",
        )?;
        // 二行目が欠落した移動先
        fs::write(
            dir.join("after.md"),
            "### 手順\n\n一行目の説明です。\n三行目の説明です。\n",
        )?;
        let mut buf = Vec::new();
        let preserved = run_verify_move(
            &dir.join("before.md"),
            &[dir.join("after.md")],
            &mut buf,
            StatsFormat::Json,
        )?;
        assert!(!preserved, "二行目が失われているはず");
        let value: serde_json::Value = serde_json::from_slice(&buf)?;
        assert_eq!(value["kind"], "jaio/verify-move");
        assert_eq!(value["version"], 1);
        assert_eq!(value["preserved"], 3);
        assert_eq!(value["total"], 4);
        let lost = value["lost_lines"].as_array().expect("lost_lines は配列");
        assert_eq!(lost.len(), 1);
        assert_eq!(lost[0]["line"], 4, "before の「二行目」は 4 行目: {lost:?}");
        assert!(
            lost[0]["text"]
                .as_str()
                .is_some_and(|t| t.contains("二行目")),
            "{lost:?}"
        );
        // after 側の見出し行は正規化で before の見出しと一致(追加行にならない)
        assert_eq!(value["added_lines"].as_array().unwrap().len(), 0);
        fs::remove_dir_all(&dir).ok();
        Ok(())
    }

    /// リンク記法の違い(相対パス補正)は簡約して吸収する(#51 の実績パターン)
    #[test]
    fn verify_move_normalizes_links_and_multiple_afters() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("jaio-vm-links-{}", std::process::id()));
        fs::create_dir_all(dir.join("references"))?;
        fs::write(
            dir.join("SKILL.md"),
            "# 概要\n\n[詳細手順](references/procedure.md)を参照します。\n最後の行です。\n",
        )?;
        // 移動先: リンク先パスが変わり、行は 2 ファイルに分散
        fs::write(
            dir.join("references/procedure.md"),
            "### 概要\n\n[詳細手義](../procedure.md)を参照します。\n",
        )?;
        // typo「手義」を混ぜて、正しくは別ファイルに保全されることを確認する
        fs::write(dir.join("references/last.md"), "最後の行です。\n")?;
        let mut buf = Vec::new();
        let preserved = run_verify_move(
            &dir.join("SKILL.md"),
            &[
                dir.join("references/procedure.md"),
                dir.join("references/last.md"),
            ],
            &mut buf,
            StatsFormat::Terminal,
        )?;
        let out = String::from_utf8(buf)?;
        assert!(!preserved, "typo 行(詳細手義)は未保全のはず: {out}");
        assert!(out.contains("保全: 2/3 行"), "{out}");
        // 報告テキストはリンク簡約後の正規化行
        assert!(out.contains("詳細手順を参照します。"), "{out}");
        fs::remove_dir_all(&dir).ok();
        Ok(())
    }

    /// コードブロック内の変更は正規化で吸収されず未保全になる(#66)。
    /// 見出しレベル変更・実リンクの簡約は従来どおり許容される
    #[test]
    fn verify_move_detects_code_block_changes() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("jaio-vm-code-{}", std::process::id()));
        fs::create_dir_all(&dir)?;
        fs::write(
            dir.join("before.md"),
            "# 手順\n\n```python\n# send_notification()\n```\n\n本文です。\n",
        )?;
        fs::write(
            dir.join("after.md"),
            "## 手順\n\n```python\nsend_notification()\n```\n\n本文です。\n",
        )?;
        let mut buf = Vec::new();
        let preserved = run_verify_move(
            &dir.join("before.md"),
            &[dir.join("after.md")],
            &mut buf,
            StatsFormat::Terminal,
        )?;
        let out = String::from_utf8(buf)?;
        assert!(
            !preserved,
            "コード内コメントの削除は内容変更なので未保全のはず: {out}"
        );
        assert!(out.contains("# send_notification()"), "{out}");
        fs::remove_dir_all(&dir).ok();
        Ok(())
    }

    /// setext → ATX の見出し形式変更と引用内見出しのレベル差は内容を
    /// 変えない構文変更なので保全扱いになる(#66)
    #[test]
    fn verify_move_absorbs_setext_and_quoted_heading_changes() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("jaio-vm-hdr-{}", std::process::id()));
        fs::create_dir_all(&dir)?;
        fs::write(
            dir.join("before.md"),
            "見出し\n===\n\n> ## 引用内見出し\n\n本文です。\n",
        )?;
        fs::write(
            dir.join("after.md"),
            "# 見出し\n\n> # 引用内見出し\n\n本文です。\n",
        )?;
        let mut buf = Vec::new();
        let preserved = run_verify_move(
            &dir.join("before.md"),
            &[dir.join("after.md")],
            &mut buf,
            StatsFormat::Terminal,
        )?;
        let out = String::from_utf8(buf)?;
        assert!(
            preserved,
            "見出し形式・引用内レベル差だけの変更は保全のはず: {out}"
        );
        fs::remove_dir_all(&dir).ok();
        Ok(())
    }
}

#[cfg(test)]
mod duplicate_line_tests {
    use super::*;

    /// 同一行が before に 2 回・after に 1 回のとき、超過の 1 行を未保全として
    /// 報告する(#80: 集合照合だと 2/2 行保全と誤報していた)
    #[test]
    fn duplicate_lines_in_before_are_counted() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("jaio-vm-dup-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        let before = dir.join("before.md");
        let after = dir.join("after.md");
        std::fs::write(
            &before,
            "# 手順\n\n共通の行です。\n別の行です。\n共通の行です。\n",
        )?;
        std::fs::write(&after, "## 手順\n\n共通の行です。\n別の行です。\n")?;
        let mut buf = Vec::new();
        let preserved = run_verify_move(
            &before,
            std::slice::from_ref(&after),
            &mut buf,
            StatsFormat::Terminal,
        )?;
        assert!(!preserved, "同一行の片方欠落は未保全のはず");
        let out = String::from_utf8(buf)?;
        assert!(
            out.contains("保全: 3/4 行"),
            "超過分 1 行を未保全として数える(見出し+2 種の実質行): {out}"
        );
        assert!(out.contains("未保全 1 行"), "{out}");
        std::fs::remove_dir_all(&dir).ok();
        Ok(())
    }
}

#[cfg(test)]
mod duplicate_after_argument_tests {
    use super::*;

    /// 同じ after ファイルの重複指定は実体 ID で排除する(#80)。重複指定による
    /// after 側の水増しがあっても、不足行を正しく検出する
    #[test]
    fn duplicate_after_argument_does_not_inflate_lines() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("jaio-vm-dup-after-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        let before = dir.join("before.md");
        let after = dir.join("after.md");
        std::fs::write(
            &before,
            "# 手順\n\n共通の行です。\n別の行です。\n共通の行です。\n",
        )?;
        std::fs::write(&after, "## 手順\n\n共通の行です。\n別の行です。\n")?;
        let after_paths = [after.clone(), after.clone()];
        let mut buf = Vec::new();
        let preserved = run_verify_move(&before, &after_paths, &mut buf, StatsFormat::Terminal)?;
        assert!(!preserved, "重複指定しても不足行の検出は維持するはず");
        let out = String::from_utf8(buf)?;
        assert!(
            out.contains("保全: 3/4 行"),
            "重複指定で after 行を水増ししない: {out}"
        );
        std::fs::remove_dir_all(&dir).ok();
        Ok(())
    }
}
