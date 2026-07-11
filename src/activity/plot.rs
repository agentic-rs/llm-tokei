use super::series::{ActivitySeries, HourlyActivitySeries};
use crate::cli::Unit;
use chrono::{Datelike, Duration, Local, NaiveDate};

pub struct ActivityPlotPoint {
  pub value: f64,
  pub level: u8,
  pub axis_label: String,
  pub tooltip: String,
}

pub struct ActivityPlot {
  pub title: String,
  pub summary: String,
  pub accessible_title: String,
  pub unit: Unit,
  pub points: Vec<ActivityPlotPoint>,
}

impl ActivityPlot {
  pub fn from_daily(series: &ActivitySeries) -> Self {
    let points = series
      .days
      .iter()
      .map(|day| ActivityPlotPoint {
        value: day.value,
        level: day.level,
        axis_label: format_date_short(day.date),
        tooltip: format!(
          "{}: {}",
          day.date.format("%b %-d, %Y"),
          format_value(day.value, series.unit, day.estimated)
        ),
      })
      .collect();
    Self {
      title: title(series),
      summary: summary(series),
      accessible_title: format!("{} activity graph", unit_name(series.unit)),
      unit: series.unit,
      points,
    }
  }

  pub fn from_hourly(series: &HourlyActivitySeries) -> Self {
    let local_start = series.start.with_timezone(&Local);
    let local_end = series.end.with_timezone(&Local);
    let same_date = local_start.date_naive() == local_end.date_naive();
    let offset_changed = local_start.offset().local_minus_utc() != local_end.offset().local_minus_utc();
    let points = series
      .hours
      .iter()
      .map(|hour| {
        let local = hour.start.with_timezone(&Local);
        ActivityPlotPoint {
          value: hour.value,
          level: hour.level,
          axis_label: if offset_changed {
            local.format("%H:%M %:z").to_string()
          } else if same_date {
            local.format("%H:%M").to_string()
          } else {
            local.format("%b %-d %H:%M").to_string()
          },
          tooltip: format!(
            "{}: {}",
            local.format("%b %-d, %Y %H:%M %:z"),
            format_value(hour.value, series.unit, hour.estimated)
          ),
        }
      })
      .collect();
    Self {
      title: hourly_title(series),
      summary: hourly_summary(series),
      accessible_title: format!("Hourly {} activity graph", unit_name_lower(series.unit)),
      unit: series.unit,
      points,
    }
  }

  pub fn len(&self) -> usize {
    self.points.len()
  }

  pub fn is_empty(&self) -> bool {
    self.points.is_empty()
  }
}

pub struct CalendarGrid {
  pub start: NaiveDate,
  pub week_count: usize,
}

impl CalendarGrid {
  pub fn new(series: &ActivitySeries) -> Option<Self> {
    if series.is_empty() {
      return None;
    }
    let start = series.start - Duration::days(series.start.weekday().num_days_from_sunday() as i64);
    let remaining = 6 - series.end.weekday().num_days_from_sunday();
    let end = series.end + Duration::days(remaining as i64);
    let week_count = ((end - start).num_days() as usize / 7) + 1;
    Some(Self { start, week_count })
  }

  pub fn date(&self, week: usize, weekday: usize) -> NaiveDate {
    self.start + Duration::days((week * 7 + weekday) as i64)
  }

  pub fn week_for(&self, date: NaiveDate) -> usize {
    ((date - self.start).num_days() as usize) / 7
  }
}

pub fn month_labels(series: &ActivitySeries, grid: &CalendarGrid) -> Vec<(usize, String)> {
  let mut labels = Vec::new();
  let mut previous = None;
  for day in &series.days {
    let month = (day.date.year(), day.date.month());
    if previous == Some(month) {
      continue;
    }
    previous = Some(month);
    labels.push((grid.week_for(day.date), day.date.format("%b").to_string()));
  }
  labels
}

pub fn title(series: &ActivitySeries) -> String {
  format!(
    "{} activity · {}",
    unit_name(series.unit),
    format_date_range(series.start, series.end)
  )
}

pub fn summary(series: &ActivitySeries) -> String {
  let mut parts = vec![
    format!("Total {}", format_value(series.total, series.unit, series.estimated)),
    format!(
      "Active {}/{} {}",
      series.active_days,
      series.len(),
      pluralize(series.len(), "day", "days")
    ),
  ];
  if let Some(best) = series.best_day() {
    parts.push(format!(
      "Best {}: {}",
      format_date_short(best.date),
      format_value(best.value, series.unit, best.estimated)
    ));
  }
  parts.push(format!(
    "Longest streak {} {}",
    series.longest_streak,
    pluralize(series.longest_streak, "day", "days")
  ));
  parts.join(" · ")
}

fn hourly_title(series: &HourlyActivitySeries) -> String {
  format!(
    "Hourly {} activity · {}",
    unit_name_lower(series.unit),
    format_hour_range(series)
  )
}

fn hourly_summary(series: &HourlyActivitySeries) -> String {
  let local_start = series.start.with_timezone(&Local);
  let local_end = series.end.with_timezone(&Local);
  let same_date = local_start.date_naive() == local_end.date_naive();
  let offset_changed = local_start.offset().local_minus_utc() != local_end.offset().local_minus_utc();
  let mut parts = vec![
    format!("Total {}", format_value(series.total, series.unit, series.estimated)),
    format!(
      "Active {}/{} {}",
      series.active_hours,
      series.len(),
      pluralize(series.len(), "hour", "hours")
    ),
  ];
  if let Some(best) = series.best_hour() {
    let local = best.start.with_timezone(&Local);
    let label = if offset_changed {
      local.format("%b %-d %H:%M %:z").to_string()
    } else if same_date {
      local.format("%H:%M").to_string()
    } else {
      local.format("%b %-d %H:%M").to_string()
    };
    parts.push(format!(
      "Best {label}: {}",
      format_value(best.value, series.unit, best.estimated)
    ));
  }
  parts.push(format!(
    "Longest streak {} {}",
    series.longest_streak,
    pluralize(series.longest_streak, "hour", "hours")
  ));
  parts.join(" · ")
}

pub fn unit_name(unit: Unit) -> &'static str {
  match unit {
    Unit::Tokens => "Token",
    Unit::Bytes => "Byte",
    Unit::Cost => "Cost",
  }
}

fn unit_name_lower(unit: Unit) -> &'static str {
  match unit {
    Unit::Tokens => "token",
    Unit::Bytes => "byte",
    Unit::Cost => "cost",
  }
}

pub fn format_value(value: f64, unit: Unit, estimated: bool) -> String {
  let prefix = if estimated { "~" } else { "" };
  if unit == Unit::Cost {
    let value = if value >= 100.0 {
      format!("${value:.0}")
    } else if value >= 1.0 {
      format!("${value:.2}")
    } else if value > 0.0 {
      format!("${value:.4}")
    } else {
      "$0.00".to_string()
    };
    return format!("{prefix}{value}");
  }

  let (scaled, suffix) = if value >= 1_000_000_000_000.0 {
    (value / 1_000_000_000_000.0, "T")
  } else if value >= 1_000_000_000.0 {
    (value / 1_000_000_000.0, "B")
  } else if value >= 1_000_000.0 {
    (value / 1_000_000.0, "M")
  } else if value >= 1_000.0 {
    (value / 1_000.0, "K")
  } else {
    return format!("{prefix}{value:.0}");
  };
  format!("{prefix}{scaled:.1}{suffix}")
}

pub fn format_date_short(date: NaiveDate) -> String {
  date.format("%b %-d").to_string()
}

fn format_date_range(start: NaiveDate, end: NaiveDate) -> String {
  if start == end {
    return start.format("%b %-d, %Y").to_string();
  }
  if start.year() == end.year() && start.month() == end.month() {
    return format!("{}–{}", start.format("%b %-d"), end.format("%-d, %Y"));
  }
  if start.year() == end.year() {
    return format!("{}–{}", start.format("%b %-d"), end.format("%b %-d, %Y"));
  }
  format!("{}–{}", start.format("%b %-d, %Y"), end.format("%b %-d, %Y"))
}

fn format_hour_range(series: &HourlyActivitySeries) -> String {
  let start = series.start.with_timezone(&Local);
  let end = series.end.with_timezone(&Local);
  if start.offset().local_minus_utc() != end.offset().local_minus_utc() {
    if start.date_naive() == end.date_naive() {
      return format!("{}–{}", start.format("%b %-d, %H:%M %:z"), end.format("%H:%M %:z, %Y"));
    }
    return format!(
      "{}–{}",
      start.format("%b %-d, %H:%M %:z"),
      end.format("%b %-d, %H:%M %:z, %Y")
    );
  }
  if start.date_naive() == end.date_naive() {
    return format!("{}–{}", start.format("%b %-d, %H:%M"), end.format("%H:%M, %Y"));
  }
  if start.year() == end.year() {
    return format!("{}–{}", start.format("%b %-d, %H:%M"), end.format("%b %-d, %H:%M, %Y"));
  }
  format!(
    "{}–{}",
    start.format("%b %-d, %Y %H:%M"),
    end.format("%b %-d, %Y %H:%M")
  )
}

fn pluralize<'a>(count: usize, singular: &'a str, plural: &'a str) -> &'a str {
  if count == 1 {
    singular
  } else {
    plural
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use chrono::{DateTime, Utc};

  fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
  }

  fn time(raw: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(raw).unwrap().with_timezone(&Utc)
  }

  #[test]
  fn formats_ranges_across_months_and_years() {
    assert_eq!(
      format_date_range(date(2026, 1, 1), date(2026, 7, 1)),
      "Jan 1–Jul 1, 2026"
    );
    assert_eq!(
      format_date_range(date(2025, 7, 1), date(2026, 7, 1)),
      "Jul 1, 2025–Jul 1, 2026"
    );
  }

  #[test]
  fn aligns_calendar_to_sunday_and_saturday() {
    let series = ActivitySeries::from_values(date(2026, 7, 1), vec![1.0], Unit::Tokens);
    let grid = CalendarGrid::new(&series).unwrap();
    assert_eq!(grid.start, date(2026, 6, 28));
    assert_eq!(grid.date(grid.week_count - 1, 6), date(2026, 7, 4));
    assert_eq!(grid.date(0, 3), date(2026, 7, 1));
  }

  #[test]
  fn hourly_plot_uses_hour_labels_and_summary_units() {
    let series = HourlyActivitySeries::from_values(time("2026-07-11T01:00:00Z"), vec![0.0, 10.0, 20.0], Unit::Tokens);
    let plot = ActivityPlot::from_hourly(&series);

    assert!(plot.title.starts_with("Hourly token activity · "));
    assert!(plot.summary.contains("Active 2/3 hours"));
    assert!(plot.summary.contains("Longest streak 2 hours"));
    assert!(plot.points[0].axis_label.contains(':'));
    assert!(plot.points[0].tooltip.ends_with(": 0"));
  }
}
