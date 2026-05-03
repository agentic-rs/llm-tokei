use crate::model::{Source, UsageRecord};
use crate::sources::copilot_shutdown::{
  normalize_copilot_model, records_from_shutdown_model_metrics, timestamp_from_event, ShutdownRecordArgs,
};
use crate::sources::UsageSource;
use anyhow::Result;
use serde_json::Value;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

const FALLBACK_MODEL: &str = "unknown";

pub struct CopilotCliSource {
  pub roots: Vec<PathBuf>,
}

impl CopilotCliSource {
  pub fn new(roots: Vec<PathBuf>) -> Self {
    Self { roots }
  }

  pub fn default_paths() -> Vec<PathBuf> {
    let mut bases = Vec::new();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
      bases.push(home.join(".copilot/session-state"));
    }
    bases.into_iter().filter(|p| p.exists()).collect()
  }

  pub fn discover_files(&self) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for root in &self.roots {
      if !root.exists() {
        continue;
      }
      for entry in WalkDir::new(root)
        .min_depth(2)
        .max_depth(2)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
      {
        if !entry.file_type().is_file() {
          continue;
        }
        let path = entry.path();
        if path.file_name().and_then(|n| n.to_str()) == Some("events.jsonl") {
          files.push(path.to_path_buf());
        }
      }
    }
    files
  }

  pub fn parse_file(path: &Path) -> Result<Option<Vec<UsageRecord>>> {
    parse_session(path)
  }
}

impl UsageSource for CopilotCliSource {
  fn name(&self) -> &'static str {
    "copilot-cli"
  }

  fn collect(&self) -> Result<Vec<UsageRecord>> {
    let mut out = Vec::new();
    for path in self.discover_files() {
      if let Ok(Some(recs)) = parse_session(&path) {
        out.extend(recs);
      }
    }
    Ok(out)
  }
}

fn parse_session(path: &Path) -> Result<Option<Vec<UsageRecord>>> {
  let events = read_jsonl_events(path)?;
  if events.is_empty() {
    return Ok(None);
  }

  let session_id = events.iter().find_map(|event| {
    if event.get("type").and_then(|v| v.as_str()) == Some("session.start") {
      event
        .pointer("/data/sessionId")
        .and_then(|v| v.as_str())
        .map(str::to_string)
    } else {
      None
    }
  });

  let shutdown_records: Vec<UsageRecord> = events
    .iter()
    .flat_map(|event| {
      records_from_shutdown_model_metrics(ShutdownRecordArgs {
        source: Source::CopilotCli,
        source_path: path,
        session_id: session_id.clone(),
        project_cwd: None,
        project_name: None,
        event,
      })
    })
    .collect();
  if !shutdown_records.is_empty() {
    return Ok(Some(shutdown_records));
  }

  let records = estimate_records_from_events(path, session_id, &events);
  if records.is_empty() {
    Ok(None)
  } else {
    Ok(Some(records))
  }
}

fn read_jsonl_events(path: &Path) -> Result<Vec<Value>> {
  let f = File::open(path)?;
  let reader = BufReader::new(f);
  let mut events = Vec::new();
  for line in reader.lines() {
    let line = match line {
      Ok(l) => l,
      Err(_) => continue,
    };
    if line.trim().is_empty() {
      continue;
    }
    if let Ok(event) = serde_json::from_str(&line) {
      events.push(event);
    }
  }
  Ok(events)
}

fn estimate_records_from_events(path: &Path, session_id: Option<String>, events: &[Value]) -> Vec<UsageRecord> {
  let mut records = Vec::new();
  let mut current_model = FALLBACK_MODEL.to_string();
  let mut pending_input = 0;

  for event in events {
    let event_type = event.get("type").and_then(|v| v.as_str());
    if event_type == Some("session.model_change") {
      if let Some(model) = event.pointer("/data/newModel").and_then(|v| v.as_str()) {
        current_model = normalize_copilot_model(model.to_string()).1;
      }
    }

    if matches!(
      event_type,
      Some("system.message" | "user.message" | "tool.execution_complete")
    ) {
      pending_input += rough_tokens(event.get("data").unwrap_or(&Value::Null));
    }

    if event_type == Some("assistant.message") {
      let (provider, model) = normalize_copilot_model(current_model.clone());
      let output_exact = event.pointer("/data/outputTokens").and_then(|v| v.as_u64());
      let output_estimated_tokens = rough_tokens(event.pointer("/data/content").unwrap_or(&Value::Null))
        + rough_tokens(event.pointer("/data/toolRequests").unwrap_or(&Value::Null));
      records.push(UsageRecord {
        source: Source::CopilotCli,
        session_id: session_id.clone().unwrap_or_else(|| fallback_session_id(path)),
        session_title: None,
        project_cwd: None,
        project_name: None,
        provider: Some(provider),
        model: Some(model),
        ts: timestamp_from_event(event),
        input: pending_input,
        output: output_exact.unwrap_or(output_estimated_tokens),
        input_estimated: true,
        output_estimated: output_exact.is_none(),
        reasoning: 0,
        cache_read: 0,
        cache_write: 0,
        mode: None,
        agent: None,
        is_compaction: false,
        rounds: 1,
        turns: 1,
        cost_embedded: None,
      });
      pending_input = rough_tokens(event.pointer("/data/content").unwrap_or(&Value::Null))
        + rough_tokens(event.pointer("/data/toolRequests").unwrap_or(&Value::Null));
    }

    if event_type == Some("session.compaction_complete") {
      if let Some(usage) = event.pointer("/data/compactionTokensUsed") {
        let model_raw = usage
          .get("model")
          .and_then(|v| v.as_str())
          .map(str::to_string)
          .unwrap_or_else(|| current_model.clone());
        let (provider, model) = normalize_copilot_model(model_raw);
        records.push(UsageRecord {
          source: Source::CopilotCli,
          session_id: session_id.clone().unwrap_or_else(|| fallback_session_id(path)),
          session_title: None,
          project_cwd: None,
          project_name: None,
          provider: Some(provider),
          model: Some(model),
          ts: timestamp_from_event(event),
          input: token_alias(usage, "inputTokens", "input"),
          output: token_alias(usage, "outputTokens", "output"),
          input_estimated: false,
          output_estimated: false,
          reasoning: 0,
          cache_read: token_alias(usage, "cacheReadTokens", "cachedInput"),
          cache_write: usage.get("cacheWriteTokens").and_then(|v| v.as_u64()).unwrap_or(0),
          mode: Some("compaction".to_string()),
          agent: Some("compaction".to_string()),
          is_compaction: true,
          rounds: 1,
          turns: 1,
          cost_embedded: None,
        });
      }
    }
  }

  records
}

fn rough_tokens(value: &Value) -> u64 {
  rough_chars(value).div_ceil(4)
}

fn rough_chars(value: &Value) -> u64 {
  match value {
    Value::String(s) => s.chars().count() as u64,
    Value::Array(items) => items.iter().map(rough_chars).sum(),
    Value::Object(map) => map.values().map(rough_chars).sum(),
    _ => 0,
  }
}

fn token_alias(value: &Value, primary: &str, fallback: &str) -> u64 {
  value
    .get(primary)
    .and_then(|v| v.as_u64())
    .or_else(|| value.get(fallback).and_then(|v| v.as_u64()))
    .unwrap_or(0)
}

fn fallback_session_id(path: &Path) -> String {
  path
    .parent()
    .and_then(|p| p.file_name())
    .and_then(|s| s.to_str())
    .unwrap_or("unknown")
    .to_string()
}
