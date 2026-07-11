use anyhow::{anyhow, Result};
use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, TimeZone, Utc};

/// Parse a "since/until" expression: RFC3339 date(time), `YYYY-MM-DD`,
/// or relative like `7d`, `12h`, `1w`, `1mo`.
pub fn parse_when(s: &str) -> Result<DateTime<Utc>> {
  let s = s.trim();
  if s.is_empty() {
    return Err(anyhow!("empty time expression"));
  }
  // RFC3339
  if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
    return Ok(dt.with_timezone(&Utc));
  }
  // Date-only values are local calendar dates, consistent with named periods.
  if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
    return Ok(local_midnight(d));
  }
  // Relative: <num><unit>
  if let Some(dt) = parse_relative(s) {
    return Ok(dt);
  }
  Err(anyhow!(
    "could not parse time '{s}' (use YYYY-MM-DD, RFC3339, or e.g. 7d, 24h, 2w, 1mo)"
  ))
}

/// Parse an upper time bound. A date-only value includes the complete local
/// calendar day; timestamps and relative expressions remain exact instants.
pub fn parse_until(s: &str) -> Result<DateTime<Utc>> {
  let s = s.trim();
  if let Ok(date) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
    let next = date.succ_opt().ok_or_else(|| anyhow!("date '{s}' is out of range"))?;
    return Ok(local_midnight(next) - Duration::nanoseconds(1));
  }
  parse_when(s)
}

/// Parse a period expression: named calendar periods (today, week, month)
/// or any expression accepted by `parse_when` (relative like 3d/12h/2w, absolute dates).
pub fn parse_period(s: &str) -> Result<DateTime<Utc>> {
  match s.trim().to_lowercase().as_str() {
    "today" => Ok(start_of_today()),
    "week" => Ok(start_of_week()),
    "month" => Ok(start_of_month()),
    // `--1m` is the documented rolling one-month shortcut. Keep `m` as
    // minutes for general relative expressions such as `--since 30m`.
    "1m" => Ok(Utc::now() - Duration::days(30)),
    _ => parse_when(s),
  }
}

fn parse_relative(s: &str) -> Option<DateTime<Utc>> {
  let bytes = s.as_bytes();
  let mut idx = 0;
  while idx < bytes.len() && bytes[idx].is_ascii_digit() {
    idx += 1;
  }
  if idx == 0 {
    return None;
  }
  let num: i64 = s[..idx].parse().ok()?;
  let unit = s[idx..].trim().to_lowercase();
  let dur = match unit.as_str() {
    "s" | "sec" | "secs" | "second" | "seconds" => Duration::seconds(num),
    "m" | "min" | "mins" | "minute" | "minutes" => Duration::minutes(num),
    "h" | "hr" | "hrs" | "hour" | "hours" => Duration::hours(num),
    "d" | "day" | "days" => Duration::days(num),
    "w" | "wk" | "wks" | "week" | "weeks" => Duration::weeks(num),
    "mo" | "month" | "months" => Duration::days(num * 30),
    "y" | "yr" | "yrs" | "year" | "years" => Duration::days(num * 365),
    _ => return None,
  };
  Some(Utc::now() - dur)
}

/// Bucket a timestamp into a label.
pub fn date_bucket(ts: DateTime<Utc>, unit: &str) -> String {
  match unit {
    "week" => {
      let iso = ts.iso_week();
      format!("{}-W{:02}", iso.year(), iso.week())
    }
    "month" => format!("{}-{:02}", ts.year(), ts.month()),
    _ => ts.format("%Y-%m-%d").to_string(),
  }
}

pub fn start_of_today() -> DateTime<Utc> {
  local_midnight(Local::now().date_naive())
}

pub fn start_of_week() -> DateTime<Utc> {
  let now = Local::now();
  let start = now.date_naive() - Duration::days(now.weekday().num_days_from_monday() as i64);
  local_midnight(start)
}

pub fn start_of_month() -> DateTime<Utc> {
  let now = Local::now();
  local_midnight(NaiveDate::from_ymd_opt(now.year(), now.month(), 1).unwrap())
}

fn local_midnight(date: NaiveDate) -> DateTime<Utc> {
  let naive = date.and_hms_opt(0, 0, 0).unwrap();
  match Local.from_local_datetime(&naive) {
    chrono::LocalResult::Single(dt) => dt.with_timezone(&Utc),
    chrono::LocalResult::Ambiguous(dt, _) => dt.with_timezone(&Utc),
    chrono::LocalResult::None => Utc.from_utc_datetime(&naive),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn to_local_date(dt: DateTime<Utc>) -> chrono::NaiveDate {
    dt.with_timezone(&Local).date_naive()
  }

  #[test]
  fn parse_period_named_today() {
    let dt = parse_period("today").unwrap();
    assert_eq!(to_local_date(dt), Local::now().date_naive());
  }

  #[test]
  fn parse_period_named_week() {
    let dt = parse_period("week").unwrap();
    let local_now = Local::now();
    let expected_monday = local_now.date_naive() - Duration::days(local_now.weekday().num_days_from_monday() as i64);
    assert_eq!(to_local_date(dt), expected_monday);
  }

  #[test]
  fn parse_period_named_month() {
    let dt = parse_period("month").unwrap();
    let local_date = dt.with_timezone(&Local).date_naive();
    assert_eq!(local_date.day(), 1);
    let now = Local::now();
    assert_eq!(local_date.month(), now.month());
    assert_eq!(local_date.year(), now.year());
  }

  #[test]
  fn parse_period_named_case_insensitive() {
    assert!(parse_period("Today").is_ok());
    assert!(parse_period("WEEK").is_ok());
    assert!(parse_period("Month").is_ok());
  }

  #[test]
  fn parse_period_relative_hours() {
    let dt = parse_period("12h").unwrap();
    let diff = Utc::now() - dt;
    assert!(diff.num_hours() >= 11 && diff.num_hours() <= 13);
  }

  #[test]
  fn parse_period_relative_days() {
    let dt = parse_period("3d").unwrap();
    let diff = Utc::now() - dt;
    assert!(diff.num_days() >= 2 && diff.num_days() <= 4);
  }

  #[test]
  fn parse_period_relative_weeks() {
    let dt = parse_period("2w").unwrap();
    let diff = Utc::now() - dt;
    assert!(diff.num_days() >= 13 && diff.num_days() <= 15);
  }

  #[test]
  fn parse_period_relative_months() {
    let dt = parse_period("6mo").unwrap();
    let diff = Utc::now() - dt;
    assert!(diff.num_days() >= 179 && diff.num_days() <= 181);
  }

  #[test]
  fn parse_period_one_month_shortcut_is_thirty_days() {
    let dt = parse_period("1m").unwrap();
    let diff = Utc::now() - dt;
    assert!(diff.num_days() >= 29 && diff.num_days() <= 31);
  }

  #[test]
  fn parse_period_relative_years() {
    let dt = parse_period("1y").unwrap();
    let diff = Utc::now() - dt;
    assert!(diff.num_days() >= 364 && diff.num_days() <= 366);
  }

  #[test]
  fn parse_period_absolute_date() {
    let dt = parse_period("2025-01-15").unwrap();
    assert_eq!(to_local_date(dt), NaiveDate::from_ymd_opt(2025, 1, 15).unwrap());
  }

  #[test]
  fn parse_period_rfc3339() {
    let dt = parse_period("2025-06-01T12:00:00Z").unwrap();
    assert_eq!(dt.format("%Y-%m-%dT%H:%M:%S").to_string(), "2025-06-01T12:00:00");
  }

  #[test]
  fn parse_period_invalid() {
    assert!(parse_period("foobar").is_err());
  }

  #[test]
  fn parse_when_relative_minutes() {
    let dt = parse_when("30m").unwrap();
    let diff = Utc::now() - dt;
    assert!(diff.num_minutes() >= 29 && diff.num_minutes() <= 31);
  }

  #[test]
  fn parse_when_relative_seconds() {
    let dt = parse_when("60s").unwrap();
    let diff = Utc::now() - dt;
    assert!(diff.num_seconds() >= 59 && diff.num_seconds() <= 61);
  }

  #[test]
  fn parse_when_relative_word_units() {
    assert!(parse_when("3 days").is_ok());
    assert!(parse_when("2 hours").is_ok());
    assert!(parse_when("1 week").is_ok());
  }

  #[test]
  fn parse_when_empty_error() {
    assert!(parse_when("").is_err());
    assert!(parse_when("  ").is_err());
  }

  #[test]
  fn parse_until_date_includes_the_complete_local_day() {
    let date = NaiveDate::from_ymd_opt(2025, 1, 15).unwrap();
    let until = parse_until("2025-01-15").unwrap();
    let next_midnight = local_midnight(date.succ_opt().unwrap());
    assert_eq!(to_local_date(until), date);
    assert_eq!(next_midnight - until, Duration::nanoseconds(1));
  }
}
