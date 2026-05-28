use crate::model::{Source, UsageRecord};
use crate::sources::{read_jsonl, summarize_records, UsageSource};
use crate::text_count::{all_strings, BytesSink, SpanSink, StatsSink, TextSpan};
use anyhow::Result;
use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use tracing::debug;
use walkdir::WalkDir;

pub struct PiAgentSource {
  pub root: PathBuf,
}

impl PiAgentSource {
  pub fn new(root: PathBuf) -> Self {
    Self { root }
  }

  pub fn default_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    Some(home.join(".pi/agent/sessions"))
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

impl UsageSource for PiAgentSource {
  fn name(&self) -> &'static str {
    "pi-agent"
  }

  fn collect(&self) -> Result<Vec<UsageRecord>> {
    let mut out = Vec::new();
    for path in self.discover_files() {
      debug!(source = "pi-agent", file = %path.display(), "processing file");
      if let Ok(Some(recs)) = Self::parse_file(&path) {
        debug!(
          source = "pi-agent",
          file = %path.display(),
          summary = %summarize_records(&recs),
          "file summary"
        );
        out.extend(recs);
      }
    }
    Ok(out)
  }
}

#[derive(Debug, Deserialize)]
struct Line {
  #[serde(default, rename = "type")]
  kind: Option<String>,
  #[serde(default)]
  id: Option<String>,
  #[serde(default, rename = "parentId")]
  parent_id: Option<String>,
  #[serde(default)]
  timestamp: Option<String>,
  #[serde(default)]
  cwd: Option<String>,
  #[serde(default)]
  provider: Option<String>,
  #[serde(default, rename = "modelId")]
  model_id: Option<String>,
  #[serde(default)]
  message: Option<Message>,
}

#[derive(Debug, Deserialize)]
struct Message {
  #[serde(default)]
  role: Option<String>,
  #[serde(default)]
  content: Option<Value>,
  #[serde(default)]
  provider: Option<String>,
  #[serde(default)]
  model: Option<String>,
  #[serde(default)]
  usage: Option<Usage>,
  #[serde(default)]
  api: Option<String>,
  #[serde(default, rename = "responseId")]
  response_id: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct Usage {
  #[serde(default)]
  input: u64,
  #[serde(default)]
  output: u64,
  #[serde(default, rename = "cacheRead")]
  cache_read: u64,
  #[serde(default, rename = "cacheWrite")]
  cache_write: u64,
  #[serde(default, rename = "totalTokens")]
  total_tokens: Option<u64>,
  #[serde(default)]
  cost: Option<Cost>,
}

#[derive(Debug, Deserialize, Default)]
struct Cost {
  #[serde(default)]
  total: Option<f64>,
}

struct PendingTurn {
  ts: DateTime<Utc>,
  provider: Option<String>,
  model: Option<String>,
  usage: Usage,
  bytes: BytesSink,
  rounds: u64,
  mode: Option<String>,
  agent: Option<String>,
}

fn parse_session(path: &Path) -> Result<Option<Vec<UsageRecord>>> {
  let mut session_id = file_stem_or(path, "unknown");
  let mut cwd: Option<String> = None;
  let mut current_provider: Option<String> = None;
  let mut current_model: Option<String> = None;
  let mut pending_bytes = BytesSink::default();
  let mut pending_rounds = 0u64;
  let mut pending: Vec<PendingTurn> = Vec::new();

  read_jsonl::<Line, _>(path, |line| {
    let ts = parse_ts(line.timestamp.as_deref());
    match line.kind.as_deref() {
      Some("session") => {
        if let Some(id) = line.id {
          session_id = id;
        }
        if cwd.is_none() {
          cwd = line.cwd;
        }
      }
      Some("model_change") => {
        current_provider = line.provider;
        current_model = line.model_id;
      }
      Some("message") => {
        let Some(message) = line.message else {
          return;
        };
        let role = message.role.as_deref();
        if role == Some("user") {
          pending_rounds = pending_rounds.saturating_add(1);
        }
        visit_message(role, message.content.as_ref(), &mut pending_bytes);

        let Some(usage) = message.usage else {
          return;
        };
        pending.push(PendingTurn {
          ts,
          provider: message.provider.or_else(|| current_provider.clone()),
          model: message.model.or_else(|| current_model.clone()),
          usage,
          bytes: pending_bytes.take(),
          rounds: std::mem::take(&mut pending_rounds),
          mode: message.api,
          agent: message.response_id.or(line.parent_id),
        });
      }
      _ => {}
    }
  })?;

  if pending.is_empty() {
    return Ok(None);
  }
  if pending.iter().all(|t| t.rounds == 0) {
    pending[0].rounds = 1;
  }

  let records = pending
    .into_iter()
    .map(|turn| UsageRecord {
      source: Source::PiAgent,
      session_id: session_id.clone(),
      session_title: None,
      project_cwd: cwd.clone(),
      project_name: None,
      provider: turn.provider,
      model: turn.model,
      ts: turn.ts,
      prompt: turn.usage.input,
      completion: turn.usage.output,
      input_bytes: turn.bytes.input,
      output_bytes: turn.bytes.output.saturating_add(turn.bytes.reasoning),
      input_estimated: false,
      output_estimated: false,
      input_bytes_estimated: true,
      output_bytes_estimated: true,
      reasoning: 0,
      cache_read: turn.usage.cache_read,
      cache_write: turn.usage.cache_write,
      total_direct: turn.usage.total_tokens,
      mode: turn.mode,
      agent: turn.agent,
      is_compaction: false,
      rounds: turn.rounds,
      calls: 1,
      cost_embedded: turn.usage.cost.and_then(|c| c.total),
    })
    .collect();
  Ok(Some(records))
}

fn visit_message(role: Option<&str>, content: Option<&Value>, sink: &mut BytesSink) {
  let Some(role) = role.and_then(normalize_role) else {
    return;
  };
  let stats = all_strings::<StatsSink>(content);
  sink.text(TextSpan::new(role, "").with_stats(stats));
}

fn normalize_role(role: &str) -> Option<&'static str> {
  match role {
    "user" => Some("user"),
    "assistant" => Some("assistant"),
    "system" => Some("system"),
    "toolResult" => Some("tool_call_result"),
    _ => None,
  }
}

fn parse_ts(s: Option<&str>) -> DateTime<Utc> {
  s.and_then(|s| DateTime::parse_from_rfc3339(s).ok())
    .map(|dt| dt.with_timezone(&Utc))
    .unwrap_or_else(|| Utc.timestamp_opt(0, 0).single().unwrap_or_else(Utc::now))
}

fn file_stem_or(path: &Path, fallback: &str) -> String {
  path
    .file_stem()
    .and_then(|s| s.to_str())
    .unwrap_or(fallback)
    .to_string()
}
