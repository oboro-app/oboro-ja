//! jaio CLI エントリポイント。コマンド体系は設計書 §7.2。

use std::io::{IsTerminal, stdout};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};

use jaio_cli::{
    OutputFormat, StatsFormat, explain_rule, list_rules, load_config, run_check, run_dump_corpus,
    run_dump_doc, run_dump_morph, run_stats, run_verify_move,
};

#[derive(Parser)]
#[command(
    name = "jaio",
    version,
    about = "リポジトリ内の日本語文書を静的解析する"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// check の出力形式(§7.2)。
#[derive(Clone, ValueEnum)]
enum Format {
    /// 人間向けのターミナル出力
    Terminal,
    /// LLM 委譲向けの JSON 出力(§7.3)
    Json,
    /// Markdown 形式のレポート出力
    Md,
    /// SARIF 2.1.0 形式の出力(CI 連携向け)
    Sarif,
}

/// `jaio dump` のダンプ対象種別(Issue #16、設計書 §7.2)。
#[derive(Clone, ValueEnum)]
enum DumpKind {
    /// 文書構造 AST(leaf・heading・links)
    Doc,
    /// 形態素解析結果
    Morph,
    /// Corpus(docs・duplicates・comments・context)
    Corpus,
}

/// stats の出力形式(#50)。lib 側の `StatsFormat` と対応する
#[derive(Clone, ValueEnum)]
enum StatsFormatCli {
    /// 人間向けのターミナル出力(表形式)
    Terminal,
    /// 層別集計の自動化向け JSON 出力
    Json,
}

#[derive(Subcommand)]
enum Command {
    /// ドキュメントを解析して診断を報告する
    Check {
        /// 対象ファイルまたはディレクトリ(未指定時はカレントディレクトリ)
        paths: Vec<PathBuf>,
        /// 色付き出力を無効化する
        #[arg(long)]
        no_color: bool,
        /// 出力形式(既定: terminal)
        #[arg(long, value_enum, default_value = "terminal")]
        format: Format,
        /// error 重大度の診断のみ報告する
        #[arg(long)]
        quiet: bool,
        /// 設定ファイル(未指定時は上位方向へ jaio.toml を探索)
        #[arg(long)]
        config: Option<PathBuf>,
        /// フェーズ別・ルール別の処理時間などの内部計測を表示する(Issue #18)
        #[arg(long)]
        verbose: bool,
    },
    /// トークン統計を表示する
    Stats {
        /// 対象ファイルまたはディレクトリ(未指定時はカレントディレクトリ)
        paths: Vec<PathBuf>,
        /// 文書ペアの bigram Jaccard 類似度分布を表示する(閾値チューニング用)
        #[arg(long)]
        similarity: bool,
        /// 出力形式(既定: terminal。json は層別集計の自動化向け、#50)
        #[arg(long, value_enum, default_value = "terminal")]
        format: StatsFormatCli,
        /// 設定ファイル(未指定時は上位方向へ jaio.toml を探索)
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// ルールの一覧を表示する
    Rules,
    /// 解析の中間データを JSON でダンプする(調査・LLM 委譲の素材)
    Dump {
        /// ダンプ対象の種別
        kind: DumpKind,
        /// 対象パス(doc/morph は 1 ファイル、corpus は未指定時カレントディレクトリ)
        paths: Vec<PathBuf>,
        /// 設定ファイル(未指定時は上位方向へ jaio.toml を探索)
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// ルールの詳細を表示する
    Explain {
        /// ルール ID(例: consistency/style-deviation)
        rule_id: String,
    },
    /// 文書移動の lossless 検証(移動元→移動先の行単位保全、#51)
    VerifyMove {
        /// 移動元ファイル
        before: PathBuf,
        /// 移動先ファイル(1 つ以上)
        #[arg(num_args = 1..)]
        after: Vec<PathBuf>,
        /// 出力形式(既定: terminal)
        #[arg(long, value_enum, default_value = "terminal")]
        format: StatsFormatCli,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let color = stdout().is_terminal();
    let result = match cli.command {
        Command::Check {
            paths,
            no_color,
            format,
            quiet,
            config,
            verbose,
        } => load_config(config.as_deref(), &paths).and_then(|config| {
            let paths = default_paths(paths);
            let format = match format {
                Format::Terminal => OutputFormat::Terminal {
                    color: color && !no_color,
                },
                Format::Json => OutputFormat::Json,
                Format::Md => OutputFormat::Markdown,
                Format::Sarif => OutputFormat::Sarif,
            };
            run_check(&paths, &mut stdout(), &config, &format, quiet, verbose).map(Some)
        }),
        Command::Stats {
            paths,
            similarity,
            format,
            config,
        } => {
            // 終了コード: 解析未完了は 2(stats も check と同じ契約、#93)
            let result = load_config(config.as_deref(), &paths).and_then(|config| {
                let paths = default_paths(paths);
                let format = match format {
                    StatsFormatCli::Terminal => StatsFormat::Terminal,
                    StatsFormatCli::Json => StatsFormat::Json,
                };
                run_stats(&paths, &mut stdout(), &config, similarity, format)
            });
            return match result {
                Ok(true) => ExitCode::SUCCESS,
                Ok(false) => ExitCode::from(2),
                Err(err) => {
                    eprintln!("error: {err:#}");
                    ExitCode::from(2)
                }
            };
        }
        Command::Rules => list_rules(&mut stdout()).map(|()| None),
        Command::Dump {
            kind,
            paths,
            config,
        } => load_config(config.as_deref(), &paths).and_then(|config| {
            let resolved = default_paths(paths);
            match kind {
                // dump doc / morph は対象 1 ファイル(設計書 §7.2)。2 つ目以降を
                // 黙って無視せず、未指定を「Is a directory」等の紛らわしい
                // エラーにしない(#110)
                DumpKind::Doc => {
                    anyhow::ensure!(
                        resolved.len() == 1,
                        "dump doc には対象ファイルを 1 つ指定してください"
                    );
                    run_dump_doc(&resolved[0], &mut stdout(), &config).map(|()| None)
                }
                DumpKind::Morph => {
                    anyhow::ensure!(
                        resolved.len() == 1,
                        "dump morph には対象ファイルを 1 つ指定してください"
                    );
                    run_dump_morph(&resolved[0], &mut stdout(), &config).map(|()| None)
                }
                DumpKind::Corpus => {
                    // check と同じ終了コード契約(解析未完了なら 2、#39)
                    run_dump_corpus(&resolved, &mut stdout(), &config).map(Some)
                }
            }
        }),
        Command::Explain { rule_id } => explain_rule(&rule_id, &mut stdout()).map(|()| None),
        // verify-move は診断サマリではなく保全判定で終了コードを決める
        // (0 = 完全保全、1 = 未保全行あり、§7.2.1)。設定非依存のため
        // load_config を通さない
        Command::VerifyMove {
            before,
            after,
            format,
        } => {
            let format = match format {
                StatsFormatCli::Terminal => StatsFormat::Terminal,
                StatsFormatCli::Json => StatsFormat::Json,
            };
            return match run_verify_move(&before, &after, &mut stdout(), format) {
                Ok(true) => ExitCode::SUCCESS,
                Ok(false) => ExitCode::FAILURE,
                Err(err) => {
                    eprintln!("error: {err:#}");
                    ExitCode::from(2)
                }
            };
        }
    };
    match result {
        // 解析未完了は 2、error 診断ありは 1、それ以外は 0(#36、§7.2)
        Ok(summary) => ExitCode::from(summary.map_or(0, |s| s.exit_code())),
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::from(2)
        }
    }
}

fn default_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    if paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        paths
    }
}
