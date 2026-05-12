use crate::cli::{Args, AvgBy, DateBucket, Format, Period};
use crate::pricing::CostMode;
use anyhow::{bail, Context, Result};
use clap::{CommandFactory, FromArgMatches, ValueEnum};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use toml::Value;

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(default, rename_all = "kebab-case")]
struct ConfigFile {
  #[serde(skip_serializing_if = "is_default")]
  output: OutputConfig,
  #[serde(skip_serializing_if = "is_default")]
  period: PeriodConfig,
  #[serde(skip_serializing_if = "is_default")]
  table: TableConfig,
  #[serde(skip_serializing_if = "is_default")]
  cache: CacheConfig,
  #[serde(skip_serializing_if = "is_default")]
  grouping: GroupingConfig,
  #[serde(skip_serializing_if = "is_default")]
  filters: FiltersConfig,
  #[serde(skip_serializing_if = "is_default")]
  sources: SourcesConfig,
  #[serde(skip_serializing_if = "is_default")]
  pricing: PricingConfig,
  #[serde(skip_serializing_if = "is_default")]
  diagnostics: DiagnosticsConfig,

  #[serde(flatten, skip_serializing)]
  flat: FlatConfig,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
struct RawConfigFile {
  output: Option<OutputConfig>,
  period: Option<PeriodConfig>,
  table: Option<TableConfig>,
  cache: Option<CacheConfig>,
  grouping: Option<GroupingConfig>,
  filters: Option<FiltersConfig>,
  sources: Option<SourcesConfig>,
  pricing: Option<PricingConfig>,
  diagnostics: Option<DiagnosticsConfig>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, rename_all = "kebab-case")]
struct OutputConfig {
  #[serde(skip_serializing_if = "Option::is_none")]
  format: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  sort: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  asc: Option<bool>,
  #[serde(skip_serializing_if = "Option::is_none")]
  limit: Option<usize>,
  #[serde(skip_serializing_if = "Option::is_none")]
  no_cost: Option<bool>,
  #[serde(skip_serializing_if = "Option::is_none")]
  cost: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  cost_per: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, rename_all = "kebab-case")]
struct PeriodConfig {
  #[serde(skip_serializing_if = "Option::is_none")]
  period: Option<String>,
  #[serde(rename = "24h", skip_serializing_if = "Option::is_none")]
  period_24h: Option<bool>,
  #[serde(rename = "7d", skip_serializing_if = "Option::is_none")]
  period_7d: Option<bool>,
  #[serde(rename = "1m", skip_serializing_if = "Option::is_none")]
  period_1m: Option<bool>,
  #[serde(skip_serializing_if = "Option::is_none")]
  today: Option<bool>,
  #[serde(skip_serializing_if = "Option::is_none")]
  week: Option<bool>,
  #[serde(skip_serializing_if = "Option::is_none")]
  month: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, rename_all = "kebab-case")]
struct TableConfig {
  #[serde(skip_serializing_if = "Option::is_none")]
  no_color: Option<bool>,
  #[serde(skip_serializing_if = "Option::is_none")]
  human: Option<bool>,
  #[serde(skip_serializing_if = "Option::is_none")]
  no_fit: Option<bool>,
  #[serde(skip_serializing_if = "Option::is_none")]
  table_width: Option<usize>,
  #[serde(skip_serializing_if = "Option::is_none")]
  split_input: Option<bool>,
  #[serde(skip_serializing_if = "Option::is_none")]
  bytes: Option<bool>,
  #[serde(skip_serializing_if = "Option::is_none")]
  avg: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, rename_all = "kebab-case")]
struct CacheConfig {
  #[serde(skip_serializing_if = "Option::is_none")]
  no_cache: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, rename_all = "kebab-case")]
struct GroupingConfig {
  #[serde(skip_serializing_if = "Option::is_none")]
  group_by: Option<Vec<String>>,
  #[serde(skip_serializing_if = "Option::is_none")]
  date_bucket: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, rename_all = "kebab-case")]
struct FiltersConfig {
  #[serde(skip_serializing_if = "Option::is_none")]
  since: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  until: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  model: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  provider: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  cwd: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, rename_all = "kebab-case")]
struct SourcesConfig {
  #[serde(skip_serializing_if = "Option::is_none")]
  source: Option<Vec<String>>,
  #[serde(skip_serializing_if = "Option::is_none")]
  codex_dir: Option<PathBuf>,
  #[serde(skip_serializing_if = "Option::is_none")]
  opencode_db: Option<PathBuf>,
  #[serde(skip_serializing_if = "Option::is_none")]
  claude_dir: Option<PathBuf>,
  #[serde(skip_serializing_if = "Option::is_none")]
  copilot_dir: Option<Vec<PathBuf>>,
  #[serde(skip_serializing_if = "Option::is_none")]
  copilot_cli_dir: Option<Vec<PathBuf>>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, rename_all = "kebab-case")]
struct PricingConfig {
  #[serde(skip_serializing_if = "Option::is_none")]
  pricing: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, rename_all = "kebab-case")]
struct DiagnosticsConfig {
  #[serde(skip_serializing_if = "Option::is_none")]
  verbose: Option<bool>,
}

fn is_default<T>(value: &T) -> bool
where
  T: Default + PartialEq,
{
  value == &T::default()
}

impl ConfigFile {
  fn merge_flat(&mut self) {
    merge_section(&mut self.output, &self.flat.output);
    merge_section(&mut self.period, &self.flat.period);
    merge_section(&mut self.table, &self.flat.table);
    merge_section(&mut self.cache, &self.flat.cache);
    merge_section(&mut self.grouping, &self.flat.grouping);
    merge_section(&mut self.filters, &self.flat.filters);
    merge_section(&mut self.sources, &self.flat.sources);
    merge_section(&mut self.pricing, &self.flat.pricing);
    merge_section(&mut self.diagnostics, &self.flat.diagnostics);
  }
}

impl RawConfigFile {
  fn into_config(self, flat: FlatConfig) -> ConfigFile {
    let mut config = ConfigFile {
      output: self.output.unwrap_or_default(),
      period: self.period.unwrap_or_default(),
      table: self.table.unwrap_or_default(),
      cache: self.cache.unwrap_or_default(),
      grouping: self.grouping.unwrap_or_default(),
      filters: self.filters.unwrap_or_default(),
      sources: self.sources.unwrap_or_default(),
      pricing: self.pricing.unwrap_or_default(),
      diagnostics: self.diagnostics.unwrap_or_default(),
      flat,
    };
    config.merge_flat();
    config
  }
}

fn merge_section<T>(dst: &mut T, src: &T)
where
  T: Clone + Default + PartialEq,
{
  if *dst == T::default() {
    *dst = src.clone();
  }
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
  if !no_default && !raw_matches.get_flag("no_config") && raw_matches.subcommand_name() != Some("config") {
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
    .map(|base| base.join("llm-tokei.toml"))
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

pub fn list_config(path: &Path) -> Result<String> {
  if path.exists() {
    std::fs::read_to_string(path).with_context(|| format!("reading config file {}", path.display()))
  } else {
    Ok(String::new())
  }
}

fn read_config(path: &Path) -> Result<ConfigFile> {
  let s = std::fs::read_to_string(path).with_context(|| format!("reading config file {}", path.display()))?;
  let value: Value = toml::from_str(&s).with_context(|| format!("parsing config file {}", path.display()))?;
  let raw = raw_config_from_value(&value)?;
  Ok(raw.into_config(flat_config_from_value(&value)?))
}

fn raw_config_from_value(value: &Value) -> Result<RawConfigFile> {
  let Some(table) = value.as_table() else {
    return Ok(RawConfigFile::default());
  };
  Ok(RawConfigFile {
    output: section(table, "output")?,
    period: section(table, "period")?,
    table: section(table, "table")?,
    cache: section(table, "cache")?,
    grouping: section(table, "grouping")?,
    filters: section(table, "filters")?,
    sources: section(table, "sources")?,
    pricing: section(table, "pricing")?,
    diagnostics: section(table, "diagnostics")?,
  })
}

fn section<T>(table: &toml::map::Map<String, Value>, key: &str) -> Result<Option<T>>
where
  T: for<'de> Deserialize<'de>,
{
  table
    .get(key)
    .filter(|v| v.is_table())
    .cloned()
    .map(T::deserialize)
    .transpose()
    .map_err(Into::into)
}

#[derive(Debug, Clone, Default, PartialEq)]
struct FlatConfig {
  output: OutputConfig,
  period: PeriodConfig,
  table: TableConfig,
  cache: CacheConfig,
  grouping: GroupingConfig,
  filters: FiltersConfig,
  sources: SourcesConfig,
  pricing: PricingConfig,
  diagnostics: DiagnosticsConfig,
}

fn flat_config_from_value(value: &Value) -> Result<FlatConfig> {
  let Some(table) = value.as_table() else {
    return Ok(FlatConfig::default());
  };
  let mut flat = FlatConfig::default();
  for (key, value) in table {
    if value.is_table() {
      continue;
    }
    match key.as_str() {
      "format" => flat.output.format = value.as_str().map(str::to_string),
      "sort" => flat.output.sort = value.as_str().map(str::to_string),
      "asc" => flat.output.asc = value.as_bool(),
      "limit" => flat.output.limit = value.as_integer().and_then(|v| usize::try_from(v).ok()),
      "no-cost" => flat.output.no_cost = value.as_bool(),
      "cost" => flat.output.cost = value.as_str().map(str::to_string),
      "cost-per" => flat.output.cost_per = value.as_str().map(str::to_string),
      "period" => flat.period.period = value.as_str().map(str::to_string),
      "24h" => flat.period.period_24h = value.as_bool(),
      "7d" => flat.period.period_7d = value.as_bool(),
      "1m" => flat.period.period_1m = value.as_bool(),
      "today" => flat.period.today = value.as_bool(),
      "week" => flat.period.week = value.as_bool(),
      "month" => flat.period.month = value.as_bool(),
      "no-color" => flat.table.no_color = value.as_bool(),
      "human" => flat.table.human = value.as_bool(),
      "no-fit" => flat.table.no_fit = value.as_bool(),
      "table-width" => flat.table.table_width = value.as_integer().and_then(|v| usize::try_from(v).ok()),
      "split-input" => flat.table.split_input = value.as_bool(),
      "bytes" => flat.table.bytes = value.as_bool(),
      "avg" => flat.table.avg = value.as_str().map(str::to_string),
      "no-cache" => flat.cache.no_cache = value.as_bool(),
      "group-by" => flat.grouping.group_by = string_array(value),
      "date-bucket" => flat.grouping.date_bucket = value.as_str().map(str::to_string),
      "since" => flat.filters.since = value.as_str().map(str::to_string),
      "until" => flat.filters.until = value.as_str().map(str::to_string),
      "model" => flat.filters.model = value.as_str().map(str::to_string),
      "provider" => flat.filters.provider = value.as_str().map(str::to_string),
      "cwd" => flat.filters.cwd = value.as_str().map(str::to_string),
      "source" => flat.sources.source = string_array(value),
      "codex-dir" => flat.sources.codex_dir = value.as_str().map(PathBuf::from),
      "opencode-db" => flat.sources.opencode_db = value.as_str().map(PathBuf::from),
      "claude-dir" => flat.sources.claude_dir = value.as_str().map(PathBuf::from),
      "copilot-dir" => flat.sources.copilot_dir = path_array(value),
      "copilot-cli-dir" => flat.sources.copilot_cli_dir = path_array(value),
      "pricing" => flat.pricing.pricing = value.as_str().map(PathBuf::from),
      "verbose" => flat.diagnostics.verbose = value.as_bool(),
      _ => {}
    }
  }
  Ok(flat)
}

fn string_array(value: &Value) -> Option<Vec<String>> {
  value
    .as_array()
    .map(|items| items.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
}

fn path_array(value: &Value) -> Option<Vec<PathBuf>> {
  string_array(value).map(|items| items.into_iter().map(PathBuf::from).collect())
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
    out.output.format = value_name::<Format>(matches, "format");
  }
  if cli_set(matches, "sort") {
    out.output.sort = matches.get_one::<String>("sort").cloned();
  }
  if cli_set(matches, "asc") {
    out.output.asc = Some(matches.get_flag("asc"));
  }
  if cli_set(matches, "limit") {
    out.output.limit = matches.get_one::<usize>("limit").copied();
  }
  if cli_set(matches, "no_cost") {
    out.output.no_cost = Some(matches.get_flag("no_cost"));
  }
  if cli_set(matches, "cost") {
    out.output.cost = value_name::<CostMode>(matches, "cost");
  }
  if cli_set(matches, "cost_per") {
    out.output.cost_per = matches.get_one::<String>("cost_per").cloned();
  }
  if cli_set(matches, "period") {
    out.period.period = value_name::<Period>(matches, "period");
  }
  if cli_set(matches, "period_24h") {
    out.period.period = Some("24h".to_string());
  }
  if cli_set(matches, "period_7d") {
    out.period.period = Some("7d".to_string());
  }
  if cli_set(matches, "period_1m") {
    out.period.period = Some("1m".to_string());
  }
  if cli_set(matches, "today") {
    out.period.period = Some("today".to_string());
  }
  if cli_set(matches, "week") {
    out.period.period = Some("week".to_string());
  }
  if cli_set(matches, "month") {
    out.period.period = Some("month".to_string());
  }
  out.table.no_color = flag_if_set(matches, "no_color");
  out.table.human = flag_if_set(matches, "human");
  out.table.no_fit = flag_if_set(matches, "no_fit");
  if cli_set(matches, "table_width") {
    out.table.table_width = matches.get_one::<usize>("table_width").copied();
  }
  out.table.split_input = flag_if_set(matches, "split_input");
  out.table.bytes = flag_if_set(matches, "bytes");
  if cli_set(matches, "avg") {
    out.table.avg = value_name::<AvgBy>(matches, "avg");
  }
  out.cache.no_cache = flag_if_set(matches, "no_cache");
  if cli_set(matches, "group_by") {
    out.grouping.group_by = matches.get_many::<String>("group_by").map(|v| v.cloned().collect());
  }
  if cli_set(matches, "date_bucket") {
    out.grouping.date_bucket = value_name::<DateBucket>(matches, "date_bucket");
  }
  if cli_set(matches, "since") {
    out.filters.since = matches.get_one::<String>("since").cloned();
  }
  if cli_set(matches, "until") {
    out.filters.until = matches.get_one::<String>("until").cloned();
  }
  if cli_set(matches, "model") {
    out.filters.model = matches.get_one::<String>("model").cloned();
  }
  if cli_set(matches, "provider") {
    out.filters.provider = matches.get_one::<String>("provider").cloned();
  }
  if cli_set(matches, "cwd") {
    out.filters.cwd = matches.get_one::<String>("cwd").cloned();
  }
  if cli_set(matches, "source") {
    out.sources.source = matches.get_many::<String>("source").map(|v| v.cloned().collect());
  }
  out.sources.codex_dir = path_if_set(matches, "codex_dir");
  out.sources.opencode_db = path_if_set(matches, "opencode_db");
  out.sources.claude_dir = path_if_set(matches, "claude_dir");
  out.sources.copilot_dir = paths_if_set(matches, "copilot_dir");
  out.sources.copilot_cli_dir = paths_if_set(matches, "copilot_cli_dir");
  out.pricing.pricing = path_if_set(matches, "pricing");
  out.diagnostics.verbose = flag_if_set(matches, "verbose");
  Ok(out)
}

fn config_to_args(config: &ConfigFile, current: &clap::ArgMatches) -> Vec<String> {
  let mut out = Vec::new();
  push_opt(&mut out, current, "format", "--format", config.output.format.as_deref());
  push_opt(&mut out, current, "sort", "--sort", config.output.sort.as_deref());
  push_bool(&mut out, current, "asc", "--asc", config.output.asc);
  push_opt_display(&mut out, current, "limit", "--limit", config.output.limit);
  push_bool(&mut out, current, "no_cost", "--no-cost", config.output.no_cost);
  push_opt(&mut out, current, "cost", "--cost", config.output.cost.as_deref());
  push_opt(
    &mut out,
    current,
    "cost_per",
    "--cost-per",
    config.output.cost_per.as_deref(),
  );
  if !cli_set(current, "period_24h")
    && !cli_set(current, "period_7d")
    && !cli_set(current, "period_1m")
    && !cli_set(current, "today")
    && !cli_set(current, "week")
    && !cli_set(current, "month")
  {
    push_opt(&mut out, current, "period", "--period", config.period.period.as_deref());
  }
  push_bool(&mut out, current, "period_24h", "--24h", config.period.period_24h);
  push_bool(&mut out, current, "period_7d", "--7d", config.period.period_7d);
  push_bool(&mut out, current, "period_1m", "--1m", config.period.period_1m);
  push_bool(&mut out, current, "today", "--today", config.period.today);
  push_bool(&mut out, current, "week", "--week", config.period.week);
  push_bool(&mut out, current, "month", "--month", config.period.month);
  push_bool(&mut out, current, "no_color", "--no-color", config.table.no_color);
  push_bool(&mut out, current, "human", "--human", config.table.human);
  push_bool(&mut out, current, "no_fit", "--no-fit", config.table.no_fit);
  push_opt_display(
    &mut out,
    current,
    "table_width",
    "--table-width",
    config.table.table_width,
  );
  push_bool(
    &mut out,
    current,
    "split_input",
    "--split-input",
    config.table.split_input,
  );
  push_bool(&mut out, current, "bytes", "--bytes", config.table.bytes);
  push_opt(&mut out, current, "avg", "--avg", config.table.avg.as_deref());
  push_bool(&mut out, current, "no_cache", "--no-cache", config.cache.no_cache);
  push_list(
    &mut out,
    current,
    "group_by",
    "--group-by",
    config.grouping.group_by.as_deref(),
  );
  push_opt(
    &mut out,
    current,
    "date_bucket",
    "--date-bucket",
    config.grouping.date_bucket.as_deref(),
  );
  push_opt(&mut out, current, "since", "--since", config.filters.since.as_deref());
  push_opt(&mut out, current, "until", "--until", config.filters.until.as_deref());
  push_opt(&mut out, current, "model", "--model", config.filters.model.as_deref());
  push_opt(
    &mut out,
    current,
    "provider",
    "--provider",
    config.filters.provider.as_deref(),
  );
  push_opt(&mut out, current, "cwd", "--cwd", config.filters.cwd.as_deref());
  push_list(
    &mut out,
    current,
    "source",
    "--source",
    config.sources.source.as_deref(),
  );
  push_path(
    &mut out,
    current,
    "codex_dir",
    "--codex-dir",
    config.sources.codex_dir.as_deref(),
  );
  push_path(
    &mut out,
    current,
    "opencode_db",
    "--opencode-db",
    config.sources.opencode_db.as_deref(),
  );
  push_path(
    &mut out,
    current,
    "claude_dir",
    "--claude-dir",
    config.sources.claude_dir.as_deref(),
  );
  push_paths(
    &mut out,
    current,
    "copilot_dir",
    "--copilot-dir",
    config.sources.copilot_dir.as_deref(),
  );
  push_paths(
    &mut out,
    current,
    "copilot_cli_dir",
    "--copilot-cli-dir",
    config.sources.copilot_cli_dir.as_deref(),
  );
  push_path(
    &mut out,
    current,
    "pricing",
    "--pricing",
    config.pricing.pricing.as_deref(),
  );
  push_bool(&mut out, current, "verbose", "--verbose", config.diagnostics.verbose);
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
