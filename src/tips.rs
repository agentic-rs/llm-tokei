use chrono::{DateTime, Utc};

const TIPS: &[&str] = &[
  "Use --7d to focus on the last week.",
  "Try --group-by project,source,model to compare projects.",
  "Find expensive groups with --sort cost --limit 10.",
  "Run llm-tokei graph --24h for an hourly activity plot.",
  "Use --source codex,claude to limit discovery to selected agents.",
  "Add -h for compact table values.",
  "Use --split-input to show uncached input separately.",
  "Run llm-tokei update to refresh price history and model mappings.",
  "Use --cost-per provider to compare top cost contributors.",
];

pub(crate) fn tip_for_hour(now: DateTime<Utc>) -> &'static str {
  let hour = now.timestamp().div_euclid(60 * 60);
  let index = hour.rem_euclid(TIPS.len() as i64) as usize;
  TIPS[index]
}

#[cfg(test)]
mod tests {
  use super::*;
  use chrono::{Duration, TimeZone};

  #[test]
  fn tip_is_stable_within_an_hour_and_rotates_afterward() {
    let start = Utc
      .with_ymd_and_hms(2026, 8, 3, 12, 10, 0)
      .single()
      .expect("valid timestamp");

    assert_eq!(tip_for_hour(start), tip_for_hour(start + Duration::minutes(49)));
    assert_ne!(tip_for_hour(start), tip_for_hour(start + Duration::hours(1)));
    assert_eq!(
      tip_for_hour(start),
      tip_for_hour(start + Duration::hours(TIPS.len() as i64))
    );
  }
}
