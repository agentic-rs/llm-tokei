use crate::aggregate::Filters;
use crate::cli::Unit;
use crate::model::UsageRecord;
use crate::pricing::{CostMode, PricingTable};
use chrono::{DateTime, Duration, Local, NaiveDate, Timelike, Utc};

#[derive(Debug, Clone)]
pub struct ActivityDay {
  pub date: NaiveDate,
  pub value: f64,
  pub level: u8,
  pub estimated: bool,
}

#[derive(Debug, Clone)]
pub struct ActivitySeries {
  pub start: NaiveDate,
  pub end: NaiveDate,
  pub unit: Unit,
  pub days: Vec<ActivityDay>,
  pub total: f64,
  pub active_days: usize,
  pub longest_streak: usize,
  pub estimated: bool,
}

#[derive(Debug, Clone)]
pub struct ActivityHour {
  pub start: DateTime<Utc>,
  pub value: f64,
  pub level: u8,
  pub estimated: bool,
}

#[derive(Debug, Clone)]
pub struct HourlyActivitySeries {
  /// Exact requested lower bound; the first bucket may be partial.
  pub start: DateTime<Utc>,
  /// Exact requested upper bound; the last bucket may be partial.
  pub end: DateTime<Utc>,
  pub unit: Unit,
  pub hours: Vec<ActivityHour>,
  pub total: f64,
  pub active_hours: usize,
  pub longest_streak: usize,
  pub estimated: bool,
}

impl ActivitySeries {
  pub fn from_records(
    records: &[UsageRecord],
    filters: &Filters,
    pricing: &PricingTable,
    cost_mode: CostMode,
    unit: Unit,
    start: NaiveDate,
    end: NaiveDate,
  ) -> Self {
    let day_count = inclusive_day_count(start, end);
    let mut values = vec![0.0; day_count];
    let mut estimated = vec![false; day_count];

    for record in records.iter().filter(|record| filters.matches(record, pricing)) {
      let date = record.ts.with_timezone(&Local).date_naive();
      if date < start || date > end {
        continue;
      }
      let index = (date - start).num_days() as usize;
      let value = record_value(record, pricing, cost_mode, unit);
      values[index] += value;
      estimated[index] |= value > 0.0 && record_is_estimated(record, unit);
    }

    Self::from_values_with_estimates(start, values, estimated, unit)
  }

  #[cfg(test)]
  pub(crate) fn from_values(start: NaiveDate, values: Vec<f64>, unit: Unit) -> Self {
    let estimated = vec![false; values.len()];
    Self::from_values_with_estimates(start, values, estimated, unit)
  }

  fn from_values_with_estimates(start: NaiveDate, values: Vec<f64>, estimated: Vec<bool>, unit: Unit) -> Self {
    debug_assert_eq!(values.len(), estimated.len());
    let end = start + Duration::days(values.len().saturating_sub(1) as i64);
    let levels = quantile_levels(&values);
    let days = values
      .into_iter()
      .zip(levels)
      .zip(estimated)
      .enumerate()
      .map(|(index, ((value, level), estimated))| ActivityDay {
        date: start + Duration::days(index as i64),
        value,
        level,
        estimated,
      })
      .collect::<Vec<_>>();
    let total = days.iter().map(|day| day.value).sum();
    let active_days = days.iter().filter(|day| day.value > 0.0).count();
    let longest_streak = longest_streak_values(days.iter().map(|day| day.value));
    let estimated = days.iter().any(|day| day.estimated);

    Self {
      start,
      end,
      unit,
      days,
      total,
      active_days,
      longest_streak,
      estimated,
    }
  }

  pub fn best_day(&self) -> Option<&ActivityDay> {
    self
      .days
      .iter()
      .filter(|day| day.value > 0.0)
      .max_by(|a, b| a.value.total_cmp(&b.value).then_with(|| b.date.cmp(&a.date)))
  }

  pub fn day(&self, date: NaiveDate) -> Option<&ActivityDay> {
    if date < self.start || date > self.end {
      return None;
    }
    self.days.get((date - self.start).num_days() as usize)
  }

  pub fn len(&self) -> usize {
    self.days.len()
  }

  pub fn is_empty(&self) -> bool {
    self.days.is_empty()
  }
}

impl HourlyActivitySeries {
  pub fn from_records(
    records: &[UsageRecord],
    filters: &Filters,
    pricing: &PricingTable,
    cost_mode: CostMode,
    unit: Unit,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
  ) -> Self {
    let first_hour = floor_local_hour(start);
    // Anchor to local clock time, then advance by elapsed hours. Re-flooring
    // every instant loses buckets when a timezone changes by less than an hour.
    let hour_count = inclusive_hour_count(first_hour, end);
    let mut values = vec![0.0; hour_count];
    let mut estimated = vec![false; hour_count];

    for record in records.iter().filter(|record| filters.matches(record, pricing)) {
      if record.ts < start || record.ts > end {
        continue;
      }
      let index = (record.ts - first_hour).num_hours() as usize;
      debug_assert!(index < values.len());
      let value = record_value(record, pricing, cost_mode, unit);
      values[index] += value;
      estimated[index] |= value > 0.0 && record_is_estimated(record, unit);
    }

    Self::from_values_with_estimates(start, end, first_hour, values, estimated, unit)
  }

  #[cfg(test)]
  pub(crate) fn from_values(start: DateTime<Utc>, values: Vec<f64>, unit: Unit) -> Self {
    let end = if values.is_empty() {
      start
    } else {
      start + Duration::hours(values.len() as i64) - Duration::nanoseconds(1)
    };
    let estimated = vec![false; values.len()];
    Self::from_values_with_estimates(start, end, start, values, estimated, unit)
  }

  fn from_values_with_estimates(
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    first_hour: DateTime<Utc>,
    values: Vec<f64>,
    estimated: Vec<bool>,
    unit: Unit,
  ) -> Self {
    debug_assert_eq!(values.len(), estimated.len());
    let levels = quantile_levels(&values);
    let hours = values
      .into_iter()
      .zip(levels)
      .zip(estimated)
      .enumerate()
      .map(|(index, ((value, level), estimated))| ActivityHour {
        start: first_hour + Duration::hours(index as i64),
        value,
        level,
        estimated,
      })
      .collect::<Vec<_>>();
    let total = hours.iter().map(|hour| hour.value).sum();
    let active_hours = hours.iter().filter(|hour| hour.value > 0.0).count();
    let longest_streak = longest_streak_values(hours.iter().map(|hour| hour.value));
    let estimated = hours.iter().any(|hour| hour.estimated);

    Self {
      start,
      end,
      unit,
      hours,
      total,
      active_hours,
      longest_streak,
      estimated,
    }
  }

  pub fn best_hour(&self) -> Option<&ActivityHour> {
    self
      .hours
      .iter()
      .filter(|hour| hour.value > 0.0)
      .max_by(|a, b| a.value.total_cmp(&b.value).then_with(|| b.start.cmp(&a.start)))
  }

  pub fn len(&self) -> usize {
    self.hours.len()
  }
}

fn inclusive_day_count(start: NaiveDate, end: NaiveDate) -> usize {
  if end < start {
    0
  } else {
    (end - start).num_days() as usize + 1
  }
}

fn inclusive_hour_count(start: DateTime<Utc>, end: DateTime<Utc>) -> usize {
  if end < start {
    0
  } else {
    (end - start).num_hours() as usize + 1
  }
}

fn floor_local_hour(time: DateTime<Utc>) -> DateTime<Utc> {
  let local = time.with_timezone(&Local);
  let elapsed_seconds = i64::from(local.minute()) * 60 + i64::from(local.second());
  time - Duration::seconds(elapsed_seconds) - Duration::nanoseconds(i64::from(local.nanosecond()))
}

fn record_value(record: &UsageRecord, pricing: &PricingTable, cost_mode: CostMode, unit: Unit) -> f64 {
  match unit {
    Unit::Tokens => record.total() as f64,
    Unit::Bytes => record.input_bytes.saturating_add(record.output_bytes) as f64,
    Unit::Cost => pricing
      .cost_breakdown_for(record, cost_mode)
      .map(|breakdown| breakdown.total())
      .unwrap_or_default(),
  }
}

fn record_is_estimated(record: &UsageRecord, unit: Unit) -> bool {
  match unit {
    Unit::Tokens => record.input_estimated || record.output_estimated,
    Unit::Bytes => record.input_bytes_estimated || record.output_bytes_estimated,
    Unit::Cost => record.input_estimated || record.output_estimated,
  }
}

fn quantile_levels(values: &[f64]) -> Vec<u8> {
  let mut nonzero = values
    .iter()
    .copied()
    .filter(|value| value.is_finite() && *value > 0.0)
    .collect::<Vec<_>>();
  nonzero.sort_by(f64::total_cmp);

  values
    .iter()
    .map(|value| {
      if !value.is_finite() || *value <= 0.0 {
        return 0;
      }
      let rank = nonzero.partition_point(|candidate| candidate <= value);
      ((rank * 4).div_ceil(nonzero.len())).clamp(1, 4) as u8
    })
    .collect()
}

fn longest_streak_values(values: impl IntoIterator<Item = f64>) -> usize {
  let mut longest = 0;
  let mut current = 0;
  for value in values {
    if value > 0.0 {
      current += 1;
      longest = longest.max(current);
    } else {
      current = 0;
    }
  }
  longest
}

#[cfg(test)]
mod tests {
  use super::*;

  fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
  }

  fn time(raw: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(raw).unwrap().with_timezone(&Utc)
  }

  #[test]
  fn fills_dates_and_computes_summary() {
    let series = ActivitySeries::from_values(
      date(2026, 7, 1),
      vec![0.0, 10.0, 20.0, 0.0, 20.0, 5.0, 0.0],
      Unit::Tokens,
    );

    assert_eq!(series.end, date(2026, 7, 7));
    assert_eq!(series.total, 55.0);
    assert_eq!(series.active_days, 4);
    assert_eq!(series.longest_streak, 2);
    assert_eq!(series.best_day().unwrap().date, date(2026, 7, 3));
    assert_eq!(series.day(date(2026, 7, 6)).unwrap().value, 5.0);
    assert!(series.day(date(2026, 6, 30)).is_none());
  }

  #[test]
  fn assigns_relative_levels_by_nonzero_rank() {
    let series = ActivitySeries::from_values(date(2026, 7, 1), vec![0.0, 10.0, 20.0, 30.0, 40.0], Unit::Tokens);

    assert_eq!(
      series.days.iter().map(|day| day.level).collect::<Vec<_>>(),
      vec![0, 1, 2, 3, 4]
    );
  }

  #[test]
  fn equal_nonzero_values_share_the_highest_level() {
    let series = ActivitySeries::from_values(date(2026, 7, 1), vec![3.0, 3.0, 3.0], Unit::Tokens);
    assert_eq!(
      series.days.iter().map(|day| day.level).collect::<Vec<_>>(),
      vec![4, 4, 4]
    );
  }

  #[test]
  fn an_empty_reversed_range_has_no_days() {
    assert_eq!(inclusive_day_count(date(2026, 7, 2), date(2026, 7, 1)), 0);
  }

  #[test]
  fn hourly_series_fills_buckets_and_computes_summary() {
    let series = HourlyActivitySeries::from_values(
      time("2026-07-11T01:00:00Z"),
      vec![0.0, 10.0, 20.0, 0.0, 5.0],
      Unit::Tokens,
    );

    assert_eq!(series.hours[4].start, time("2026-07-11T05:00:00Z"));
    assert_eq!(series.total, 35.0);
    assert_eq!(series.active_hours, 3);
    assert_eq!(series.longest_streak, 2);
    assert_eq!(series.best_hour().unwrap().start, time("2026-07-11T03:00:00Z"));
  }

  #[test]
  fn hour_count_includes_partial_edge_buckets() {
    assert_eq!(
      inclusive_hour_count(
        floor_local_hour(time("2026-07-11T01:34:00Z")),
        time("2026-07-11T13:34:00Z")
      ),
      13
    );
  }
}
