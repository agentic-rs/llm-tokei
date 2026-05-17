use crate::model::{Source, UsageRecord};
use crate::sources::dump::{DumpRecord, DumpedSession};
use crate::sources::UsageSource;
use crate::text_count::{
  all_strings, json_serialized_or_string, message_content, nested_fields, stats_for_str, DumpSink, SpanSink, StatsSink,
  StringSink, TextSpan, TextStats, TokenSpan, TokenStatsSink,
};
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

  pub fn dump_session_messages(path: &Path) -> Result<Option<DumpedSession>> {
    dump_rollout(path)
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

#[derive(Debug, Default, Clone)]
struct BytesUsage {
  input_bytes: u64,
  output_bytes: u64,
  reasoning_output_bytes: u64,
  total_bytes: u64,
}

impl BytesUsage {
  fn add_input(&mut self, bytes: u64) {
    self.input_bytes = self.input_bytes.saturating_add(bytes);
    self.recompute_total();
  }

  fn add_output(&mut self, bytes: u64) {
    self.output_bytes = self.output_bytes.saturating_add(bytes);
    self.recompute_total();
  }

  fn add_reasoning_output(&mut self, bytes: u64) {
    self.reasoning_output_bytes = self.reasoning_output_bytes.saturating_add(bytes);
    self.recompute_total();
  }

  fn add(&mut self, other: BytesUsage) {
    self.input_bytes = self.input_bytes.saturating_add(other.input_bytes);
    self.output_bytes = self.output_bytes.saturating_add(other.output_bytes);
    self.reasoning_output_bytes = self.reasoning_output_bytes.saturating_add(other.reasoning_output_bytes);
    self.recompute_total();
  }

  fn recompute_total(&mut self) {
    self.total_bytes = self
      .input_bytes
      .saturating_add(self.output_bytes)
      .saturating_add(self.reasoning_output_bytes);
  }
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
    bytes: BytesUsage,
    rounds: u64,
  }
  let mut turns: Vec<Turn> = Vec::new();
  let mut prev_total: Option<TokenUsage> = None;
  let mut pending_round: u64 = 0; // turn_context events that haven't been attributed yet
  let mut pending_bytes = BytesUsage::default();
  let mut last_ts: Option<DateTime<Utc>> = None;

  read_rollout_lines(path, |parsed| {
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
              let bytes = std::mem::take(&mut pending_bytes);
              turns.push(Turn {
                ts,
                model: model.clone(),
                provider: provider.clone(),
                usage,
                bytes,
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
      Some("response_item") => {
        if let Some(payload) = &parsed.payload {
          if let Some(m) = payload.get("model").and_then(|v| v.as_str()) {
            if model.is_none() {
              model = Some(m.to_string());
            }
          }
          pending_bytes.add(response_item_bytes(payload));
        }
      }
      _ => {}
    }
  })?;

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
    .map(|t| {
      let tokens = token_stats_from_usage(&t.usage);
      UsageRecord {
        source: Source::Codex,
        session_id: sid.clone(),
        session_title: None,
        project_cwd: cwd.clone(),
        project_name: None,
        provider: t.provider,
        model: t.model,
        ts: t.ts,
        input: tokens.input,
        output: tokens.output,
        input_bytes: t.bytes.input_bytes,
        output_bytes: t.bytes.output_bytes,
        input_estimated: false,
        output_estimated: false,
        input_bytes_estimated: true,
        output_bytes_estimated: true,
        reasoning: tokens.reasoning,
        cache_read: tokens.cache_read,
        cache_write: tokens.cache_write,
        mode: None,
        agent: None,
        is_compaction: false,
        rounds: t.rounds,
        turns: 1,
        cost_embedded: None,
      }
    })
    .collect();

  Ok(Some(records))
}

fn token_stats_from_usage(usage: &TokenUsage) -> crate::text_count::TokenUsageStats {
  let mut sink = TokenStatsSink::default();
  sink.token(TokenSpan::usage(
    usage.input_tokens.saturating_sub(usage.cached_input_tokens),
    usage.output_tokens,
    usage.reasoning_output_tokens,
    usage.cached_input_tokens,
    0,
  ));
  sink.usage
}

fn read_rollout_lines(path: &Path, mut visit: impl FnMut(RolloutLine)) -> Result<()> {
  let f = File::open(path)?;
  let reader = BufReader::new(f);

  for line in reader.lines() {
    let line = match line {
      Ok(l) => l,
      Err(_) => continue,
    };
    if line.trim().is_empty() {
      continue;
    }
    if let Ok(parsed) = serde_json::from_str(&line) {
      visit(parsed);
    }
  }

  Ok(())
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

fn response_item_bytes(payload: &serde_json::Value) -> BytesUsage {
  analyze_response_item(payload).bytes
}

struct ResponseItemAnalysis {
  bytes: BytesUsage,
  dump: Option<DumpRecord>,
}

fn analyze_response_item(payload: &serde_json::Value) -> ResponseItemAnalysis {
  let call_id = payload.get("call_id").and_then(|v| v.as_str()).map(str::to_string);
  let spans = match payload.get("type").and_then(|v| v.as_str()) {
    Some("message") => {
      let role = match payload.get("role").and_then(|v| v.as_str()) {
        Some("user") => "user",
        Some("assistant") => "assistant",
        Some("developer") => "developer",
        Some("system") => "system",
        _ => {
          return ResponseItemAnalysis {
            bytes: BytesUsage::default(),
            dump: None,
          }
        }
      };
      let text = message_content::<StringSink>(payload.get("content"));
      let stats = message_content::<StatsSink>(payload.get("content"));
      vec![TextSpan::new(role, text).with_stats(stats).with_call_id(call_id)]
    }
    Some("function_call") => {
      let mut stats = string_field_stats(payload, "name");
      stats.add(string_field_stats(payload, "arguments"));
      let text = dump_tool_call_text(payload.get("name").and_then(|v| v.as_str()), payload.get("arguments"));
      vec![TextSpan::new("tool_call", text).with_stats(stats).with_call_id(call_id)]
    }
    Some("function_call_output") => {
      let stats = all_strings::<StatsSink>(payload.get("output"));
      let text = nested_fields::<StringSink>(payload.get("output"));
      vec![TextSpan::new("tool_call_result", text)
        .with_stats(stats)
        .with_call_id(call_id)]
    }
    Some("custom_tool_call") => {
      let mut stats = string_field_stats(payload, "name");
      stats.add(all_strings::<StatsSink>(payload.get("input")));
      let text = dump_tool_call_text(payload.get("name").and_then(|v| v.as_str()), payload.get("input"));
      vec![TextSpan::new("tool_call", text).with_stats(stats).with_call_id(call_id)]
    }
    Some("custom_tool_call_output") => {
      let stats = all_strings::<StatsSink>(payload.get("output"));
      let text = nested_fields::<StringSink>(payload.get("output"));
      vec![TextSpan::new("tool_call_result", text)
        .with_stats(stats)
        .with_call_id(call_id)]
    }
    Some("reasoning") => {
      let Some(encrypted_text) = payload.get("encrypted_content").and_then(|v| v.as_str()) else {
        return ResponseItemAnalysis {
          bytes: BytesUsage::default(),
          dump: None,
        };
      };
      vec![TextSpan::encrypted(
        "reasoning",
        encrypted_text.to_string(),
        stats_for_str(encrypted_text),
      )]
    }
    _ => Vec::new(),
  };

  let bytes = response_item_bytes_from_spans(&spans);
  let mut dump_sink = DumpSink::default();
  for span in spans {
    dump_sink.text(span);
  }
  ResponseItemAnalysis {
    bytes,
    dump: dump_sink.records.into_iter().next(),
  }
}

fn response_item_bytes_from_spans(spans: &[TextSpan<'_>]) -> BytesUsage {
  let mut usage = BytesUsage::default();
  for span in spans {
    let stats = span.stats.unwrap_or_else(|| stats_for_str(&span.text));
    match span.role {
      "user" | "system" | "developer" | "tool_call_result" => usage.add_input(stats.bytes),
      "assistant" | "tool_call" => usage.add_output(stats.bytes),
      "reasoning" => usage.add_reasoning_output(stats.bytes),
      _ => {}
    }
  }
  usage
}

fn string_field_stats(value: &serde_json::Value, field: &str) -> TextStats {
  value
    .get(field)
    .and_then(|v| v.as_str())
    .map(stats_for_str)
    .unwrap_or_default()
}

fn dump_rollout(path: &Path) -> Result<Option<DumpedSession>> {
  let mut session_id: Option<String> = None;
  let mut records = Vec::new();

  read_rollout_lines(path, |parsed| match parsed.kind.as_deref() {
    Some("session_meta") if session_id.is_none() => {
      session_id = parsed
        .payload
        .as_ref()
        .and_then(|payload| payload.get("meta").unwrap_or(payload).get("id"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or(parsed.id.clone());
    }
    Some("response_item") => {
      if let Some(payload) = parsed.payload.as_ref() {
        if let Some(record) = dump_record_from_response_item(payload) {
          records.push(record);
        }
      }
    }
    _ => {}
  })?;

  if records.is_empty() {
    return Ok(None);
  }

  Ok(Some(DumpedSession {
    session_id: session_id.unwrap_or_else(|| {
      path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string()
    }),
    records,
  }))
}

fn dump_record_from_response_item(payload: &serde_json::Value) -> Option<DumpRecord> {
  analyze_response_item(payload).dump
}

fn dump_tool_call_text(name: Option<&str>, body: Option<&serde_json::Value>) -> String {
  let name = name.unwrap_or("tool");
  let args = dump_tool_body(body);
  if args.is_empty() {
    name.to_string()
  } else {
    format!("{name}: {args}")
  }
}

fn dump_tool_body(value: Option<&serde_json::Value>) -> String {
  json_serialized_or_string::<StringSink>(value)
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

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn response_item_bytes_are_attached_to_each_pushed_turn() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
      .join("tests/fixtures/codex/sessions/2025/01/02/rollout-2025-01-02T10-00-00-test.jsonl");
    let records = parse_rollout(&path).expect("parse fixture").expect("records");

    assert_eq!(records.len(), 4);
    assert_eq!(
      records.iter().map(|r| r.input_bytes).collect::<Vec<_>>(),
      vec![21, 4, 7, 5]
    );
    assert_eq!(
      records.iter().map(|r| r.output_bytes).collect::<Vec<_>>(),
      vec![20, 4, 5, 5]
    );
    // `input` stores the uncached portion (input_tokens - cached_input_tokens).
    assert_eq!(
      records.iter().map(|r| r.input).collect::<Vec<_>>(),
      vec![60, 60, 120, 60]
    );
    assert_eq!(
      records.iter().map(|r| r.output).collect::<Vec<_>>(),
      vec![50, 40, 90, 40]
    );
    assert_eq!(
      records.iter().map(|r| r.cache_read).collect::<Vec<_>>(),
      vec![40, 40, 80, 40]
    );
  }

  #[test]
  fn reasoning_response_item_bytes_are_separate_from_output_bytes() {
    let payload = serde_json::json!({
      "type": "reasoning",
      "encrypted_content": "sealed",
      "summary": [{ "type": "summary_text", "text": "ignored" }]
    });
    let usage = response_item_bytes(&payload);

    assert_eq!(usage.input_bytes, 0);
    assert_eq!(usage.output_bytes, 0);
    assert_eq!(usage.reasoning_output_bytes, 6);
    assert_eq!(usage.total_bytes, 6);
  }
}
