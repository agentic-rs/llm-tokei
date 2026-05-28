use crate::model::UsageRecord;
use anyhow::Result;
use chrono::{DateTime, TimeZone, Utc};
use serde::de::DeserializeOwned;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

pub mod claude;
pub mod codex;
pub mod copilot;
pub mod copilot_cli;
pub mod copilot_shutdown;
pub mod dump;
pub mod opencode;
pub mod pi_agent;

#[allow(dead_code)]
pub trait UsageSource {
  fn name(&self) -> &'static str;
  fn collect(&self) -> Result<Vec<UsageRecord>>;
}

/// Read a JSONL file and call `visit` with each successfully parsed line.
/// Empty lines and lines that fail to parse are silently skipped, matching
/// the historical behavior of every per-source reader.
pub fn read_jsonl<T, F>(path: &Path, mut visit: F) -> Result<()>
where
  T: DeserializeOwned,
  F: FnMut(T),
{
  let file = File::open(path)?;
  let reader = BufReader::new(file);
  for line in reader.lines() {
    let line = match line {
      Ok(l) => l,
      Err(_) => continue,
    };
    if line.trim().is_empty() {
      continue;
    }
    if let Ok(parsed) = serde_json::from_str::<T>(&line) {
      visit(parsed);
    }
  }
  Ok(())
}

/// Convenience: collect all JSONL records into a `Vec`.
pub fn read_jsonl_collect<T: DeserializeOwned>(path: &Path) -> Result<Vec<T>> {
  let mut out = Vec::new();
  read_jsonl(path, |value| out.push(value))?;
  Ok(out)
}

/// Convert a Unix millisecond timestamp into UTC, defaulting to "now"
/// for unrepresentable values.
pub fn ms_to_dt(ms: i64) -> DateTime<Utc> {
  let secs = ms.div_euclid(1000);
  let nanos = (ms.rem_euclid(1000) * 1_000_000) as u32;
  Utc.timestamp_opt(secs, nanos).single().unwrap_or_else(Utc::now)
}

/// Common one-line summary used by every source's debug logging.
pub fn summarize_records(records: &[UsageRecord]) -> String {
  let input: u64 = records.iter().map(UsageRecord::display_input).sum();
  let output: u64 = records.iter().map(UsageRecord::display_output).sum();
  let reasoning: u64 = records.iter().map(|r| r.reasoning).sum();
  let cache_read: u64 = records.iter().map(|r| r.cache_read).sum();
  let cache_write: u64 = records.iter().map(|r| r.cache_write).sum();
  let input_est = records.iter().any(|r| r.input_estimated);
  let output_est = records.iter().any(|r| r.output_estimated);
  let fmt = |est: bool, n: u64| if est { format!("~{n}") } else { n.to_string() };
  format!(
    "records={}, input={}, output={}, reasoning={}, cache_r={}, cache_w={}",
    records.len(),
    fmt(input_est, input),
    fmt(output_est, output),
    reasoning,
    cache_read,
    cache_write
  )
}
