use chrono::{DateTime, Utc};

use crate::cli::{Args, Cmd, DateBucket, Format, GraphChart, Unit};
use crate::pricing::CostMode;

struct Tip {
  text: &'static str,
  covered: fn(&Args) -> bool,
}

const TIPS: &[Tip] = &[
  Tip {
    text: "Use `--7d` to focus on the last week.",
    covered: period_7d,
  },
  Tip {
    text: "Try `--group-by project,source,model` to compare projects.",
    covered: group_by_project_source_model,
  },
  Tip {
    text: "Find expensive groups with `--sort cost --limit 10`.",
    covered: sort_cost_with_limit,
  },
  Tip {
    text: "Run `llm-tokei graph --24h` for an hourly activity plot.",
    covered: graph_24h,
  },
  Tip {
    text: "Use `--source codex,claude` to limit discovery to selected agents.",
    covered: source_codex_claude,
  },
  Tip {
    text: "Add `-h` for compact table values.",
    covered: human,
  },
  Tip {
    text: "Use `--split-input` to show uncached input separately.",
    covered: split_input,
  },
  Tip {
    text: "Run `llm-tokei update` to refresh price history and model mappings.",
    covered: update_command,
  },
  Tip {
    text: "Use `--cost-per provider` to compare top cost contributors.",
    covered: cost_per_provider,
  },
  Tip {
    text: "Use `--since 12h --model 'gpt-*'` to inspect recent model usage.",
    covered: recent_gpt_model,
  },
  Tip {
    text: "Filter one provider with `--provider openai`.",
    covered: provider_openai,
  },
  Tip {
    text: "Focus on a workspace with `--cwd '*/project-*'`.",
    covered: cwd_filter,
  },
  Tip {
    text: "Try `--group-by date,source --date-bucket week` for weekly trends.",
    covered: group_by_date_source_week,
  },
  Tip {
    text: "Use `--cost official` to compare against published rates.",
    covered: cost_official,
  },
  Tip {
    text: "Try `--unit cost` to show spend instead of tokens.",
    covered: unit_cost,
  },
  Tip {
    text: "Use `llm-tokei graph --chart heatmap --month` for a compact activity overview.",
    covered: graph_heatmap_month,
  },
  Tip {
    text: "Run `llm-tokei graph --width 100` to control graph spacing.",
    covered: graph_width,
  },
  Tip {
    text: "Use `--format json` for scripts and dashboards.",
    covered: format_json,
  },
  Tip {
    text: "Save a shareable chart with `llm-tokei graph --format svg > activity.svg`.",
    covered: format_svg,
  },
  Tip {
    text: "Run `--no-cache` after source files change outside the cache.",
    covered: no_cache,
  },
  Tip {
    text: "Add `--verbose` to inspect parsing warnings.",
    covered: verbose,
  },
  Tip {
    text: "Use `--avg session` to compare typical session usage.",
    covered: avg_session,
  },
  Tip {
    text: "Use `--asc` to surface the smallest groups first.",
    covered: ascending,
  },
  Tip {
    text: "Use `--limit 10` to keep a report focused.",
    covered: limit,
  },
  Tip {
    text: "Use `--table-width 100` to fit a fixed-width report.",
    covered: table_width,
  },
  Tip {
    text: "Use `--today` for usage since local midnight.",
    covered: today,
  },
  Tip {
    text: "Use `--period 1m` for a rolling 30-day view.",
    covered: period_1m,
  },
  Tip {
    text: "Group by `date,project` with `--date-bucket month` for monthly project trends.",
    covered: group_by_date_project_month,
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
    if !(tip.covered)(args) {
      return Some(tip.text);
    }
  }

  None
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
  effective_period(args).is_some_and(|period| period.eq_ignore_ascii_case(expected))
}

fn group_by_contains(args: &Args, dimensions: &[&str]) -> bool {
  dimensions
    .iter()
    .all(|dimension| args.group_by.iter().any(|group| group.eq_ignore_ascii_case(dimension)))
}

fn source_contains(args: &Args, sources: &[&str]) -> bool {
  args.source.as_ref().is_some_and(|selected| {
    sources
      .iter()
      .all(|source| selected.iter().any(|current| current.eq_ignore_ascii_case(source)))
  })
}

fn is_graph(args: &Args) -> bool {
  matches!(args.cmd, Some(Cmd::Graph { .. }))
}

fn period_7d(args: &Args) -> bool {
  period_is(args, "7d")
}

fn group_by_project_source_model(args: &Args) -> bool {
  group_by_contains(args, &["project", "source", "model"])
}

fn sort_cost_with_limit(args: &Args) -> bool {
  args.sort.eq_ignore_ascii_case("cost") && args.limit.is_some()
}

fn graph_24h(args: &Args) -> bool {
  is_graph(args) && period_is(args, "24h")
}

fn source_codex_claude(args: &Args) -> bool {
  source_contains(args, &["codex", "claude"])
}

fn human(args: &Args) -> bool {
  args.human
}

fn split_input(args: &Args) -> bool {
  args.split_input
}

fn update_command(args: &Args) -> bool {
  matches!(args.cmd, Some(Cmd::Update { .. }))
}

fn cost_per_provider(args: &Args) -> bool {
  args
    .cost_per
    .as_deref()
    .is_some_and(|dimension| dimension.eq_ignore_ascii_case("provider"))
}

fn recent_gpt_model(args: &Args) -> bool {
  args
    .since
    .as_deref()
    .is_some_and(|since| since.eq_ignore_ascii_case("12h"))
    && args
      .model
      .as_deref()
      .is_some_and(|model| model.eq_ignore_ascii_case("gpt-*"))
}

fn provider_openai(args: &Args) -> bool {
  args
    .provider
    .as_deref()
    .is_some_and(|provider| provider.eq_ignore_ascii_case("openai"))
}

fn cwd_filter(args: &Args) -> bool {
  args.cwd.is_some()
}

fn group_by_date_source_week(args: &Args) -> bool {
  group_by_contains(args, &["date", "source"]) && args.date_bucket == DateBucket::Week
}

fn cost_official(args: &Args) -> bool {
  args.cost == CostMode::Official
}

fn unit_cost(args: &Args) -> bool {
  args.unit == Some(Unit::Cost)
}

fn graph_heatmap_month(args: &Args) -> bool {
  matches!(
    args.cmd,
    Some(Cmd::Graph {
      chart: GraphChart::Heatmap,
      ..
    })
  ) && period_is(args, "month")
}

fn graph_width(args: &Args) -> bool {
  matches!(args.cmd, Some(Cmd::Graph { width: Some(_), .. }))
}

fn format_json(args: &Args) -> bool {
  args.format == Format::Json
}

fn format_svg(args: &Args) -> bool {
  args.format == Format::Svg
}

fn no_cache(args: &Args) -> bool {
  args.no_cache
}

fn verbose(args: &Args) -> bool {
  args.verbose
}

fn avg_session(args: &Args) -> bool {
  args.avg == Some(crate::cli::AvgBy::Session)
}

fn ascending(args: &Args) -> bool {
  args.asc
}

fn limit(args: &Args) -> bool {
  args.limit.is_some()
}

fn table_width(args: &Args) -> bool {
  args.table_width.is_some()
}

fn today(args: &Args) -> bool {
  period_is(args, "today")
}

fn period_1m(args: &Args) -> bool {
  period_is(args, "1m")
}

fn group_by_date_project_month(args: &Args) -> bool {
  group_by_contains(args, &["date", "project"]) && args.date_bucket == DateBucket::Month
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
