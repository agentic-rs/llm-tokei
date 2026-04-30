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
  model: Option<String>,
  #[serde(default)]
  usage: Option<Usage>,
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
    if !self.root.exists() {
      return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in WalkDir::new(&self.root)
      .follow_links(false)
      .into_iter()
      .filter_map(|e| e.ok())
    {
      if !entry.file_type().is_file() {
        continue;
      }
      let path = entry.path();
      let name = match path.file_name().and_then(|n| n.to_str()) {
        Some(n) => n,
        None => continue,
      };
      if !name.ends_with(".jsonl") {
        continue;
      }
      if let Ok(Some(rec)) = parse_session(path) {
        out.push(rec);
      }
    }
    Ok(out)
  }
}

fn parse_session(path: &Path) -> Result<Option<UsageRecord>> {
  let f = File::open(path)?;
  let reader = BufReader::new(f);

  let mut session_id: Option<String> = None;
  let mut cwd: Option<String> = None;
  let mut model: Option<String> = None;
  let mut first_ts: Option<DateTime<Utc>> = None;
  let mut last_ts: Option<DateTime<Utc>> = None;

  let mut input_uncached: u64 = 0;
  let mut output: u64 = 0;
  let mut cache_read: u64 = 0;
  let mut cache_write: u64 = 0;
  let mut assistant_rows: u64 = 0;

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

    if let Some(ts_str) = &parsed.timestamp {
      if let Ok(dt) = DateTime::parse_from_rfc3339(ts_str) {
        let utc = dt.with_timezone(&Utc);
        last_ts = Some(utc);
        if first_ts.is_none() {
          first_ts = Some(utc);
        }
      }
    }
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

    if parsed.kind.as_deref() == Some("assistant") {
      if let Some(msg) = parsed.message {
        if let Some(m) = msg.model {
          if !m.is_empty() {
            model = Some(m);
          }
        }
        if let Some(u) = msg.usage {
          input_uncached = input_uncached.saturating_add(u.input_tokens);
          output = output.saturating_add(u.output_tokens);
          cache_read = cache_read.saturating_add(u.cache_read_input_tokens);
          let cw = if let Some(cc) = u.cache_creation {
            cc.ephemeral_5m_input_tokens
              .saturating_add(cc.ephemeral_1h_input_tokens)
          } else {
            u.cache_creation_input_tokens
          };
          // Fallback: if both forms are present prefer the detailed sum,
          // but if cache_creation object is empty (zeros) and the flat
          // field is set, use the flat field.
          let cw = if cw == 0 { u.cache_creation_input_tokens } else { cw };
          cache_write = cache_write.saturating_add(cw);
          assistant_rows += 1;
        }
      }
    }
  }

  if assistant_rows == 0 {
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

  let ts = last_ts
    .or(first_ts)
    .unwrap_or_else(|| Utc.timestamp_opt(0, 0).single().unwrap_or_else(Utc::now));

  // `input` is the full prompt total (matching codex/opencode semantics):
  // uncached + cache_read + cache_write (cache_creation is billed input that
  // also became cached for later turns).
  let input = input_uncached.saturating_add(cache_read).saturating_add(cache_write);

  Ok(Some(UsageRecord {
    source: Source::Claude,
    session_id: sid,
    session_title: None,
    project_cwd: cwd,
    project_name: None,
    provider: Some("anthropic".to_string()),
    model,
    ts,
    input,
    output,
    reasoning: 0,
    cache_read,
    cache_write,
    cost_embedded: None,
  }))
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
