use crate::model::{Source, UsageRecord};
use crate::sources::UsageSource;
use anyhow::Result;
use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use tracing::debug;
use walkdir::WalkDir;

pub struct CodexSource {
  pub root: PathBuf,
}

impl CodexSource {
  pub fn new(root: PathBuf) -> Self {
    Self { root }
  }

  pub fn default_path() -> Option<PathBuf> {
    let base = std::env::var_os("CODEX_HOME")
      .map(PathBuf::from)
      .or_else(|| std::env::var_os("HOME").map(PathBuf::from).map(|p| p.join(".codex")))?;
    Some(base.join("sessions"))
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
    parse_rollout(path)
  }
}

#[derive(Debug, Deserialize)]
struct RolloutLine {
  #[serde(default, rename = "type")]
  kind: Option<String>,
  #[serde(default)]
  timestamp: Option<String>,
  #[serde(default)]
  payload: Option<serde_json::Value>,
  // session_meta inline fields (some versions emit at top level)
  #[serde(default)]
  id: Option<String>,
  #[serde(default)]
  cwd: Option<String>,
  #[serde(default)]
  model: Option<String>,
  #[serde(default)]
  originator: Option<String>,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct TokenUsage {
  #[serde(default)]
  input_tokens: u64,
  #[serde(default)]
  cached_input_tokens: u64,
  #[serde(default)]
  output_tokens: u64,
  #[serde(default)]
  reasoning_output_tokens: u64,
  #[serde(default)]
  total_tokens: u64,
}

impl UsageSource for CodexSource {
  fn name(&self) -> &'static str {
    "codex"
  }

  fn collect(&self) -> Result<Vec<UsageRecord>> {
    let mut out = Vec::new();
    for path in self.discover_files() {
      debug!(source = "codex", file = %path.display(), "processing file");
      if let Ok(Some(recs)) = Self::parse_file(&path) {
        debug!(
          source = "codex",
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

fn parse_rollout(path: &Path) -> Result<Option<Vec<UsageRecord>>> {
  let f = File::open(path)?;
  let reader = BufReader::new(f);

  let mut session_id: Option<String> = None;
  let mut cwd: Option<String> = None;
  let mut model: Option<String> = None;
  let mut provider: Option<String> = None;
  let mut session_ts: Option<DateTime<Utc>> = None;

  // We emit one record per `token_count` event. Source data is one of:
  //   - `last_token_usage` (per-turn delta), preferred when present
  //   - `total_token_usage` (cumulative), in which case we emit deltas
  //     vs. the previous cumulative snapshot.
  struct Turn {
    ts: DateTime<Utc>,
    model: Option<String>,
    provider: Option<String>,
    usage: TokenUsage,
    rounds: u64,
  }
  let mut turns: Vec<Turn> = Vec::new();
  let mut prev_total: Option<TokenUsage> = None;
  let mut pending_round: u64 = 0; // turn_context events that haven't been attributed yet
  let mut last_ts: Option<DateTime<Utc>> = None;

  for line in reader.lines() {
    let line = match line {
      Ok(l) => l,
      Err(_) => continue,
    };
    if line.trim().is_empty() {
      continue;
    }
    let parsed: RolloutLine = match serde_json::from_str(&line) {
      Ok(p) => p,
      Err(_) => continue,
    };

    if let Some(ts_str) = &parsed.timestamp {
      if let Ok(dt) = DateTime::parse_from_rfc3339(ts_str) {
        let utc = dt.with_timezone(&Utc);
        last_ts = Some(utc);
        if session_ts.is_none() {
          session_ts = Some(utc);
        }
      }
    }

    match parsed.kind.as_deref() {
      Some("session_meta") => {
        if let Some(payload) = &parsed.payload {
          let meta_holder = payload.get("meta").unwrap_or(payload);
          if session_id.is_none() {
            session_id = meta_holder.get("id").and_then(|v| v.as_str()).map(|s| s.to_string());
          }
          if cwd.is_none() {
            cwd = meta_holder.get("cwd").and_then(|v| v.as_str()).map(|s| s.to_string());
          }
          if model.is_none() {
            model = meta_holder.get("model").and_then(|v| v.as_str()).map(|s| s.to_string());
          }
          if provider.is_none() {
            provider = meta_holder
              .get("model_provider")
              .and_then(|v| v.as_str())
              .map(|s| s.to_string())
              .or_else(|| {
                meta_holder
                  .get("originator")
                  .and_then(|v| v.as_str())
                  .map(|s| s.to_string())
              });
          }
        }
        if session_id.is_none() {
          session_id = parsed.id.clone();
        }
        if cwd.is_none() {
          cwd = parsed.cwd.clone();
        }
        if model.is_none() {
          model = parsed.model.clone();
        }
        if provider.is_none() {
          provider = parsed.originator.clone();
        }
      }
      Some("event_msg") => {
        if let Some(payload) = &parsed.payload {
          let inner_kind = payload.get("type").and_then(|v| v.as_str());
          if inner_kind == Some("token_count") {
            let info = payload.get("info").unwrap_or(payload);

            let mut turn_usage: Option<TokenUsage> = None;

            if let Some(last) = info.get("last_token_usage") {
              if let Ok(t) = serde_json::from_value::<TokenUsage>(last.clone()) {
                turn_usage = Some(t);
              }
            }

            if turn_usage.is_none() {
              if let Some(total) = info.get("total_token_usage") {
                if let Ok(t) = serde_json::from_value::<TokenUsage>(total.clone()) {
                  let delta = match &prev_total {
                    Some(prev) => sub_usage(&t, prev),
                    None => t.clone(),
                  };
                  prev_total = Some(t);
                  turn_usage = Some(delta);
                }
              }
            } else if let Some(total) = info.get("total_token_usage") {
              if let Ok(t) = serde_json::from_value::<TokenUsage>(total.clone()) {
                prev_total = Some(t);
              }
            }

            if let Some(usage) = turn_usage {
              let ts = last_ts
                .or(session_ts)
                .unwrap_or_else(|| Utc.timestamp_opt(0, 0).single().unwrap_or_else(Utc::now));
              let rounds = std::mem::take(&mut pending_round);
              turns.push(Turn {
                ts,
                model: model.clone(),
                provider: provider.clone(),
                usage,
                rounds,
              });
            }
          }
        }
      }
      Some("turn_context") => {
        pending_round += 1;
        if let Some(payload) = &parsed.payload {
          if let Some(m) = payload.get("model").and_then(|v| v.as_str()) {
            model = Some(m.to_string());
          }
          if let Some(p) = payload.get("model_provider").and_then(|v| v.as_str()) {
            provider = Some(p.to_string());
          }
        }
      }
      Some("response_item") if model.is_none() => {
        if let Some(payload) = &parsed.payload {
          if let Some(m) = payload.get("model").and_then(|v| v.as_str()) {
            model = Some(m.to_string());
          }
        }
      }
      _ => {}
    }
  }

  if turns.is_empty() {
    return Ok(None);
  }

  // If no turn_context events were ever observed, attribute one round to the
  // first turn so totals match historical behavior.
  if turns.iter().all(|t| t.rounds == 0) {
    turns[0].rounds = 1;
  }

  let sid = session_id.unwrap_or_else(|| {
    path
      .file_stem()
      .and_then(|s| s.to_str())
      .unwrap_or("unknown")
      .to_string()
  });

  let records = turns
    .into_iter()
    .map(|t| UsageRecord {
      source: Source::Codex,
      session_id: sid.clone(),
      session_title: None,
      project_cwd: cwd.clone(),
      project_name: None,
      provider: t.provider,
      model: t.model,
      ts: t.ts,
      input: t.usage.input_tokens,
      output: t.usage.output_tokens,
      input_bytes: 0,
      output_bytes: 0,
      input_estimated: false,
      output_estimated: false,
      input_bytes_estimated: true,
      output_bytes_estimated: true,
      reasoning: t.usage.reasoning_output_tokens,
      cache_read: t.usage.cached_input_tokens,
      cache_write: 0,
      mode: None,
      agent: None,
      is_compaction: false,
      rounds: t.rounds,
      turns: 1,
      cost_embedded: None,
    })
    .collect();

  Ok(Some(records))
}

fn sub_usage(a: &TokenUsage, b: &TokenUsage) -> TokenUsage {
  TokenUsage {
    input_tokens: a.input_tokens.saturating_sub(b.input_tokens),
    cached_input_tokens: a.cached_input_tokens.saturating_sub(b.cached_input_tokens),
    output_tokens: a.output_tokens.saturating_sub(b.output_tokens),
    reasoning_output_tokens: a.reasoning_output_tokens.saturating_sub(b.reasoning_output_tokens),
    total_tokens: a.total_tokens.saturating_sub(b.total_tokens),
  }
}

#[allow(dead_code)]
pub fn _phantom(_m: HashMap<String, String>) {}

fn summarize(records: &[UsageRecord]) -> String {
  let input: u64 = records.iter().map(|r| r.input).sum();
  let output: u64 = records.iter().map(|r| r.output).sum();
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
