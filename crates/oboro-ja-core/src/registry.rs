//! 規則の登録簿(設計書 §6.4)。規則 ID の持ち主を 1 つに決める。
//!
//! 複数の部品(診断・推敲など)を 1 つの実行ファイルへ組み込むとき、各部品が
//! 同じ登録簿へ規則を登録する。同じ ID の二重登録をエラーにして、規則の
//! 持ち主が 2 つになるのを防ぐ。重さと許可は `[lint]` の設定で制御し、
//! 設定に書かれた規則 ID をこの登録簿と照合する。

use anyhow::{Result, bail};

use crate::config::LintConfig;

/// 登録する規則 1 件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleEntry {
    /// 規則 ID(例: "consistency/style-deviation")
    pub id: &'static str,
    /// 持ち主(規則を実装する部品の名前。例: "jaio")
    pub owner: &'static str,
    /// 規則の説明
    pub description: &'static str,
}

/// 規則の登録簿。登録順を保つ。
#[derive(Debug, Default)]
pub struct RuleRegistry {
    entries: Vec<RuleEntry>,
}

impl RuleRegistry {
    /// 空の登録簿を作る
    pub fn new() -> Self {
        Self::default()
    }

    /// 規則を登録する。登録済みの ID は、持ち主が同じでもエラーにする
    /// (1 規則 1 持ち主)
    pub fn register(&mut self, entry: RuleEntry) -> Result<()> {
        if let Some(existing) = self.get(entry.id) {
            bail!(
                "規則 ID が重複しています: {}(登録済みの持ち主: {}、登録しようとした持ち主: {})",
                entry.id,
                existing.owner,
                entry.owner
            );
        }
        self.entries.push(entry);
        Ok(())
    }

    /// 規則 ID から登録内容を引く
    pub fn get(&self, id: &str) -> Option<&RuleEntry> {
        self.entries.iter().find(|entry| entry.id == id)
    }

    /// 登録内容の一覧(登録順)
    pub fn entries(&self) -> &[RuleEntry] {
        &self.entries
    }

    /// `[lint.rules]` と `[lint.rule_excludes]` に書かれた規則 ID を照合する。
    /// ID の typo は黙って無効化されるため、未登録の ID を設定エラーにする(#36、#75)。
    /// 「既知」の一覧は辞書順(登録順に依存しない決定的な表示、#110)
    pub fn validate_lint(&self, lint: &LintConfig) -> Result<()> {
        let mut known: Vec<&'static str> = self.entries.iter().map(|entry| entry.id).collect();
        known.sort_unstable();
        let known_list = known.join(", ");
        for id in lint.rules.keys() {
            if !known.contains(&id.as_str()) {
                bail!("lint.rules に未知のルール ID があります: {id}(既知: {known_list})");
            }
        }
        for id in lint.rule_excludes.keys() {
            if !known.contains(&id.as_str()) {
                bail!("lint.rule_excludes に未知のルール ID があります: {id}(既知: {known_list})");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn entry(id: &'static str, owner: &'static str) -> RuleEntry {
        RuleEntry {
            id,
            owner,
            description: "説明",
        }
    }

    #[test]
    fn keeps_registration_order_and_looks_up_by_id() -> Result<()> {
        let mut registry = RuleRegistry::new();
        registry.register(entry("structure/heading-skip", "jaio"))?;
        registry.register(entry("efficiency/filler", "jaio"))?;
        let ids: Vec<&str> = registry.entries().iter().map(|e| e.id).collect();
        assert_eq!(ids, vec!["structure/heading-skip", "efficiency/filler"]);
        assert_eq!(
            registry.get("efficiency/filler").map(|e| e.owner),
            Some("jaio")
        );
        assert!(registry.get("no/such-rule").is_none());
        Ok(())
    }

    #[test]
    fn rejects_second_owner_for_same_rule() -> Result<()> {
        let mut registry = RuleRegistry::new();
        registry.register(entry("efficiency/filler", "jaio"))?;
        // 別の部品が同じ ID を登録すると、両方の持ち主を示してエラーになる
        let err = registry
            .register(entry("efficiency/filler", "readable"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("efficiency/filler"), "{err}");
        assert!(err.contains("jaio") && err.contains("readable"), "{err}");
        // 同じ持ち主の二重登録も拒む
        assert!(
            registry
                .register(entry("efficiency/filler", "jaio"))
                .is_err()
        );
        assert_eq!(registry.entries().len(), 1, "失敗した登録は残らない");
        Ok(())
    }

    #[test]
    fn validate_lint_rejects_unregistered_ids() -> Result<()> {
        let mut registry = RuleRegistry::new();
        registry.register(entry("structure/heading-skip", "jaio"))?;
        registry.register(entry("efficiency/filler", "jaio"))?;

        let ok = Config::from_toml("[lint.rules]\n\"efficiency/filler\" = \"allow\"\n")?;
        registry.validate_lint(&ok.lint)?;

        let bad = Config::from_toml("[lint.rules]\n\"no/such-rule\" = \"warn\"\n")?;
        assert_eq!(
            registry.validate_lint(&bad.lint).unwrap_err().to_string(),
            "lint.rules に未知のルール ID があります: no/such-rule(既知: efficiency/filler, structure/heading-skip)"
        );

        let bad =
            Config::from_toml("[lint.rule_excludes]\n\"no/such-rule\" = [\"templates/**\"]\n")?;
        assert_eq!(
            registry.validate_lint(&bad.lint).unwrap_err().to_string(),
            "lint.rule_excludes に未知のルール ID があります: no/such-rule(既知: efficiency/filler, structure/heading-skip)"
        );
        Ok(())
    }
}
