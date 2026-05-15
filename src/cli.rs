use crate::pricing::CostMode;
use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

const CONFIG_HELP: &str = "Config";

fn default_format() -> Format {
  Format::Table
}

fn default_sort() -> String {
  "total".to_string()
}

fn default_cost() -> CostMode {
  CostMode::Mixed
}

fn default_group_by() -> Vec<String> {
  vec!["source".to_string(), "model".to_string()]
}

fn default_date_bucket() -> DateBucket {
  DateBucket::Day
}

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
pub enum AvgBy {
  Turn,
  Round,
  Session,
}

/// Token usage stats for local LLM agent sessions.
#[derive(Debug, Parser)]
#[command(name = "llm-tokei", version, about, disable_help_flag = true)]
pub struct Args {
  /// Output format.
  #[arg(long, value_enum, default_value_t = default_format(), help_heading = "Output")]
  pub format: Format,

  /// Sort key: total|input|output|cost|date|turns.
  #[arg(long, default_value_t = default_sort(), help_heading = "Output")]
  pub sort: String,

  /// Sort ascending instead of descending.
  #[arg(long, help_heading = "Output")]
  pub asc: bool,

  /// Limit number of rows.
  #[arg(long, help_heading = "Output")]
  pub limit: Option<usize>,

  /// Hide cost column.
  #[arg(long, help_heading = "Output")]
  pub no_cost: bool,

  /// Cost mode: actual, mixed, or official.
  #[arg(long, value_enum, default_value_t = default_cost(), help_heading = "Output")]
  pub cost: CostMode,

  /// Add top cost split columns for a grouping dimension.
  #[arg(long, help_heading = "Output")]
  pub cost_per: Option<String>,

  /// Filter to a recent or calendar time window (e.g. 3d, 12h, 2w, today, week, month).
  #[arg(long, help_heading = "Period", conflicts_with_all = ["period_24h", "period_7d", "period_1m", "today", "week", "month"])]
  pub period: Option<String>,

  /// Shortcut for `--period 24h`.
  #[arg(long = "24h", help_heading = "Period")]
  pub period_24h: bool,

  /// Shortcut for `--period 7d`.
  #[arg(long = "7d", help_heading = "Period")]
  pub period_7d: bool,

  /// Shortcut for `--period 1m`.
  #[arg(long = "1m", help_heading = "Period")]
  pub period_1m: bool,

  /// Shortcut for `--period today`.
  #[arg(long, help_heading = "Period")]
  pub today: bool,

  /// Shortcut for `--period week`.
  #[arg(long, help_heading = "Period")]
  pub week: bool,

  /// Shortcut for `--period month`.
  #[arg(long, help_heading = "Period")]
  pub month: bool,

  /// Disable ANSI colors.
  #[arg(long, help_heading = "Table")]
  pub no_color: bool,

  /// Show human-readable usage values in table output.
  #[arg(short = 'h', long, help_heading = "Table")]
  pub human: bool,

  /// Disable automatic table column fitting.
  #[arg(long, conflicts_with = "table_width", help_heading = "Table")]
  pub no_fit: bool,

  /// Force table output to fit this width.
  #[arg(long, help_heading = "Table")]
  pub table_width: Option<usize>,

  /// Show uncached input only.
  #[arg(long, help_heading = "Table")]
  pub split_input: bool,

  /// Show input/output in bytes instead of tokens.
  #[arg(long, help_heading = "Table")]
  pub bytes: bool,

  /// Show per-unit averages in table output: turn|round|session.
  #[arg(long, value_enum, help_heading = "Table")]
  pub avg: Option<AvgBy>,

  /// Disable the usage cache (re-parse all source files).
  #[arg(long, help_heading = "Cache")]
  pub no_cache: bool,

  /// Grouping dimensions, comma-separated: source,model,provider,project,date,session.
  #[arg(
    long,
    value_delimiter = ',',
    default_values_t = default_group_by(),
    help_heading = "Grouping"
  )]
  pub group_by: Vec<String>,

  /// Date bucket unit (used when grouping by date).
  #[arg(long, value_enum, default_value_t = default_date_bucket(), help_heading = "Grouping")]
  pub date_bucket: DateBucket,

  /// Config file path (default: $XDG_CONFIG_HOME/llm-tokei.toml).
  #[arg(long, help_heading = CONFIG_HELP)]
  pub config: Option<PathBuf>,

  /// Disable loading the config file.
  #[arg(long, help_heading = CONFIG_HELP)]
  pub no_config: bool,

  /// Save current main CLI flags as config defaults, then run.
  #[arg(long, help_heading = CONFIG_HELP)]
  pub save_default: bool,

  /// Do not apply saved config defaults for this run.
  #[arg(long, help_heading = CONFIG_HELP)]
  pub no_default: bool,

  /// Filter: include records on/after this time (e.g. 7d, 24h, 2025-04-01, RFC3339).
  #[arg(long, help_heading = "Filters")]
  pub since: Option<String>,

  /// Filter: include records on/before this time.
  #[arg(long, help_heading = "Filters")]
  pub until: Option<String>,

  /// Filter: model name glob (e.g. "claude-*").
  #[arg(long, help_heading = "Filters")]
  pub model: Option<String>,

  /// Filter: provider glob.
  #[arg(long, help_heading = "Filters")]
  pub provider: Option<String>,

  /// Filter: cwd glob.
  #[arg(long, help_heading = "Filters")]
  pub cwd: Option<String>,

  /// Comma-separated source list: codex,opencode,claude,copilot,copilot-cli (default: all).
  #[arg(long, value_delimiter = ',', help_heading = "Sources")]
  pub source: Option<Vec<String>>,

  /// Override Codex sessions root (default: $CODEX_HOME/sessions or ~/.codex/sessions).
  #[arg(long, help_heading = "Sources")]
  pub codex_dir: Option<PathBuf>,

  /// Override OpenCode database path (default: ~/.local/share/opencode/opencode.db).
  #[arg(long, help_heading = "Sources")]
  pub opencode_db: Option<PathBuf>,

  /// Override Claude Code projects root (default: $CLAUDE_HOME/projects or ~/.claude/projects).
  #[arg(long, help_heading = "Sources")]
  pub claude_dir: Option<PathBuf>,

  /// Override Copilot Chat workspaceStorage root (default: VS Code / Insiders / VSCodium / Cursor user dirs).
  /// Repeatable; if unset, all known defaults are scanned.
  #[arg(long, help_heading = "Sources")]
  pub copilot_dir: Option<Vec<PathBuf>>,

  /// Override GitHub Copilot CLI state root (default: ~/.copilot/session-state).
  /// Repeatable; if unset, all known defaults are scanned.
  #[arg(long, help_heading = "Sources")]
  pub copilot_cli_dir: Option<Vec<PathBuf>>,

  /// Override/extend pricing table (JSON file).
  #[arg(long, help_heading = "Pricing")]
  pub pricing: Option<PathBuf>,

  /// Print help.
  #[arg(long, action = clap::ArgAction::HelpLong, help_heading = "Diagnostics")]
  pub help: Option<bool>,

  /// Print parsing warnings.
  #[arg(short, long, help_heading = "Diagnostics")]
  pub verbose: bool,

  #[command(subcommand)]
  pub cmd: Option<Cmd>,
}

#[derive(Debug, Subcommand)]
pub enum Cmd {
  /// Manage llm-tokei config defaults.
  Config {
    #[command(subcommand)]
    cmd: ConfigCmd,
  },

  /// Fetch current models.dev prices into the runtime price cache.
  Update {
    /// Print help.
    #[arg(long, action = clap::ArgAction::HelpLong, help_heading = "Diagnostics")]
    help: Option<bool>,
  },

  /// Dump per-session JSONL transcripts of user-side messages.
  ///
  /// With `--out`, writes one `<session-id>.jsonl` per session. Without
  /// `--out`, writes comment headers plus JSONL records to stdout.
  Dump {
    /// Dump GitHub Copilot Chat sessions.
    #[arg(long, help_heading = "Source Selection", display_order = 10)]
    copilot: bool,
    /// Dump GitHub Copilot CLI sessions.
    #[arg(long = "copilot-cli", help_heading = "Source Selection", display_order = 11)]
    copilot_cli: bool,
    /// Dump OpenAI Codex CLI sessions.
    #[arg(long, help_heading = "Source Selection", display_order = 12)]
    codex: bool,
    /// Input session JSONL files. If omitted, sessions are discovered from
    /// the selected source's configured/default session roots.
    files: Vec<PathBuf>,
    /// Output directory (created if missing).
    #[arg(long, short = 'o', help_heading = "Output", display_order = 20)]
    out: Option<PathBuf>,
    /// Print help.
    #[arg(long, action = clap::ArgAction::HelpLong, help_heading = "Diagnostics", display_order = 30)]
    help: Option<bool>,
  },
}

#[derive(Debug, Subcommand)]
pub enum ConfigCmd {
  /// Print the current config file.
  List {
    /// Print help.
    #[arg(long, action = clap::ArgAction::HelpLong, help_heading = "Diagnostics")]
    help: Option<bool>,
  },

  /// Parse CLI args and save them as config defaults.
  Args {
    /// Argument string to parse and save, e.g. `--cost official --group-by provider`.
    args: Option<String>,
    /// Clear saved main-flag defaults from the config file.
    #[arg(long)]
    reset: bool,
    /// Print help.
    #[arg(long, action = clap::ArgAction::HelpLong, help_heading = "Diagnostics")]
    help: Option<bool>,
  },
}
