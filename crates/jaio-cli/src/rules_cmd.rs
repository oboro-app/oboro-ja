use super::*;

/// 実装済みルール ID の一覧(lint.rules の typo 検出用、#36)
pub(crate) fn known_rule_ids() -> Vec<&'static str> {
    let config = Config::default();
    default_rules(&config)
        .iter()
        .map(|r| r.id())
        .chain(default_corpus_rules(&config, &[]).iter().map(|r| r.id()))
        .collect()
}

/// ルールの一覧を表示する(`jaio rules`)。
pub fn list_rules(out: &mut impl Write) -> Result<()> {
    for rule in default_rules(&Config::default()) {
        writeln!(out, "{}\t{}", rule.id(), rule.description())?;
    }
    for rule in default_corpus_rules(&Config::default(), &[]) {
        writeln!(out, "{}\t{}", rule.id(), rule.description())?;
    }
    Ok(())
}

/// ルールの詳細を表示する(`jaio explain <RULE_ID>`)。
pub fn explain_rule(rule_id: &str, out: &mut impl Write) -> Result<()> {
    for rule in default_rules(&Config::default()) {
        if rule.id() == rule_id {
            writeln!(out, "{}\t{}", rule.id(), rule.description())?;
            return Ok(());
        }
    }
    for rule in default_corpus_rules(&Config::default(), &[]) {
        if rule.id() == rule_id {
            writeln!(out, "{}\t{}", rule.id(), rule.description())?;
            return Ok(());
        }
    }
    anyhow::bail!("不明なルール ID です: {rule_id}(jaio rules で一覧を確認してください)")
}
