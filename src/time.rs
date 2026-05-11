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
  // Date only
  if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
    let nd = d.and_hms_opt(0, 0, 0).unwrap();
    return Ok(Utc.from_utc_datetime(&nd));
  }
  // Relative: <num><unit>
  if let Some(dt) = parse_relative(s) {
    return Ok(dt);
  }
  Err(anyhow!(
    "could not parse time '{s}' (use YYYY-MM-DD, RFC3339, or e.g. 7d, 24h, 2w, 1mo)"
  ))
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

pub fn last_24h() -> DateTime<Utc> {
  Utc::now() - Duration::hours(24)
}

pub fn last_7d() -> DateTime<Utc> {
  Utc::now() - Duration::days(7)
}

pub fn last_1m() -> DateTime<Utc> {
  Utc::now() - Duration::days(30)
}

fn local_midnight(date: NaiveDate) -> DateTime<Utc> {
  let naive = date.and_hms_opt(0, 0, 0).unwrap();
  match Local.from_local_datetime(&naive) {
    chrono::LocalResult::Single(dt) => dt.with_timezone(&Utc),
    chrono::LocalResult::Ambiguous(dt, _) => dt.with_timezone(&Utc),
    chrono::LocalResult::None => Utc.from_utc_datetime(&naive),
  }
}
