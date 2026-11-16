use super::*;

/// jaio の規則を登録した登録簿を作る(設計書 §6.4)
pub(crate) fn rule_registry() -> Result<RuleRegistry> {
    let mut registry = RuleRegistry::new();
    register_rules(&mut registry)?;
    Ok(registry)
}

/// ルールの一覧を表示する(`jaio rules`)。
pub fn list_rules(out: &mut impl Write) -> Result<()> {
    for entry in rule_registry()?.entries() {
        writeln!(out, "{}\t{}", entry.id, entry.description)?;
    }
    Ok(())
}

/// ルールの詳細を表示する(`jaio explain <RULE_ID>`)。
pub fn explain_rule(rule_id: &str, out: &mut impl Write) -> Result<()> {
    let registry = rule_registry()?;
    let Some(entry) = registry.get(rule_id) else {
        anyhow::bail!("不明なルール ID です: {rule_id}(jaio rules で一覧を確認してください)")
    };
    writeln!(out, "{}\t{}", entry.id, entry.description)?;
    Ok(())
}
