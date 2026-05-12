use crate::cli::Args;
use anyhow::{Context, Result};
use clap::{CommandFactory, FromArgMatches, ValueEnum};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
struct ConfigFile {
  format: Option<String>,
  sort: Option<String>,
  asc: Option<bool>,
  limit: Option<usize>,
  no_cost: Option<bool>,
  cost: Option<String>,
  cost_per: Option<String>,
  period: Option<String>,
  period_24h: Option<bool>,
  period_7d: Option<bool>,
  period_1m: Option<bool>,
  today: Option<bool>,
  week: Option<bool>,
  month: Option<bool>,
  no_color: Option<bool>,
  human: Option<bool>,
  no_fit: Option<bool>,
  table_width: Option<usize>,
  split_input: Option<bool>,
  bytes: Option<bool>,
  avg: Option<String>,
  no_cache: Option<bool>,
  group_by: Option<Vec<String>>,
  date_bucket: Option<String>,
  since: Option<String>,
  until: Option<String>,
  model: Option<String>,
  provider: Option<String>,
  cwd: Option<String>,
  source: Option<Vec<String>>,
  codex_dir: Option<PathBuf>,
  opencode_db: Option<PathBuf>,
  claude_dir: Option<PathBuf>,
  copilot_dir: Option<Vec<PathBuf>>,
  copilot_cli_dir: Option<Vec<PathBuf>>,
  pricing: Option<PathBuf>,
  verbose: Option<bool>,
}

pub fn parse_args() -> Result<Args> {
  let matches = Args::command().get_matches();
  let mut args = Args::from_arg_matches(&matches)?;
  let config_path = args.config.clone().or_else(default_config_path);

  if !args.no_config {
    if let Some(path) = config_path.as_deref() {
      if path.exists() {
        let config = read_config(path)?;
        apply_config(&mut args, &matches, config)?;
      }
    }
  }

  Ok(args)
}

fn default_config_path() -> Option<PathBuf> {
  std::env::var_os("XDG_CONFIG_HOME")
    .map(PathBuf::from)
    .or_else(|| std::env::var_os("HOME").map(PathBuf::from).map(|p| p.join(".config")))
    .map(|base| base.join("llm-tokei").join("config.toml"))
}

fn read_config(path: &Path) -> Result<ConfigFile> {
  let s = std::fs::read_to_string(path).with_context(|| format!("reading config file {}", path.display()))?;
  toml::from_str(&s).with_context(|| format!("parsing config file {}", path.display()))
}

fn apply_config(args: &mut Args, matches: &clap::ArgMatches, config: ConfigFile) -> Result<()> {
  set(matches, "format", config.format, |v| {
    assign(&mut args.format, parse_value_enum("format", &v)?)
  })?;
  set(matches, "sort", config.sort, |v| assign(&mut args.sort, v))?;
  set(matches, "asc", config.asc, |v| assign(&mut args.asc, v))?;
  set(matches, "limit", config.limit, |v| assign(&mut args.limit, Some(v)))?;
  set(matches, "no_cost", config.no_cost, |v| assign(&mut args.no_cost, v))?;
  set(matches, "cost", config.cost, |v| {
    assign(&mut args.cost, parse_value_enum("cost", &v)?)
  })?;
  set(matches, "cost_per", config.cost_per, |v| {
    assign(&mut args.cost_per, Some(v))
  })?;
  set(matches, "period", config.period, |v| {
    assign(&mut args.period, Some(parse_value_enum("period", &v)?))
  })?;
  set(matches, "period_24h", config.period_24h, |v| {
    assign(&mut args.period_24h, v)
  })?;
  set(matches, "period_7d", config.period_7d, |v| {
    assign(&mut args.period_7d, v)
  })?;
  set(matches, "period_1m", config.period_1m, |v| {
    assign(&mut args.period_1m, v)
  })?;
  set(matches, "today", config.today, |v| assign(&mut args.today, v))?;
  set(matches, "week", config.week, |v| assign(&mut args.week, v))?;
  set(matches, "month", config.month, |v| assign(&mut args.month, v))?;
  set(matches, "no_color", config.no_color, |v| assign(&mut args.no_color, v))?;
  set(matches, "human", config.human, |v| assign(&mut args.human, v))?;
  set(matches, "no_fit", config.no_fit, |v| assign(&mut args.no_fit, v))?;
  set(matches, "table_width", config.table_width, |v| {
    assign(&mut args.table_width, Some(v))
  })?;
  set(matches, "split_input", config.split_input, |v| {
    assign(&mut args.split_input, v)
  })?;
  set(matches, "bytes", config.bytes, |v| assign(&mut args.bytes, v))?;
  set(matches, "avg", config.avg, |v| {
    assign(&mut args.avg, Some(parse_value_enum("avg", &v)?))
  })?;
  set(matches, "no_cache", config.no_cache, |v| assign(&mut args.no_cache, v))?;
  set(matches, "group_by", config.group_by, |v| assign(&mut args.group_by, v))?;
  set(matches, "date_bucket", config.date_bucket, |v| {
    assign(&mut args.date_bucket, parse_value_enum("date-bucket", &v)?)
  })?;
  set(matches, "since", config.since, |v| assign(&mut args.since, Some(v)))?;
  set(matches, "until", config.until, |v| assign(&mut args.until, Some(v)))?;
  set(matches, "model", config.model, |v| assign(&mut args.model, Some(v)))?;
  set(matches, "provider", config.provider, |v| {
    assign(&mut args.provider, Some(v))
  })?;
  set(matches, "cwd", config.cwd, |v| assign(&mut args.cwd, Some(v)))?;
  set(matches, "source", config.source, |v| assign(&mut args.source, Some(v)))?;
  set(matches, "codex_dir", config.codex_dir, |v| {
    assign(&mut args.codex_dir, Some(v))
  })?;
  set(matches, "opencode_db", config.opencode_db, |v| {
    assign(&mut args.opencode_db, Some(v))
  })?;
  set(matches, "claude_dir", config.claude_dir, |v| {
    assign(&mut args.claude_dir, Some(v))
  })?;
  set(matches, "copilot_dir", config.copilot_dir, |v| {
    assign(&mut args.copilot_dir, Some(v))
  })?;
  set(matches, "copilot_cli_dir", config.copilot_cli_dir, |v| {
    assign(&mut args.copilot_cli_dir, Some(v))
  })?;
  set(matches, "pricing", config.pricing, |v| {
    assign(&mut args.pricing, Some(v))
  })?;
  set(matches, "verbose", config.verbose, |v| assign(&mut args.verbose, v))?;
  Ok(())
}

fn set<T, F>(matches: &clap::ArgMatches, id: &'static str, value: Option<T>, apply: F) -> Result<()>
where
  F: FnOnce(T) -> Result<()>,
{
  if let Some(value) = value {
    if matches.value_source(id) != Some(clap::parser::ValueSource::CommandLine) {
      apply(value)?;
    }
  }
  Ok(())
}

fn assign<T>(slot: &mut T, value: T) -> Result<()> {
  *slot = value;
  Ok(())
}

fn parse_value_enum<T>(name: &str, value: &str) -> Result<T>
where
  T: ValueEnum,
{
  T::from_str(value, true).map_err(|_| anyhow::anyhow!("invalid config value for {name}: {value}"))
}
