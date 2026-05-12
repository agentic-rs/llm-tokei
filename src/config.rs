use crate::cli::{Args, AvgBy, DateBucket, Format, Period};
use crate::pricing::CostMode;
use anyhow::{bail, Context, Result};
use clap::{CommandFactory, FromArgMatches, ValueEnum};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
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
  #[serde(rename = "24h")]
  period_24h: Option<bool>,
  #[serde(rename = "7d")]
  period_7d: Option<bool>,
  #[serde(rename = "1m")]
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
  let raw_args: Vec<String> = std::env::args().collect();
  let raw_matches = Args::command().get_matches_from(raw_args.clone());
  let no_default = raw_matches.get_flag("no_default");
  let config_path = raw_matches
    .get_one::<PathBuf>("config")
    .cloned()
    .or_else(default_config_path);

  let mut parse_args = raw_args.clone();
  if !no_default && !raw_matches.get_flag("no_config") {
    if raw_matches.subcommand_name() != Some("config") {
      if let Some(path) = config_path.as_deref() {
        if path.exists() {
          let config = read_config(path)?;
          let defaults = config_to_args(&config, &raw_matches);
          if !defaults.is_empty() {
            parse_args = vec![raw_args[0].clone()];
            parse_args.extend(defaults);
            parse_args.extend(raw_args.iter().skip(1).cloned());
          }
        }
      }
    }
  }

  let matches = Args::command().get_matches_from(parse_args);
  let args = Args::from_arg_matches(&matches)?;

  if args.save_default {
    let save_config = args_from_matches(&raw_matches)?;
    let path = args
      .config
      .clone()
      .or_else(default_config_path)
      .context("cannot determine config path")?;
    save_defaults(&path, &save_config)?;
  }

  Ok(args)
}

pub fn default_config_path() -> Option<PathBuf> {
  std::env::var_os("XDG_CONFIG_HOME")
    .map(PathBuf::from)
    .or_else(|| std::env::var_os("HOME").map(PathBuf::from).map(|p| p.join(".config")))
    .map(|base| base.join("llm-tokei").join("config.toml"))
}

pub fn save_default_arg_string(path: &Path, arg_string: &str) -> Result<()> {
  let mut args = vec!["llm-tokei".to_string()];
  args.extend(split_arg_string(arg_string));
  let matches = Args::command().try_get_matches_from(args)?;
  if matches.subcommand().is_some() {
    bail!("config args only accepts main llm-tokei flags, not subcommands");
  }
  let config = args_from_matches(&matches)?;
  save_defaults(path, &config)
}

pub fn reset_defaults(path: &Path) -> Result<()> {
  save_config(path, &ConfigFile::default())
}

fn read_config(path: &Path) -> Result<ConfigFile> {
  let s = std::fs::read_to_string(path).with_context(|| format!("reading config file {}", path.display()))?;
  toml::from_str(&s).with_context(|| format!("parsing config file {}", path.display()))
}

fn save_defaults(path: &Path, config: &ConfigFile) -> Result<()> {
  save_config(path, config)
}

fn save_config(path: &Path, config: &ConfigFile) -> Result<()> {
  if let Some(parent) = path.parent() {
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
  }
  let toml = toml::to_string_pretty(config).context("serializing config")?;
  std::fs::write(path, toml).with_context(|| format!("writing config file {}", path.display()))
}

fn args_from_matches(matches: &clap::ArgMatches) -> Result<ConfigFile> {
  let mut out = ConfigFile::default();
  if cli_set(matches, "format") {
    out.format = value_name::<Format>(matches, "format");
  }
  if cli_set(matches, "sort") {
    out.sort = matches.get_one::<String>("sort").cloned();
  }
  if cli_set(matches, "asc") {
    out.asc = Some(matches.get_flag("asc"));
  }
  if cli_set(matches, "limit") {
    out.limit = matches.get_one::<usize>("limit").copied();
  }
  if cli_set(matches, "no_cost") {
    out.no_cost = Some(matches.get_flag("no_cost"));
  }
  if cli_set(matches, "cost") {
    out.cost = value_name::<CostMode>(matches, "cost");
  }
  if cli_set(matches, "cost_per") {
    out.cost_per = matches.get_one::<String>("cost_per").cloned();
  }
  if cli_set(matches, "period") {
    out.period = value_name::<Period>(matches, "period");
  }
  out.period_24h = flag_if_set(matches, "period_24h");
  out.period_7d = flag_if_set(matches, "period_7d");
  out.period_1m = flag_if_set(matches, "period_1m");
  out.today = flag_if_set(matches, "today");
  out.week = flag_if_set(matches, "week");
  out.month = flag_if_set(matches, "month");
  out.no_color = flag_if_set(matches, "no_color");
  out.human = flag_if_set(matches, "human");
  out.no_fit = flag_if_set(matches, "no_fit");
  if cli_set(matches, "table_width") {
    out.table_width = matches.get_one::<usize>("table_width").copied();
  }
  out.split_input = flag_if_set(matches, "split_input");
  out.bytes = flag_if_set(matches, "bytes");
  if cli_set(matches, "avg") {
    out.avg = value_name::<AvgBy>(matches, "avg");
  }
  out.no_cache = flag_if_set(matches, "no_cache");
  if cli_set(matches, "group_by") {
    out.group_by = matches.get_many::<String>("group_by").map(|v| v.cloned().collect());
  }
  if cli_set(matches, "date_bucket") {
    out.date_bucket = value_name::<DateBucket>(matches, "date_bucket");
  }
  if cli_set(matches, "since") {
    out.since = matches.get_one::<String>("since").cloned();
  }
  if cli_set(matches, "until") {
    out.until = matches.get_one::<String>("until").cloned();
  }
  if cli_set(matches, "model") {
    out.model = matches.get_one::<String>("model").cloned();
  }
  if cli_set(matches, "provider") {
    out.provider = matches.get_one::<String>("provider").cloned();
  }
  if cli_set(matches, "cwd") {
    out.cwd = matches.get_one::<String>("cwd").cloned();
  }
  if cli_set(matches, "source") {
    out.source = matches.get_many::<String>("source").map(|v| v.cloned().collect());
  }
  out.codex_dir = path_if_set(matches, "codex_dir");
  out.opencode_db = path_if_set(matches, "opencode_db");
  out.claude_dir = path_if_set(matches, "claude_dir");
  out.copilot_dir = paths_if_set(matches, "copilot_dir");
  out.copilot_cli_dir = paths_if_set(matches, "copilot_cli_dir");
  out.pricing = path_if_set(matches, "pricing");
  out.verbose = flag_if_set(matches, "verbose");
  Ok(out)
}

fn config_to_args(config: &ConfigFile, current: &clap::ArgMatches) -> Vec<String> {
  let mut out = Vec::new();
  push_opt(&mut out, current, "format", "--format", config.format.as_deref());
  push_opt(&mut out, current, "sort", "--sort", config.sort.as_deref());
  push_bool(&mut out, current, "asc", "--asc", config.asc);
  push_opt_display(&mut out, current, "limit", "--limit", config.limit);
  push_bool(&mut out, current, "no_cost", "--no-cost", config.no_cost);
  push_opt(&mut out, current, "cost", "--cost", config.cost.as_deref());
  push_opt(&mut out, current, "cost_per", "--cost-per", config.cost_per.as_deref());
  push_opt(&mut out, current, "period", "--period", config.period.as_deref());
  push_bool(&mut out, current, "period_24h", "--24h", config.period_24h);
  push_bool(&mut out, current, "period_7d", "--7d", config.period_7d);
  push_bool(&mut out, current, "period_1m", "--1m", config.period_1m);
  push_bool(&mut out, current, "today", "--today", config.today);
  push_bool(&mut out, current, "week", "--week", config.week);
  push_bool(&mut out, current, "month", "--month", config.month);
  push_bool(&mut out, current, "no_color", "--no-color", config.no_color);
  push_bool(&mut out, current, "human", "--human", config.human);
  push_bool(&mut out, current, "no_fit", "--no-fit", config.no_fit);
  push_opt_display(&mut out, current, "table_width", "--table-width", config.table_width);
  push_bool(&mut out, current, "split_input", "--split-input", config.split_input);
  push_bool(&mut out, current, "bytes", "--bytes", config.bytes);
  push_opt(&mut out, current, "avg", "--avg", config.avg.as_deref());
  push_bool(&mut out, current, "no_cache", "--no-cache", config.no_cache);
  push_list(&mut out, current, "group_by", "--group-by", config.group_by.as_deref());
  push_opt(
    &mut out,
    current,
    "date_bucket",
    "--date-bucket",
    config.date_bucket.as_deref(),
  );
  push_opt(&mut out, current, "since", "--since", config.since.as_deref());
  push_opt(&mut out, current, "until", "--until", config.until.as_deref());
  push_opt(&mut out, current, "model", "--model", config.model.as_deref());
  push_opt(&mut out, current, "provider", "--provider", config.provider.as_deref());
  push_opt(&mut out, current, "cwd", "--cwd", config.cwd.as_deref());
  push_list(&mut out, current, "source", "--source", config.source.as_deref());
  push_path(
    &mut out,
    current,
    "codex_dir",
    "--codex-dir",
    config.codex_dir.as_deref(),
  );
  push_path(
    &mut out,
    current,
    "opencode_db",
    "--opencode-db",
    config.opencode_db.as_deref(),
  );
  push_path(
    &mut out,
    current,
    "claude_dir",
    "--claude-dir",
    config.claude_dir.as_deref(),
  );
  push_paths(
    &mut out,
    current,
    "copilot_dir",
    "--copilot-dir",
    config.copilot_dir.as_deref(),
  );
  push_paths(
    &mut out,
    current,
    "copilot_cli_dir",
    "--copilot-cli-dir",
    config.copilot_cli_dir.as_deref(),
  );
  push_path(&mut out, current, "pricing", "--pricing", config.pricing.as_deref());
  push_bool(&mut out, current, "verbose", "--verbose", config.verbose);
  out
}

fn cli_set(matches: &clap::ArgMatches, id: &str) -> bool {
  matches.value_source(id) == Some(clap::parser::ValueSource::CommandLine)
}

fn flag_if_set(matches: &clap::ArgMatches, id: &str) -> Option<bool> {
  cli_set(matches, id).then(|| matches.get_flag(id))
}

fn value_name<T>(matches: &clap::ArgMatches, id: &str) -> Option<String>
where
  T: ValueEnum + Send + Sync + 'static,
{
  let value = matches.get_one::<T>(id)?;
  value.to_possible_value().map(|v| v.get_name().to_string())
}

fn path_if_set(matches: &clap::ArgMatches, id: &str) -> Option<PathBuf> {
  cli_set(matches, id)
    .then(|| matches.get_one::<PathBuf>(id).cloned())
    .flatten()
}

fn paths_if_set(matches: &clap::ArgMatches, id: &str) -> Option<Vec<PathBuf>> {
  cli_set(matches, id)
    .then(|| matches.get_many::<PathBuf>(id).map(|v| v.cloned().collect()))
    .flatten()
}

fn push_opt(out: &mut Vec<String>, current: &clap::ArgMatches, id: &str, flag: &str, value: Option<&str>) {
  if cli_set(current, id) {
    return;
  }
  if let Some(value) = value {
    out.push(flag.to_string());
    out.push(value.to_string());
  }
}

fn push_opt_display<T: std::fmt::Display>(
  out: &mut Vec<String>,
  current: &clap::ArgMatches,
  id: &str,
  flag: &str,
  value: Option<T>,
) {
  if cli_set(current, id) {
    return;
  }
  if let Some(value) = value {
    out.push(flag.to_string());
    out.push(value.to_string());
  }
}

fn push_bool(out: &mut Vec<String>, current: &clap::ArgMatches, id: &str, flag: &str, value: Option<bool>) {
  if cli_set(current, id) {
    return;
  }
  if value == Some(true) {
    out.push(flag.to_string());
  }
}

fn push_list(out: &mut Vec<String>, current: &clap::ArgMatches, id: &str, flag: &str, values: Option<&[String]>) {
  if cli_set(current, id) {
    return;
  }
  if let Some(values) = values {
    if !values.is_empty() {
      out.push(flag.to_string());
      out.push(values.join(","));
    }
  }
}

fn push_path(out: &mut Vec<String>, current: &clap::ArgMatches, id: &str, flag: &str, value: Option<&Path>) {
  if cli_set(current, id) {
    return;
  }
  if let Some(value) = value {
    out.push(flag.to_string());
    out.push(value.to_string_lossy().to_string());
  }
}

fn push_paths(out: &mut Vec<String>, current: &clap::ArgMatches, id: &str, flag: &str, values: Option<&[PathBuf]>) {
  if cli_set(current, id) {
    return;
  }
  if let Some(values) = values {
    for value in values {
      out.push(flag.to_string());
      out.push(value.to_string_lossy().to_string());
    }
  }
}

fn split_arg_string(input: &str) -> Vec<String> {
  input.split_whitespace().map(str::to_string).collect()
}
