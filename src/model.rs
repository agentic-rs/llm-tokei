use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Source {
  Codex,
  OpenCode,
  Claude,
  Copilot,
  CopilotCli,
  PiAgent,
}

impl Source {
  pub fn as_str(&self) -> &'static str {
    match self {
      Source::Codex => "codex",
      Source::OpenCode => "opencode",
      Source::Claude => "claude",
      Source::Copilot => "copilot",
      Source::CopilotCli => "copilot-cli",
      Source::PiAgent => "pi-agent",
    }
  }
}

impl std::fmt::Display for Source {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.write_str(self.as_str())
  }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum SessionKind {
  #[default]
  Root,
  SubAgent,
}

impl SessionKind {
  pub fn as_str(self) -> &'static str {
    match self {
      Self::Root => "root",
      Self::SubAgent => "sub_agent",
    }
  }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UsageRecord {
  pub source: Source,
  pub session_id: String,
  pub session_kind: SessionKind,
  pub parent_session_id: Option<String>,
  pub session_title: Option<String>,
  pub project_cwd: Option<String>,
  pub project_name: Option<String>,
  pub provider: Option<String>,
  pub model: Option<String>,
  pub ts: DateTime<Utc>,
  pub prompt: u64,
  pub completion: u64,
  pub input_bytes: u64,
  pub output_bytes: u64,
  pub input_estimated: bool,
  pub output_estimated: bool,
  pub input_bytes_estimated: bool,
  pub output_bytes_estimated: bool,
  pub reasoning: u64,
  pub cache_read: u64,
  pub cache_write: u64,
  /// Source-reported total token count when available. This is stored as-is
  /// from the upstream entry and may differ from the derived display total.
  pub total_direct: Option<u64>,
  pub mode: Option<String>,
  pub agent: Option<String>,
  pub is_compaction: bool,
  /// Number of user-initiated rounds (prompts) in this record.
  pub rounds: u64,
  /// Number of total API calls (including tool-call continuations) in this record.
  pub calls: u64,
  /// Cost reported by the source (e.g. OpenCode); USD.
  pub cost_embedded: Option<f64>,
}

/// A normalized usage record together with its stable source-file identity.
///
/// `origin_key` is not user-facing output. It allows the cache to distinguish
/// two equal usage records that originated from different source events.
#[derive(Debug, Clone)]
pub struct ParsedUsageRecord {
  pub origin_key: String,
  pub record: UsageRecord,
}

/// The result of parsing one cacheable source file.
///
/// `complete` is false when a JSONL reader encountered an invalid line. Such
/// a parse can still produce displayable records, but must not remove cached
/// records that were absent from the partial read.
#[derive(Debug, Clone)]
pub struct ParsedUsageFile {
  pub complete: bool,
  pub records: Vec<ParsedUsageRecord>,
}

impl ParsedUsageFile {
  pub fn new(complete: bool, mut records: Vec<ParsedUsageRecord>) -> Self {
    ensure_unique_origin_keys(&mut records);
    Self { complete, records }
  }

  pub fn into_usage_records(self) -> Vec<UsageRecord> {
    self.records.into_iter().map(|parsed| parsed.record).collect()
  }
}

fn ensure_unique_origin_keys(records: &mut [ParsedUsageRecord]) {
  let mut occurrences = HashMap::<String, usize>::new();
  let mut used = HashSet::new();
  for parsed in records {
    let base = parsed.origin_key.clone();
    let occurrence = occurrences.entry(base.clone()).or_default();
    loop {
      let candidate = if *occurrence == 0 {
        base.clone()
      } else {
        format!("{base}:duplicate:{occurrence}")
      };
      *occurrence += 1;
      if used.insert(candidate.clone()) {
        parsed.origin_key = candidate;
        break;
      }
    }
  }
}

impl UsageRecord {
  /// Displayed input includes prompt and cache traffic.
  pub fn display_input(&self) -> u64 {
    self
      .prompt
      .saturating_add(self.cache_read)
      .saturating_add(self.cache_write)
  }

  /// Displayed output includes visible completion and reasoning.
  pub fn display_output(&self) -> u64 {
    self.completion.saturating_add(self.reasoning)
  }

  /// Display total uses the displayed input/output columns as-is.
  pub fn total(&self) -> u64 {
    self.display_input().saturating_add(self.display_output())
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use chrono::TimeZone;

  fn record() -> UsageRecord {
    UsageRecord {
      source: Source::Codex,
      session_id: "session".to_string(),
      session_kind: SessionKind::Root,
      parent_session_id: None,
      session_title: None,
      project_cwd: None,
      project_name: None,
      provider: None,
      model: None,
      ts: Utc.timestamp_opt(0, 0).single().expect("epoch timestamp"),
      prompt: 0,
      completion: 0,
      input_bytes: 0,
      output_bytes: 0,
      input_estimated: false,
      output_estimated: false,
      input_bytes_estimated: false,
      output_bytes_estimated: false,
      reasoning: 0,
      cache_read: 0,
      cache_write: 0,
      total_direct: None,
      mode: None,
      agent: None,
      is_compaction: false,
      rounds: 0,
      calls: 0,
      cost_embedded: None,
    }
  }

  #[test]
  fn parsed_usage_file_disambiguates_duplicate_origin_keys() {
    let parsed = ParsedUsageFile::new(
      true,
      vec![
        ParsedUsageRecord {
          origin_key: "event".to_string(),
          record: record(),
        },
        ParsedUsageRecord {
          origin_key: "event".to_string(),
          record: record(),
        },
        ParsedUsageRecord {
          origin_key: "event:duplicate:1".to_string(),
          record: record(),
        },
      ],
    );

    assert_eq!(
      parsed
        .records
        .iter()
        .map(|record| record.origin_key.as_str())
        .collect::<Vec<_>>(),
      ["event", "event:duplicate:1", "event:duplicate:1:duplicate:1"]
    );
  }
}
