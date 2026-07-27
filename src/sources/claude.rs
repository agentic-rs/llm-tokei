use crate::model::{ParsedUsageFile, ParsedUsageRecord, Source, UsageRecord};
use crate::sources::{read_jsonl_with_status, JsonlPosition, UsageSource};
use anyhow::Result;
use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use tracing::debug;
use walkdir::WalkDir;

pub struct ClaudeSource {
  pub root: PathBuf,
}

impl ClaudeSource {
  pub fn new(root: PathBuf) -> Self {
    Self { root }
  }

  pub fn default_path() -> Option<PathBuf> {
    let base = std::env::var_os("CLAUDE_HOME")
      .map(PathBuf::from)
      .or_else(|| std::env::var_os("HOME").map(PathBuf::from).map(|p| p.join(".claude")))?;
    Some(base.join("projects"))
  }

  pub fn discover_files(&self) -> Vec<PathBuf> {
    if !self.root.exists() {
      return Vec::new();
    }
    WalkDir::new(&self.root)
      .follow_links(false)
      .into_iter()
      .filter_map(|e| e.ok())
      .filter(|e| e.file_type().is_file())
      .filter_map(|entry| {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str())?;
        if name.ends_with(".jsonl") {
          Some(path.to_path_buf())
        } else {
          None
        }
      })
      .collect()
  }

  pub fn parse_file(path: &Path) -> Result<Option<Vec<UsageRecord>>> {
    let records = Self::parse_cache_file(path)?.into_usage_records();
    Ok(if records.is_empty() { None } else { Some(records) })
  }

  pub fn parse_cache_file(path: &Path) -> Result<ParsedUsageFile> {
    parse_session(path)
  }
}

#[derive(Debug, Deserialize)]
struct Line {
  #[serde(default, rename = "type")]
  kind: Option<String>,
  #[serde(default)]
  timestamp: Option<String>,
  #[serde(default, rename = "sessionId")]
  session_id: Option<String>,
  #[serde(default)]
  uuid: Option<String>,
  #[serde(default)]
  id: Option<String>,
  #[serde(default)]
  cwd: Option<String>,
  #[serde(default)]
  message: Option<MessageObj>,
}

#[derive(Debug, Deserialize)]
struct MessageObj {
  #[serde(default)]
  id: Option<String>,
  #[serde(default)]
  #[allow(dead_code)]
  role: Option<String>,
  #[serde(default)]
  model: Option<String>,
  #[serde(default)]
  usage: Option<Usage>,
  #[serde(default)]
  content: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, Default)]
struct Usage {
  #[serde(default)]
  input_tokens: u64,
  #[serde(default)]
  output_tokens: u64,
  #[serde(default)]
  cache_read_input_tokens: u64,
  #[serde(default)]
  cache_creation_input_tokens: u64,
  #[serde(default)]
  cache_creation: Option<CacheCreation>,
}

#[derive(Debug, Deserialize, Default)]
struct CacheCreation {
  #[serde(default)]
  ephemeral_5m_input_tokens: u64,
  #[serde(default)]
  ephemeral_1h_input_tokens: u64,
}

impl UsageSource for ClaudeSource {
  fn name(&self) -> &'static str {
    "claude"
  }

  fn collect(&self) -> Result<Vec<UsageRecord>> {
    let mut out = Vec::new();
    for path in self.discover_files() {
      debug!(source = "claude", file = %path.display(), "processing file");
      if let Ok(Some(recs)) = Self::parse_file(&path) {
        debug!(
          source = "claude",
          file = %path.display(),
          summary = %summarize(&recs),
          "file summary"
        );
        out.extend(recs);
      }
    }
    Ok(out)
  }
}

fn parse_session(path: &Path) -> Result<ParsedUsageFile> {
  let mut session_id: Option<String> = None;
  let mut cwd: Option<String> = None;

  // We can't construct the final record until we've resolved session_id/cwd
  // (they may appear on later lines). Stash raw turn data and finalize at end.
  struct PendingTurn {
    origin_key: String,
    ts: Option<DateTime<Utc>>,
    model: Option<String>,
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
    rounds_at: u64, // user_rounds counter snapshot at this turn (1 if part of round 1)
  }
  let mut pending: Vec<PendingTurn> = Vec::new();
  let mut user_rounds: u64 = 0;

  let complete = read_jsonl_with_status::<Line, _>(path, |parsed, position| {
    let ts = parsed
      .timestamp
      .as_deref()
      .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
      .map(|dt| dt.with_timezone(&Utc));

    if session_id.is_none() {
      if let Some(s) = parsed.session_id.clone() {
        session_id = Some(s);
      }
    }
    if cwd.is_none() {
      if let Some(c) = parsed.cwd.clone() {
        cwd = Some(c);
      }
    }

    if parsed.kind.as_deref() == Some("user") {
      if let Some(msg) = &parsed.message {
        if !is_tool_result(&msg.content) {
          user_rounds += 1;
        }
      } else {
        user_rounds += 1;
      }
    }

    if parsed.kind.as_deref() == Some("assistant") {
      if let Some(msg) = parsed.message {
        if let Some(u) = msg.usage {
          let cw = if let Some(cc) = u.cache_creation {
            cc.ephemeral_5m_input_tokens
              .saturating_add(cc.ephemeral_1h_input_tokens)
          } else {
            u.cache_creation_input_tokens
          };
          let cw = if cw == 0 { u.cache_creation_input_tokens } else { cw };
          pending.push(PendingTurn {
            origin_key: origin_key(
              parsed.uuid.as_deref().or(parsed.id.as_deref()).or(msg.id.as_deref()),
              position,
              0,
            ),
            ts,
            model: msg.model.filter(|m| !m.is_empty()),
            input: u.input_tokens,
            output: u.output_tokens,
            cache_read: u.cache_read_input_tokens,
            cache_write: cw,
            rounds_at: user_rounds.max(1),
          });
        }
      }
    }
  })?;

  if pending.is_empty() {
    return Ok(ParsedUsageFile::new(complete, Vec::new()));
  }

  let sid = session_id.unwrap_or_else(|| {
    path
      .file_stem()
      .and_then(|s| s.to_str())
      .unwrap_or("unknown")
      .to_string()
  });
  let cwd = cwd.or_else(|| decode_dir_name(path));

  // Distribute `rounds` across calls: assign rounds=1 to the *first* call of
  // each round, 0 to subsequent calls in the same round, so the sum equals
  // total user rounds.
  let mut last_round_seen: u64 = 0;
  let mut records = Vec::with_capacity(pending.len());
  for turn in pending {
    let rounds_this = if turn.rounds_at != last_round_seen {
      last_round_seen = turn.rounds_at;
      1
    } else {
      0
    };
    let prompt = turn.input;
    let completion = turn.output;
    let ts = turn
      .ts
      .unwrap_or_else(|| Utc.timestamp_opt(0, 0).single().unwrap_or_else(Utc::now));
    records.push(ParsedUsageRecord {
      origin_key: turn.origin_key,
      record: UsageRecord {
        source: Source::Claude,
        session_id: sid.clone(),
        session_kind: crate::model::SessionKind::Root,
        parent_session_id: None,
        session_title: None,
        project_cwd: cwd.clone(),
        project_name: None,
        provider: Some("anthropic".to_string()),
        model: turn.model,
        ts,
        prompt,
        completion,
        input_bytes: 0,
        output_bytes: 0,
        input_estimated: false,
        output_estimated: false,
        input_bytes_estimated: true,
        output_bytes_estimated: true,
        reasoning: 0,
        cache_read: turn.cache_read,
        cache_write: turn.cache_write,
        total_direct: None,
        mode: None,
        agent: None,
        is_compaction: false,
        rounds: rounds_this,
        calls: 1,
        cost_embedded: None,
      },
    });
  }

  // Ensure at least one record carries rounds=1 even if no `user` line was seen.
  if records.iter().all(|record| record.record.rounds == 0) {
    if let Some(first) = records.first_mut() {
      first.record.rounds = 1;
    }
  }

  Ok(ParsedUsageFile::new(complete, records))
}

fn origin_key(event_id: Option<&str>, position: JsonlPosition, emitted_slot: usize) -> String {
  match event_id.filter(|id| !id.is_empty()) {
    Some(id) => format!("event:{id}:slot:{emitted_slot}"),
    None => format!("offset:{}:slot:{emitted_slot}", position.byte_offset),
  }
}

/// Returns true if the message content is a tool-result injection
/// (i.e. not a human-authored prompt).
fn is_tool_result(content: &Option<serde_json::Value>) -> bool {
  match content {
    None => false,
    Some(serde_json::Value::Array(arr)) => arr.iter().any(|item| {
      item
        .get("type")
        .and_then(|v| v.as_str())
        .is_some_and(|t| t == "tool_result" || t == "tool_use")
    }),
    _ => false,
  }
}

/// Claude encodes the project directory as the absolute path with `/` and other
/// separators replaced by `-`. We can't perfectly invert it (a real `-` in the
/// path is indistinguishable from a separator), but we can return the encoded
/// form so it's at least visible/groupable.
fn decode_dir_name(path: &Path) -> Option<String> {
  let parent = path.parent()?;
  let name = parent.file_name()?.to_str()?;
  if name.is_empty() {
    return None;
  }
  // Best-effort: replace leading '-' with '/' to look path-like.
  let decoded = if let Some(rest) = name.strip_prefix('-') {
    format!("/{}", rest.replace('-', "/"))
  } else {
    name.to_string()
  };
  Some(decoded)
}

fn summarize(records: &[UsageRecord]) -> String {
  let input: u64 = records.iter().map(UsageRecord::display_input).sum();
  let output: u64 = records.iter().map(UsageRecord::display_output).sum();
  let reasoning: u64 = records.iter().map(|r| r.reasoning).sum();
  let cache_read: u64 = records.iter().map(|r| r.cache_read).sum();
  let cache_write: u64 = records.iter().map(|r| r.cache_write).sum();
  let input_est = records.iter().any(|r| r.input_estimated);
  let output_est = records.iter().any(|r| r.output_estimated);
  format!(
    "records={}, input={}, output={}, reasoning={}, cache_r={}, cache_w={}",
    records.len(),
    if input_est {
      format!("~{input}")
    } else {
      input.to_string()
    },
    if output_est {
      format!("~{output}")
    } else {
      output.to_string()
    },
    reasoning,
    cache_read,
    cache_write
  )
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn cache_parser_uses_stable_offset_keys_when_events_have_no_id() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/claude/projects/-home-me-proj/abc123.jsonl");

    let parsed = ClaudeSource::parse_cache_file(&path).expect("parse fixture");
    let keys = parsed
      .records
      .iter()
      .map(|record| record.origin_key.clone())
      .collect::<Vec<_>>();

    assert!(parsed.complete);
    assert_eq!(keys.len(), 2);
    assert!(keys.iter().all(|key| key.starts_with("offset:")));
    assert_eq!(
      keys,
      ClaudeSource::parse_cache_file(&path)
        .expect("reparse fixture")
        .records
        .iter()
        .map(|record| record.origin_key.clone())
        .collect::<Vec<_>>()
    );
  }

  #[test]
  fn origin_key_prefers_a_source_event_id() {
    assert_eq!(
      origin_key(
        Some("assistant-event"),
        JsonlPosition {
          byte_offset: 42,
          line_number: 3,
        },
        0,
      ),
      "event:assistant-event:slot:0"
    );
  }
}
