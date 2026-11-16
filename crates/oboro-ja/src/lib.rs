//! oboro-ja の入口(設計書 §6.3)。
//!
//! ほかの実行ファイルへ組み込むときの唯一の依存先。`--format json` の直列化と
//! `docs/schema/` の文字列を公開する。運用の概念は持たない。

pub mod json;
pub mod schema;

/// `jaio check --format json` の JSON Schema 本文(`docs/schema/report.schema.json`)。
/// 組み込み側が出力を照合するときの正本。
pub const SCHEMA: &str = schema::REPORT;

pub use json::{JsonReport, render_json};
pub use oboro_ja_core::{Diagnostic, Measurement, MeasurementStatus, Severity, Span};
