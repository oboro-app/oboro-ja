//! oboro-jaio: jaio の診断(設計書 §6.3)。共通の土台 oboro-ja-core の上に載る。
//!
//! 読み取り専用の解析だけを行い、文書を書き換えない(設計書 §7.4)。

pub mod collect;
pub mod comments;
pub mod index;
pub mod report;
pub mod rules;
