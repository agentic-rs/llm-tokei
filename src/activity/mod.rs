mod plot;
mod series;
mod svg;
mod terminal;

use crate::aggregate::Filters;
use crate::cli::{Format, GraphChart, Unit};
use crate::model::UsageRecord;
use crate::pricing::{CostMode, PricingTable};
use anyhow::Result;
use chrono::{Duration, Local, Utc};
use series::{ActivitySeries, HourlyActivitySeries};
use svg::{render_activity_svg, render_hourly_activity_svg};
use terminal::{render_activity_terminal, render_hourly_activity_terminal, ActivityTerminalOptions};

const HOURLY_CUTOFF_HOURS: i64 = 30;

#[derive(Clone, Copy)]
pub struct ActivityRenderOptions<'a> {
  pub chart: GraphChart,
  pub format: Format,
  pub unit: Unit,
  pub cost_mode: CostMode,
  pub use_color: bool,
  pub width: Option<usize>,
  pub command: &'a str,
}

pub fn render_activity(
  records: &[UsageRecord],
  filters: &Filters,
  pricing: &PricingTable,
  options: ActivityRenderOptions<'_>,
) -> Result<String> {
  let terminal_options = ActivityTerminalOptions {
    use_color: options.use_color,
    width: options.width,
  };
  let now = Utc::now();

  if options.chart != GraphChart::Heatmap {
    if let Some((start, end)) =
      activity_time_range(filters, now)?.filter(|(start, end)| *end - *start < Duration::hours(HOURLY_CUTOFF_HOURS))
    {
      let series =
        HourlyActivitySeries::from_records(records, filters, pricing, options.cost_mode, options.unit, start, end);
      return Ok(match options.format {
        Format::Table => render_hourly_activity_terminal(&series, &terminal_options),
        Format::Svg => render_hourly_activity_svg(&series, options.command),
        Format::Json => unreachable!("graph JSON output is rejected before collecting records"),
      });
    }
  }

  let (start, end) = activity_date_range(filters, now)?;
  let series = ActivitySeries::from_records(records, filters, pricing, options.cost_mode, options.unit, start, end);
  Ok(match options.format {
    Format::Table => render_activity_terminal(&series, options.chart, &terminal_options),
    Format::Svg => render_activity_svg(&series, options.chart, options.command),
    Format::Json => unreachable!("graph JSON output is rejected before collecting records"),
  })
}

fn activity_time_range(
  filters: &Filters,
  now: chrono::DateTime<chrono::Utc>,
) -> Result<Option<(chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)>> {
  let Some(start) = filters.since else {
    return Ok(None);
  };
  let end = filters.until.unwrap_or(now);
  if start > end {
    anyhow::bail!("graph: start time {start} is after end time {end}");
  }
  Ok(Some((start, end)))
}

fn activity_date_range(
  filters: &Filters,
  now: chrono::DateTime<chrono::Utc>,
) -> Result<(chrono::NaiveDate, chrono::NaiveDate)> {
  let end = filters.until.unwrap_or(now).with_timezone(&Local).date_naive();
  let start = filters
    .since
    .map(|since| since.with_timezone(&Local).date_naive())
    .unwrap_or_else(|| end - Duration::days(364));
  if start > end {
    anyhow::bail!("graph: start date {start} is after end date {end}");
  }
  Ok((start, end))
}
