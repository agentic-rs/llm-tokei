use crate::model::{Source, UsageRecord};
use crate::sources::UsageSource;
use anyhow::Result;
use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
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
  cwd: Option<String>,
  #[serde(default)]
  message: Option<MessageObj>,
}

#[derive(Debug, Deserialize)]
struct MessageObj {
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
      if let Ok(Some(recs)) = Self::parse_file(&path) {
        out.extend(recs);
      }
    }
    Ok(out)
  }
}

fn parse_session(path: &Path) -> Result<Option<Vec<UsageRecord>>> {
  let f = File::open(path)?;
  let reader = BufReader::new(f);

  let mut session_id: Option<String> = None;
  let mut cwd: Option<String> = None;

  // Per-turn records emitted as we encounter each assistant message with usage.
  let mut records: Vec<UsageRecord> = Vec::new();
  // We can't construct the final record until we've resolved session_id/cwd
  // (they may appear on later lines). Stash raw turn data and finalize at end.
  struct PendingTurn {
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

  for line in reader.lines() {
    let line = match line {
      Ok(l) => l,
      Err(_) => continue,
    };
    if line.trim().is_empty() {
      continue;
    }
    let parsed: Line = match serde_json::from_str(&line) {
      Ok(p) => p,
      Err(_) => continue,
    };

    let ts = parsed
      .timestamp
      .as_deref()
      .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
      .map(|dt| dt.with_timezone(&Utc));

    if session_id.is_none() {
      if let Some(s) = parsed.session_id {
        session_id = Some(s);
      }
    }
    if cwd.is_none() {
      if let Some(c) = parsed.cwd {
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
  }

  if pending.is_empty() {
    return Ok(None);
  }

  let sid = session_id.unwrap_or_else(|| {
    path
      .file_stem()
      .and_then(|s| s.to_str())
      .unwrap_or("unknown")
      .to_string()
  });
  let cwd = cwd.or_else(|| decode_dir_name(path));

  // Distribute `rounds` across turns: assign rounds=1 to the *first* turn of
  // each round, 0 to subsequent turns in the same round, so the sum equals
  // total user rounds.
  let mut last_round_seen: u64 = 0;
  for turn in pending.into_iter() {
    let rounds_this = if turn.rounds_at != last_round_seen {
      last_round_seen = turn.rounds_at;
      1
    } else {
      0
    };
    let input = turn.input.saturating_add(turn.cache_read).saturating_add(turn.cache_write);
    let ts = turn
      .ts
      .unwrap_or_else(|| Utc.timestamp_opt(0, 0).single().unwrap_or_else(Utc::now));
    records.push(UsageRecord {
      source: Source::Claude,
      session_id: sid.clone(),
      session_title: None,
      project_cwd: cwd.clone(),
      project_name: None,
      provider: Some("anthropic".to_string()),
      model: turn.model,
      ts,
      input,
      output: turn.output,
      reasoning: 0,
      cache_read: turn.cache_read,
      cache_write: turn.cache_write,
      rounds: rounds_this,
      turns: 1,
      cost_embedded: None,
    });
  }

  // Ensure at least one record carries rounds=1 even if no `user` line was seen.
  if records.iter().all(|r| r.rounds == 0) {
    if let Some(first) = records.first_mut() {
      first.rounds = 1;
    }
  }

  Ok(Some(records))
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
