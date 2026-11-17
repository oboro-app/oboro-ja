//! `--format json` の直列化(設計書 §7.3)。
//!
//! 単体 CLI もこの module を通して同じ JSON を出す。

pub use oboro_jaio::report::{
    JsonAlgorithms, JsonCandidateSearch, JsonContext, JsonOperationalError, JsonReport,
    JsonRuleStatus, JsonSource, JsonSummary, JsonTargets, SCHEMA_VERSION, render_json,
};
