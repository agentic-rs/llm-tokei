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

#[derive(Debug, Clone, Copy)]
pub struct JsonlPosition {
  pub byte_offset: u64,
  #[allow(dead_code)]
  pub line_number: usize,
}

#[derive(Debug, Clone)]
pub struct JsonlEntry<T> {
  pub position: JsonlPosition,
  pub value: T,
}

#[derive(Debug, Clone)]
pub struct JsonlRead<T> {
  pub complete: bool,
  pub entries: Vec<JsonlEntry<T>>,
}

/// Read a JSONL file and call `visit` with every valid line.
///
/// Invalid non-empty lines leave the read incomplete but do not discard other
/// valid lines. Callers can still render the valid records while the cache
/// avoids treating the partial result as a complete replacement snapshot.
pub fn read_jsonl_with_status<T, F>(path: &Path, mut visit: F) -> Result<bool>
where
  T: DeserializeOwned,
  F: FnMut(T, JsonlPosition),
{
  let file = File::open(path)?;
  let mut reader = BufReader::new(file);
  let mut byte_offset = 0u64;
  let mut line_number = 0usize;
  let mut complete = true;
  let mut bytes = Vec::new();

  loop {
    bytes.clear();
    let byte_count = reader.read_until(b'\n', &mut bytes)?;
    if byte_count == 0 {
      break;
    }
    line_number += 1;
    let position = JsonlPosition {
      byte_offset,
      line_number,
    };
    byte_offset = byte_offset.saturating_add(byte_count as u64);

    let Ok(line) = std::str::from_utf8(&bytes) else {
      complete = false;
      continue;
    };
    if line.trim().is_empty() {
      continue;
    }
    match serde_json::from_str::<T>(line) {
      Ok(parsed) => visit(parsed, position),
      Err(_) => complete = false,
    }
  }
  Ok(complete)
}

/// Collect JSONL records together with their positions and completeness.
pub fn read_jsonl_collect_with_status<T: DeserializeOwned>(path: &Path) -> Result<JsonlRead<T>> {
  let mut entries = Vec::new();
  let complete = read_jsonl_with_status(path, |value, position| entries.push(JsonlEntry { position, value }))?;
  Ok(JsonlRead { complete, entries })
}

/// Convenience: collect JSONL records into a `Vec`.
pub fn read_jsonl_collect<T: DeserializeOwned>(path: &Path) -> Result<Vec<T>> {
  Ok(
    read_jsonl_collect_with_status(path)?
      .entries
      .into_iter()
      .map(|entry| entry.value)
      .collect(),
  )
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
