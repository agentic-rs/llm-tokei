use chrono::{DateTime, Utc};

use crate::cli::{Args, AvgBy, Cmd, DateBucket, Format, GraphChart, SvgTheme, Unit};
use crate::pricing::CostMode;

struct Tip {
  text: &'static str,
  covered: TipCoverage,
}

#[derive(Clone, Copy)]
enum TipCoverage {
  Period(&'static str),
  GraphPeriod(&'static str),
  GroupBy(&'static [&'static str]),
  GroupByBucket {
    dimensions: &'static [&'static str],
    bucket: DateBucket,
  },
  Source(&'static [&'static str]),
  Cwd,
  Provider(&'static str),
  Model(&'static str),
  Since(&'static str),
  Until(&'static str),
  SourceModel {
    source: &'static str,
    model: &'static str,
  },
  ProviderModel {
    provider: &'static str,
    model: &'static str,
  },
  SinceProvider {
    since: &'static str,
    provider: &'static str,
  },
  Sort(&'static str),
  SortWithLimit(&'static str),
  SortAsc(&'static str),
  Cost(CostMode),
  CostPer(&'static str),
  Unit(Unit),
  Format(Format),
  Theme(SvgTheme),
  Avg(AvgBy),
  Flag(TipFlag),
  Command(TipCommand),
  GraphChartPeriod {
    chart: GraphChart,
    period: &'static str,
  },
  GraphWidth,
  GraphUnit(Unit),
  GraphUnitPeriod {
    unit: Unit,
    period: &'static str,
  },
  GraphSince(&'static str),
  GraphFormat(Format),
  GraphFormatTheme {
    format: Format,
    theme: SvgTheme,
  },
}

#[derive(Clone, Copy)]
enum TipFlag {
  Human,
  SplitInput,
  NoCache,
  Verbose,
  Asc,
  Limit,
  TableWidth,
  NoColor,
  NoCost,
  Bytes,
  NoFit,
  SaveDefault,
  NoDefault,
  NoConfig,
  NoTips,
}

#[derive(Clone, Copy)]
enum TipCommand {
  Update,
}

const GROUP_PROJECT_SOURCE_MODEL: &[&str] = &["project", "source", "model"];
const GROUP_SESSION_SOURCE_MODEL: &[&str] = &["session", "source", "model"];
const GROUP_PROVIDER_MODEL: &[&str] = &["provider", "model"];
const GROUP_DATE_SOURCE: &[&str] = &["date", "source"];
const GROUP_DATE_PROJECT: &[&str] = &["date", "project"];
const GROUP_SOURCE_PROVIDER: &[&str] = &["source", "provider"];
const GROUP_MODEL_PROJECT: &[&str] = &["model", "project"];
const GROUP_SESSION_PROJECT: &[&str] = &["session", "project"];
const GROUP_DATE_MODEL: &[&str] = &["date", "model"];
const GROUP_PROVIDER_PROJECT: &[&str] = &["provider", "project"];
const GROUP_SOURCE: &[&str] = &["source"];
const GROUP_MODEL: &[&str] = &["model"];
const GROUP_DATE: &[&str] = &["date"];
const GROUP_SESSION: &[&str] = &["session"];

const SOURCE_CODEX_CLAUDE: &[&str] = &["codex", "claude"];
const SOURCE_OPENCODE_PI: &[&str] = &["opencode", "pi-agent"];
const SOURCE_COPILOT: &[&str] = &["copilot", "copilot-cli"];

const TIPS: &[Tip] = &[
  // Time windows.
  Tip {
    text: "Use `--24h` for rolling last-day usage.",
    covered: TipCoverage::Period("24h"),
  },
  Tip {
    text: "Use `--7d` to focus on the last week.",
    covered: TipCoverage::Period("7d"),
  },
  Tip {
    text: "Use `--1m` for a rolling 30-day view.",
    covered: TipCoverage::Period("1m"),
  },
  Tip {
    text: "Use `--today` for usage since local midnight.",
    covered: TipCoverage::Period("today"),
  },
  Tip {
    text: "Use `--week` for usage since local week start.",
    covered: TipCoverage::Period("week"),
  },
  Tip {
    text: "Use `--month` for usage since local month start.",
    covered: TipCoverage::Period("month"),
  },
  Tip {
    text: "Use `--period 3d` for a short recent window.",
    covered: TipCoverage::Period("3d"),
  },
  Tip {
    text: "Use `--period 12h` for a half-day window.",
    covered: TipCoverage::Period("12h"),
  },
  Tip {
    text: "Use `--period 2w` for a two-week view.",
    covered: TipCoverage::Period("2w"),
  },
  Tip {
    text: "Use `--period 6h` for a focused work session.",
    covered: TipCoverage::Period("6h"),
  },
  Tip {
    text: "Use `--period 1mo` for a rolling month.",
    covered: TipCoverage::Period("1mo"),
  },
  Tip {
    text: "Use `--since 12h` to inspect recent activity.",
    covered: TipCoverage::Since("12h"),
  },
  Tip {
    text: "Use `--since 7d` to compare recent activity.",
    covered: TipCoverage::Since("7d"),
  },
  Tip {
    text: "Use `--since 2w` to inspect the last fortnight.",
    covered: TipCoverage::Since("2w"),
  },
  // Filters and source selection.
  Tip {
    text: "Filter models with `--model 'claude-*'`.",
    covered: TipCoverage::Model("claude-*"),
  },
  Tip {
    text: "Filter GPT models with `--model 'gpt-*'`.",
    covered: TipCoverage::Model("gpt-*"),
  },
  Tip {
    text: "Filter one provider with `--provider openai`.",
    covered: TipCoverage::Provider("openai"),
  },
  Tip {
    text: "Try `--provider anthropic` for Claude usage.",
    covered: TipCoverage::Provider("anthropic"),
  },
  Tip {
    text: "Try `--provider google` for Gemini usage.",
    covered: TipCoverage::Provider("google"),
  },
  Tip {
    text: "Use `--provider xai` to inspect Grok usage.",
    covered: TipCoverage::Provider("xai"),
  },
  Tip {
    text: "Focus on a workspace with `--cwd '*/project-*'`.",
    covered: TipCoverage::Cwd,
  },
  Tip {
    text: "Narrow a report with `--cwd '*/work/*'`.",
    covered: TipCoverage::Cwd,
  },
  Tip {
    text: "Limit discovery with `--source codex,claude`.",
    covered: TipCoverage::Source(SOURCE_CODEX_CLAUDE),
  },
  Tip {
    text: "Compare `--source opencode,pi-agent` sessions.",
    covered: TipCoverage::Source(SOURCE_OPENCODE_PI),
  },
  Tip {
    text: "Inspect Copilot sources with `--source copilot,copilot-cli`.",
    covered: TipCoverage::Source(SOURCE_COPILOT),
  },
  Tip {
    text: "Combine `--source codex` with `--model 'gpt-*'`.",
    covered: TipCoverage::SourceModel {
      source: "codex",
      model: "gpt-*",
    },
  },
  Tip {
    text: "Combine `--provider openai` with `--model 'gpt-*'`.",
    covered: TipCoverage::ProviderModel {
      provider: "openai",
      model: "gpt-*",
    },
  },
  Tip {
    text: "Use `--since 12h --provider openai` for recent OpenAI usage.",
    covered: TipCoverage::SinceProvider {
      since: "12h",
      provider: "openai",
    },
  },
  Tip {
    text: "Use `--until 1h` to exclude the current hour.",
    covered: TipCoverage::Until("1h"),
  },
  // Grouping.
  Tip {
    text: "Compare projects with `--group-by project,source,model`.",
    covered: TipCoverage::GroupBy(GROUP_PROJECT_SOURCE_MODEL),
  },
  Tip {
    text: "Inspect sessions with `--group-by session,source,model`.",
    covered: TipCoverage::GroupBy(GROUP_SESSION_SOURCE_MODEL),
  },
  Tip {
    text: "Compare providers with `--group-by provider,model`.",
    covered: TipCoverage::GroupBy(GROUP_PROVIDER_MODEL),
  },
  Tip {
    text: "See weekly trends with `--group-by date,source --date-bucket week`.",
    covered: TipCoverage::GroupByBucket {
      dimensions: GROUP_DATE_SOURCE,
      bucket: DateBucket::Week,
    },
  },
  Tip {
    text: "See monthly projects with `--group-by date,project --date-bucket month`.",
    covered: TipCoverage::GroupByBucket {
      dimensions: GROUP_DATE_PROJECT,
      bucket: DateBucket::Month,
    },
  },
  Tip {
    text: "Use `--group-by date,source --date-bucket day` for daily trends.",
    covered: TipCoverage::GroupByBucket {
      dimensions: GROUP_DATE_SOURCE,
      bucket: DateBucket::Day,
    },
  },
  Tip {
    text: "Compare agents and providers with `--group-by source,provider`.",
    covered: TipCoverage::GroupBy(GROUP_SOURCE_PROVIDER),
  },
  Tip {
    text: "Use `--group-by model,project` to compare model usage.",
    covered: TipCoverage::GroupBy(GROUP_MODEL_PROJECT),
  },
  Tip {
    text: "Inspect sessions by project with `--group-by session,project`.",
    covered: TipCoverage::GroupBy(GROUP_SESSION_PROJECT),
  },
  Tip {
    text: "Track models over time with `--group-by date,model`.",
    covered: TipCoverage::GroupBy(GROUP_DATE_MODEL),
  },
  Tip {
    text: "Compare project providers with `--group-by provider,project`.",
    covered: TipCoverage::GroupBy(GROUP_PROVIDER_PROJECT),
  },
  Tip {
    text: "Use `--group-by source` for a compact source summary.",
    covered: TipCoverage::GroupBy(GROUP_SOURCE),
  },
  Tip {
    text: "Use `--group-by model` for a compact model summary.",
    covered: TipCoverage::GroupBy(GROUP_MODEL),
  },
  Tip {
    text: "Use `--group-by date` for a simple timeline.",
    covered: TipCoverage::GroupBy(GROUP_DATE),
  },
  Tip {
    text: "Use `--group-by session` to find individual runs.",
    covered: TipCoverage::GroupBy(GROUP_SESSION),
  },
  // Sorting and cost.
  Tip {
    text: "Find expensive groups with `--sort cost --limit 10`.",
    covered: TipCoverage::SortWithLimit("cost"),
  },
  Tip {
    text: "Sort by `--sort total` to rank overall usage.",
    covered: TipCoverage::Sort("total"),
  },
  Tip {
    text: "Use `--sort input` to find prompt-heavy groups.",
    covered: TipCoverage::Sort("input"),
  },
  Tip {
    text: "Use `--sort output` to find response-heavy groups.",
    covered: TipCoverage::Sort("output"),
  },
  Tip {
    text: "Use `--sort calls` to find the busiest groups.",
    covered: TipCoverage::Sort("calls"),
  },
  Tip {
    text: "Use `--sort date` to order groups chronologically.",
    covered: TipCoverage::Sort("date"),
  },
  Tip {
    text: "Use `--sort cost --asc` to find the least expensive groups.",
    covered: TipCoverage::SortAsc("cost"),
  },
  Tip {
    text: "Use `--limit 10` to keep a report focused.",
    covered: TipCoverage::Flag(TipFlag::Limit),
  },
  Tip {
    text: "Use `--limit 20` for a broader top list.",
    covered: TipCoverage::Flag(TipFlag::Limit),
  },
  Tip {
    text: "Use `--asc` to reverse the current sort order.",
    covered: TipCoverage::Flag(TipFlag::Asc),
  },
  Tip {
    text: "Use `--cost official` to compare official model rates.",
    covered: TipCoverage::Cost(CostMode::Official),
  },
  Tip {
    text: "Use `--cost actual` to use provider-reported costs.",
    covered: TipCoverage::Cost(CostMode::Actual),
  },
  Tip {
    text: "Use `--cost mixed` to fill missing provider rates.",
    covered: TipCoverage::Cost(CostMode::Mixed),
  },
  Tip {
    text: "Use `--cost-per provider` to split top costs by provider.",
    covered: TipCoverage::CostPer("provider"),
  },
  Tip {
    text: "Use `--cost-per model` to split top costs by model.",
    covered: TipCoverage::CostPer("model"),
  },
  Tip {
    text: "Use `--cost-per project` to split top costs by project.",
    covered: TipCoverage::CostPer("project"),
  },
  Tip {
    text: "Use `--unit cost` to show spend instead of tokens.",
    covered: TipCoverage::Unit(Unit::Cost),
  },
  Tip {
    text: "Use `--unit tokens` for token-based display.",
    covered: TipCoverage::Unit(Unit::Tokens),
  },
  Tip {
    text: "Use `--bytes` to show input and output bytes.",
    covered: TipCoverage::Flag(TipFlag::Bytes),
  },
  Tip {
    text: "Use `--no-cost` to hide cost columns.",
    covered: TipCoverage::Flag(TipFlag::NoCost),
  },
  // Table, output, and configuration.
  Tip {
    text: "Use `--human` (or `-h`) for compact table values.",
    covered: TipCoverage::Flag(TipFlag::Human),
  },
  Tip {
    text: "Use `--split-input` to show uncached input as `input_u`.",
    covered: TipCoverage::Flag(TipFlag::SplitInput),
  },
  Tip {
    text: "Use `--table-width 100` to fit a fixed-width report.",
    covered: TipCoverage::Flag(TipFlag::TableWidth),
  },
  Tip {
    text: "Use `--no-fit` to disable automatic table fitting.",
    covered: TipCoverage::Flag(TipFlag::NoFit),
  },
  Tip {
    text: "Use `--no-color` to keep output plain.",
    covered: TipCoverage::Flag(TipFlag::NoColor),
  },
  Tip {
    text: "Use `--avg call` to see average usage per call.",
    covered: TipCoverage::Avg(AvgBy::Call),
  },
  Tip {
    text: "Use `--avg round` to see average usage per round.",
    covered: TipCoverage::Avg(AvgBy::Round),
  },
  Tip {
    text: "Use `--avg session` to see average usage per session.",
    covered: TipCoverage::Avg(AvgBy::Session),
  },
  Tip {
    text: "Use `--format json` for scripts and dashboards.",
    covered: TipCoverage::Format(Format::Json),
  },
  Tip {
    text: "Save a shareable table with `--format svg > usage.svg`.",
    covered: TipCoverage::Format(Format::Svg),
  },
  Tip {
    text: "Run `llm-tokei update` to refresh price history and model mappings.",
    covered: TipCoverage::Command(TipCommand::Update),
  },
  Tip {
    text: "Run `--no-cache` to re-parse every source file.",
    covered: TipCoverage::Flag(TipFlag::NoCache),
  },
  Tip {
    text: "Add `--verbose` to inspect parsing warnings.",
    covered: TipCoverage::Flag(TipFlag::Verbose),
  },
  Tip {
    text: "Use `--save-default` to remember your usual flags.",
    covered: TipCoverage::Flag(TipFlag::SaveDefault),
  },
  Tip {
    text: "Use `--no-default` to ignore saved defaults once.",
    covered: TipCoverage::Flag(TipFlag::NoDefault),
  },
  Tip {
    text: "Use `--no-config` to ignore the config file.",
    covered: TipCoverage::Flag(TipFlag::NoConfig),
  },
  Tip {
    text: "Use `--svg-theme light` for light-friendly SVGs.",
    covered: TipCoverage::Theme(SvgTheme::Light),
  },
  Tip {
    text: "Use `--svg-theme dark` for dark-friendly SVGs.",
    covered: TipCoverage::Theme(SvgTheme::Dark),
  },
  Tip {
    text: "Use `--no-tips` to hide the footer.",
    covered: TipCoverage::Flag(TipFlag::NoTips),
  },
  // Graphs.
  Tip {
    text: "Run `llm-tokei graph --24h` for an hourly plot.",
    covered: TipCoverage::GraphPeriod("24h"),
  },
  Tip {
    text: "Run `llm-tokei graph --7d` for a weekly activity view.",
    covered: TipCoverage::GraphPeriod("7d"),
  },
  Tip {
    text: "Run `llm-tokei graph --month` for a monthly view.",
    covered: TipCoverage::GraphPeriod("month"),
  },
  Tip {
    text: "Use `llm-tokei graph --chart plot --month` for a plot.",
    covered: TipCoverage::GraphChartPeriod {
      chart: GraphChart::Plot,
      period: "month",
    },
  },
  Tip {
    text: "Use `llm-tokei graph --chart heatmap --month` for a monthly heatmap.",
    covered: TipCoverage::GraphChartPeriod {
      chart: GraphChart::Heatmap,
      period: "month",
    },
  },
  Tip {
    text: "Run `llm-tokei graph --width 100` to control spacing.",
    covered: TipCoverage::GraphWidth,
  },
  Tip {
    text: "Graph bytes with `llm-tokei graph --unit bytes --month`.",
    covered: TipCoverage::GraphUnitPeriod {
      unit: Unit::Bytes,
      period: "month",
    },
  },
  Tip {
    text: "Graph spend with `llm-tokei graph --unit cost`.",
    covered: TipCoverage::GraphUnit(Unit::Cost),
  },
  Tip {
    text: "Graph recent activity with `llm-tokei graph --since 7d`.",
    covered: TipCoverage::GraphSince("7d"),
  },
  Tip {
    text: "Use `llm-tokei graph --since 12h` for hourly detail.",
    covered: TipCoverage::GraphSince("12h"),
  },
  Tip {
    text: "Use `llm-tokei graph --chart heatmap --24h` for daily buckets.",
    covered: TipCoverage::GraphChartPeriod {
      chart: GraphChart::Heatmap,
      period: "24h",
    },
  },
  Tip {
    text: "Use `llm-tokei graph --chart plot --7d` for a bar plot.",
    covered: TipCoverage::GraphChartPeriod {
      chart: GraphChart::Plot,
      period: "7d",
    },
  },
  Tip {
    text: "Use `llm-tokei graph --format svg > activity.svg` for sharing.",
    covered: TipCoverage::GraphFormat(Format::Svg),
  },
  Tip {
    text: "Use `llm-tokei graph --format svg --svg-theme light > activity.svg`.",
    covered: TipCoverage::GraphFormatTheme {
      format: Format::Svg,
      theme: SvgTheme::Light,
    },
  },
  Tip {
    text: "Use `llm-tokei graph --unit bytes --7d` for byte activity.",
    covered: TipCoverage::GraphUnitPeriod {
      unit: Unit::Bytes,
      period: "7d",
    },
  },
  Tip {
    text: "Use `llm-tokei graph --unit cost --7d` for cost activity.",
    covered: TipCoverage::GraphUnitPeriod {
      unit: Unit::Cost,
      period: "7d",
    },
  },
  Tip {
    text: "Use `llm-tokei graph --no-color` for plain terminal charts.",
    covered: TipCoverage::Flag(TipFlag::NoColor),
  },
];

pub(crate) fn tip_for_hour(args: &Args, now: DateTime<Utc>) -> Option<&'static str> {
  if args.no_tips {
    return None;
  }

  let hour = now.timestamp().div_euclid(60 * 60);
  let start = hour.rem_euclid(TIPS.len() as i64) as usize;

  for offset in 0..TIPS.len() {
    let tip = &TIPS[(start + offset) % TIPS.len()];
    if !tip.covered.matches(args) {
      return Some(tip.text);
    }
  }

  None
}

impl TipCoverage {
  fn matches(self, args: &Args) -> bool {
    match self {
      Self::Period(period) => period_is(args, period),
      Self::GraphPeriod(period) => is_graph(args) && period_is(args, period),
      Self::GroupBy(dimensions) => group_by_contains(args, dimensions),
      Self::GroupByBucket { dimensions, bucket } => group_by_contains(args, dimensions) && args.date_bucket == bucket,
      Self::Source(sources) => source_contains(args, sources),
      Self::Cwd => args.cwd.is_some(),
      Self::Provider(provider) => value_is(args.provider.as_deref(), provider),
      Self::Model(model) => value_is(args.model.as_deref(), model),
      Self::Since(since) => value_is(args.since.as_deref(), since),
      Self::Until(until) => value_is(args.until.as_deref(), until),
      Self::SourceModel { source, model } => source_contains(args, &[source]) && value_is(args.model.as_deref(), model),
      Self::ProviderModel { provider, model } => {
        value_is(args.provider.as_deref(), provider) && value_is(args.model.as_deref(), model)
      }
      Self::SinceProvider { since, provider } => {
        value_is(args.since.as_deref(), since) && value_is(args.provider.as_deref(), provider)
      }
      Self::Sort(sort) => args.sort.eq_ignore_ascii_case(sort),
      Self::SortWithLimit(sort) => args.sort.eq_ignore_ascii_case(sort) && args.limit.is_some(),
      Self::SortAsc(sort) => args.sort.eq_ignore_ascii_case(sort) && args.asc,
      Self::Cost(cost) => args.cost == cost,
      Self::CostPer(dimension) => args
        .cost_per
        .as_deref()
        .is_some_and(|current| dimension_is(current, dimension)),
      Self::Unit(unit) => unit_is(args, unit),
      Self::Format(format) => args.format == format,
      Self::Theme(theme) => args.svg_theme == theme,
      Self::Avg(avg) => args.avg == Some(avg),
      Self::Flag(flag) => flag.matches(args),
      Self::Command(TipCommand::Update) => matches!(args.cmd.as_ref(), Some(Cmd::Update { .. })),
      Self::GraphChartPeriod { chart, period } => graph_chart_is(args, chart) && period_is(args, period),
      Self::GraphWidth => graph_width_is(args),
      Self::GraphUnit(unit) => is_graph(args) && unit_is(args, unit),
      Self::GraphUnitPeriod { unit, period } => is_graph(args) && unit_is(args, unit) && period_is(args, period),
      Self::GraphSince(since) => is_graph(args) && value_is(args.since.as_deref(), since),
      Self::GraphFormat(format) => is_graph(args) && args.format == format,
      Self::GraphFormatTheme { format, theme } => is_graph(args) && args.format == format && args.svg_theme == theme,
    }
  }
}

impl TipFlag {
  fn matches(self, args: &Args) -> bool {
    match self {
      Self::Human => args.human,
      Self::SplitInput => args.split_input,
      Self::NoCache => args.no_cache,
      Self::Verbose => args.verbose,
      Self::Asc => args.asc,
      Self::Limit => args.limit.is_some(),
      Self::TableWidth => args.table_width.is_some(),
      Self::NoColor => args.no_color,
      Self::NoCost => args.no_cost,
      Self::Bytes => args.bytes,
      Self::NoFit => args.no_fit,
      Self::SaveDefault => args.save_default,
      Self::NoDefault => args.no_default,
      Self::NoConfig => args.no_config,
      Self::NoTips => args.no_tips,
    }
  }
}

fn effective_period(args: &Args) -> Option<&str> {
  if let Some(period) = args.period.as_deref() {
    return Some(period);
  }
  if args.period_24h {
    Some("24h")
  } else if args.period_7d {
    Some("7d")
  } else if args.period_1m {
    Some("1m")
  } else if args.today {
    Some("today")
  } else if args.week {
    Some("week")
  } else if args.month {
    Some("month")
  } else {
    None
  }
}

fn period_is(args: &Args, expected: &str) -> bool {
  args.since.is_none() && effective_period(args).is_some_and(|period| period.eq_ignore_ascii_case(expected))
}

fn group_by_contains(args: &Args, dimensions: &[&str]) -> bool {
  dimensions
    .iter()
    .all(|dimension| args.group_by.iter().any(|group| dimension_is(group, dimension)))
}

fn source_contains(args: &Args, sources: &[&str]) -> bool {
  args.source.as_ref().is_some_and(|selected| {
    sources
      .iter()
      .all(|source| selected.iter().any(|current| current.eq_ignore_ascii_case(source)))
  })
}

fn value_is(value: Option<&str>, expected: &str) -> bool {
  value.is_some_and(|value| value.eq_ignore_ascii_case(expected))
}

fn dimension_is(value: &str, expected: &str) -> bool {
  value.eq_ignore_ascii_case(expected)
    || matches!(
      (value, expected),
      ("tool", "source") | ("cwd", "project") | ("day", "date")
    )
}

fn unit_is(args: &Args, expected: Unit) -> bool {
  if expected == Unit::Bytes {
    args.bytes || args.unit == Some(Unit::Bytes)
  } else {
    args.unit == Some(expected)
  }
}

fn is_graph(args: &Args) -> bool {
  matches!(args.cmd.as_ref(), Some(Cmd::Graph { .. }))
}

fn graph_chart_is(args: &Args, expected: GraphChart) -> bool {
  matches!(args.cmd.as_ref(), Some(Cmd::Graph { chart, .. }) if *chart == expected)
}

fn graph_width_is(args: &Args) -> bool {
  matches!(args.cmd.as_ref(), Some(Cmd::Graph { width: Some(_), .. }))
}

#[cfg(test)]
mod tests {
  use super::*;
  use chrono::{Duration, TimeZone};
  use clap::Parser;

  fn parse_args(arguments: &[&str]) -> Args {
    let mut argv = vec!["llm-tokei"];
    argv.extend_from_slice(arguments);
    Args::try_parse_from(argv).expect("valid test arguments")
  }

  #[test]
  fn catalog_has_a_hundred_tips() {
    assert_eq!(TIPS.len(), 100);
  }

  #[test]
  fn tip_is_stable_within_an_hour_and_rotates_afterward() {
    let args = parse_args(&[]);
    let start = Utc
      .with_ymd_and_hms(2026, 8, 3, 12, 10, 0)
      .single()
      .expect("valid timestamp");

    assert_eq!(
      tip_for_hour(&args, start),
      tip_for_hour(&args, start + Duration::minutes(49))
    );
    assert_ne!(
      tip_for_hour(&args, start),
      tip_for_hour(&args, start + Duration::hours(1))
    );
    assert_eq!(
      tip_for_hour(&args, start),
      tip_for_hour(&args, start + Duration::hours(TIPS.len() as i64))
    );
  }

  #[test]
  fn covered_tips_are_skipped() {
    let args = parse_args(&["--7d"]);
    let start = Utc
      .with_ymd_and_hms(2026, 8, 3, 12, 10, 0)
      .single()
      .expect("valid timestamp");

    for hour in 0..TIPS.len() {
      assert_ne!(
        tip_for_hour(&args, start + Duration::hours(hour as i64)),
        Some("Use `--7d` to focus on the last week.")
      );
    }
  }

  #[test]
  fn period_tips_respect_since_precedence() {
    let args = parse_args(&["--7d", "--since", "12h"]);

    assert!(!TipCoverage::Period("7d").matches(&args));
    assert!(TipCoverage::Since("12h").matches(&args));
  }

  #[test]
  fn grouping_aliases_count_as_covered() {
    let args = parse_args(&["--group-by", "tool,cwd,day"]);

    assert!(TipCoverage::GroupBy(GROUP_SOURCE).matches(&args));
    assert!(TipCoverage::GroupBy(GROUP_DATE_PROJECT).matches(&args));
    assert!(TipCoverage::CostPer("project").matches(&parse_args(&["--cost-per", "cwd"])));
  }

  #[test]
  fn no_tips_disables_the_footer() {
    let args = parse_args(&["--no-tips"]);
    let now = Utc
      .with_ymd_and_hms(2026, 8, 3, 12, 10, 0)
      .single()
      .expect("valid timestamp");

    assert_eq!(tip_for_hour(&args, now), None);
  }

  #[test]
  fn tips_mark_command_boundaries_with_backticks() {
    assert!(TIPS.iter().all(|tip| tip.text.contains('`')));
  }
}
