use crate::activity::ActivitySeries;
use crate::cli::Unit;
use chrono::{Datelike, Duration, NaiveDate};

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
    format!("Active {}/{} days", series.active_days, series.len()),
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

pub fn unit_name(unit: Unit) -> &'static str {
  match unit {
    Unit::Tokens => "Token",
    Unit::Bytes => "Byte",
    Unit::Cost => "Cost",
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
  use crate::activity::ActivitySeries;

  fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
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
}
