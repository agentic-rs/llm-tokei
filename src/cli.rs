use clap::{Parser, ValueEnum};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum Format {
  Table,
  Json,
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum DateBucket {
  Day,
  Week,
  Month,
}

impl DateBucket {
  pub fn as_str(&self) -> &'static str {
    match self {
      DateBucket::Day => "day",
      DateBucket::Week => "week",
      DateBucket::Month => "month",
    }
  }
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum Period {
  Today,
  #[value(name = "7d")]
  Week,
  Month,
}

/// Token usage stats for Codex and OpenCode sessions.
#[derive(Debug, Parser)]
#[command(name = "llm-tokei", version, about)]
pub struct Args {
  /// Comma-separated source list: codex,opencode,claude,copilot (default: all).
  #[arg(long, value_delimiter = ',')]
  pub source: Option<Vec<String>>,

  /// Override Codex sessions root (default: $CODEX_HOME/sessions or ~/.codex/sessions).
  #[arg(long)]
  pub codex_dir: Option<PathBuf>,

  /// Override OpenCode database path (default: ~/.local/share/opencode/opencode.db).
  #[arg(long)]
  pub opencode_db: Option<PathBuf>,

  /// Override Claude Code projects root (default: $CLAUDE_HOME/projects or ~/.claude/projects).
  #[arg(long)]
  pub claude_dir: Option<PathBuf>,

  /// Override Copilot Chat workspaceStorage root (default: VS Code / Insiders / VSCodium / Cursor user dirs).
  /// Repeatable; if unset, all known defaults are scanned.
  #[arg(long)]
  pub copilot_dir: Option<Vec<PathBuf>>,

  /// Shortcut: filter to a recent time window (today / 7d / month).
  #[arg(long, value_enum)]
  pub period: Option<Period>,

  /// Filter: include records on/after this time (e.g. 7d, 24h, 2025-04-01, RFC3339).
  #[arg(long)]
  pub since: Option<String>,

  /// Filter: include records on/before this time.
  #[arg(long)]
  pub until: Option<String>,

  /// Filter: model name glob (e.g. "claude-*").
  #[arg(long)]
  pub model: Option<String>,

  /// Filter: provider glob.
  #[arg(long)]
  pub provider: Option<String>,

  /// Filter: cwd glob.
  #[arg(long)]
  pub cwd: Option<String>,

  /// Grouping dimensions, comma-separated: source,model,provider,project,date,session.
  #[arg(long, value_delimiter = ',', default_value = "source,model")]
  pub group_by: Vec<String>,

  /// Date bucket unit (used when grouping by date).
  #[arg(long, value_enum, default_value_t = DateBucket::Day)]
  pub date_bucket: DateBucket,

  /// Output format.
  #[arg(long, value_enum, default_value_t = Format::Table)]
  pub format: Format,

  /// Sort key: total|input|output|cost|date|turns.
  #[arg(long, default_value = "total")]
  pub sort: String,

  /// Sort ascending instead of descending.
  #[arg(long)]
  pub asc: bool,

  /// Limit number of rows.
  #[arg(long)]
  pub limit: Option<usize>,

  /// Disable ANSI colors.
  #[arg(long)]
  pub no_color: bool,

  /// Hide cost column.
  #[arg(long)]
  pub no_cost: bool,

  /// Override/extend pricing table (JSON file).
  #[arg(long)]
  pub pricing: Option<PathBuf>,

  /// Disable the usage cache (re-parse all source files).
  #[arg(long)]
  pub no_cache: bool,

  /// Print parsing warnings.
  #[arg(short, long)]
  pub verbose: bool,
}
