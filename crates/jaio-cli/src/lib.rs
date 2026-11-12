//! CLI の中核ロジック(設定読み込み・走査・解析実行・統計)。
//! Markdown 走査(ignore)はここに残り、ソースコメント抽出は jaio-collect が担う(設計書 §6.3)。

use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result};
use ignore::WalkBuilder;

use jaio_core::comments::CommentFile;
use jaio_core::config::{
    AnalysisMode, Config, RuleExcludes, SeveritySetting, StyleOverrides, TokenEncoding,
};
use jaio_core::{Diagnostic, Severity};
use jaio_index::{
    PairSearchMethod, SIMILARITY_BIN_WIDTH, bigram_hashes, jaccard_sorted, normalize_text,
    similarity_bin_index, similarity_histogram, top_similar_pairs,
};
use jaio_parse::doc::{Doc, Heading, TextLeaf};
use jaio_parse::morph::{Analyzer, Morph};
use jaio_parse::tokencount::TokenCounter;
use jaio_report::{
    JsonAlgorithms, JsonCandidateSearch, JsonContext, JsonOperationalError, JsonRuleStatus,
    JsonSource, JsonSummary, JsonTargets, PhaseTimings, top_rules, top_slow_rules,
};
use jaio_rules::{
    Corpus, Coverage, DocContext, DocumentRule, SectionSlice, comparison_key, default_corpus_rules,
    default_rules, extract_sections, normalize_path, substantive_lines,
};
use rayon::prelude::*;
use sha2::Digest as _;
use std::cell::RefCell;

/// check コマンドの出力形式(§7.2)。
pub enum OutputFormat {
    /// 人間向けのターミナル出力
    Terminal {
        /// 色付き表示にするか
        color: bool,
    },
    /// LLM 委譲向けの JSON 出力(§7.3)
    Json,
    /// Markdown 形式のレポート出力(§7.2)
    Markdown,
    /// SARIF 2.1.0 出力(CI 連携向け、§7.2)
    Sarif,
}

pub(crate) use check::*;
pub(crate) use collect::*;
pub(crate) use context::*;
pub(crate) use rules_cmd::*;
#[cfg(test)]
pub(crate) use stats::*;

mod check;
mod collect;
mod config;
mod context;
mod corpus;
mod dump;
mod rules_cmd;
mod stats;
#[cfg(test)]
mod tests;
mod verify_move;

pub use check::run_check;
pub use collect::collect_md_files;
pub use config::load_config;
pub use dump::{run_dump_corpus, run_dump_doc, run_dump_morph};
pub use rules_cmd::{explain_rule, list_rules};
pub use stats::{StatsFormat, run_stats};
pub use verify_move::run_verify_move;
