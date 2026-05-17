use crate::model::{Source, UsageRecord};
use crate::sources::dump::{DumpRecord, DumpedSession};
use crate::sources::{read_jsonl, summarize_records, UsageSource};
use crate::text_count::{
  all_strings, json_serialized_or_string, message_content, nested_fields, stats_for_str, DumpSink, SpanSink, StatsSink,
  StringSink, TextSpan, TextStats, TokenSpan, TokenStatsSink, TokenUsageStats,
};
use anyhow::Result;
use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;
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

#[derive(Debug, Default, Clone, Copy)]
struct BytesUsage {
  input_bytes: u64,
  output_bytes: u64,
  reasoning_output_bytes: u64,
  total_bytes: u64,
}

impl BytesUsage {
  fn add_input(&mut self, bytes: u64) {
    self.input_bytes = self.input_bytes.saturating_add(bytes);
    self.total_bytes = self.total_bytes.saturating_add(bytes);
  }

  fn add_output(&mut self, bytes: u64) {
    self.output_bytes = self.output_bytes.saturating_add(bytes);
    self.total_bytes = self.total_bytes.saturating_add(bytes);
  }

  fn add_reasoning_output(&mut self, bytes: u64) {
    self.reasoning_output_bytes = self.reasoning_output_bytes.saturating_add(bytes);
    self.total_bytes = self.total_bytes.saturating_add(bytes);
  }

  fn add(&mut self, other: BytesUsage) {
    self.input_bytes = self.input_bytes.saturating_add(other.input_bytes);
    self.output_bytes = self.output_bytes.saturating_add(other.output_bytes);
    self.reasoning_output_bytes = self.reasoning_output_bytes.saturating_add(other.reasoning_output_bytes);
    self.total_bytes = self.total_bytes.saturating_add(other.total_bytes);
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
          summary = %summarize_records(&recs),
          "file summary"
        );
        out.extend(recs);
      }
    }
    Ok(out)
  }
}

struct Turn {
  ts: DateTime<Utc>,
  model: Option<String>,
  provider: Option<String>,
  usage: TokenUsage,
  bytes: BytesUsage,
  rounds: u64,
}

fn parse_rollout(path: &Path) -> Result<Option<Vec<UsageRecord>>> {
  // We emit one record per `token_count` event. Source data is one of:
  //   - `last_token_usage` (per-turn delta), preferred when present
  //   - `total_token_usage` (cumulative), in which case we emit deltas
  //     vs. the previous cumulative snapshot.
  let mut session_id: Option<String> = None;
  let mut cwd: Option<String> = None;
  let mut model: Option<String> = None;
  let mut provider: Option<String> = None;
  let mut session_ts: Option<DateTime<Utc>> = None;
  let mut turns: Vec<Turn> = Vec::new();
  let mut prev_total: Option<TokenUsage> = None;
  let mut pending_round: u64 = 0;
  let mut pending_bytes = BytesUsage::default();
  let mut last_ts: Option<DateTime<Utc>> = None;

  read_jsonl::<RolloutLine, _>(path, |parsed| {
    if let Some(ts_str) = &parsed.timestamp {
      if let Ok(dt) = DateTime::parse_from_rfc3339(ts_str) {
        let utc = dt.with_timezone(&Utc);
        last_ts = Some(utc);
        session_ts.get_or_insert(utc);
      }
    }

    match parsed.kind.as_deref() {
      Some("session_meta") => {
        apply_session_meta(&parsed, &mut session_id, &mut cwd, &mut model, &mut provider);
      }
      Some("event_msg") => {
        if let Some(payload) = &parsed.payload {
          if payload.get("type").and_then(|v| v.as_str()) == Some("token_count") {
            if let Some(usage) = extract_turn_usage(payload, &mut prev_total) {
              let ts = last_ts
                .or(session_ts)
                .unwrap_or_else(|| Utc.timestamp_opt(0, 0).single().unwrap_or_else(Utc::now));
              turns.push(Turn {
                ts,
                model: model.clone(),
                provider: provider.clone(),
                usage,
                bytes: std::mem::take(&mut pending_bytes),
                rounds: std::mem::take(&mut pending_round),
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
            model.get_or_insert_with(|| m.to_string());
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

  let sid = session_id.unwrap_or_else(|| file_stem_or(path, "unknown"));

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

fn apply_session_meta(
  parsed: &RolloutLine,
  session_id: &mut Option<String>,
  cwd: &mut Option<String>,
  model: &mut Option<String>,
  provider: &mut Option<String>,
) {
  if let Some(payload) = &parsed.payload {
    let meta = payload.get("meta").unwrap_or(payload);
    let str_field = |key: &str| meta.get(key).and_then(|v| v.as_str()).map(str::to_string);
    if session_id.is_none() {
      *session_id = str_field("id");
    }
    if cwd.is_none() {
      *cwd = str_field("cwd");
    }
    if model.is_none() {
      *model = str_field("model");
    }
    if provider.is_none() {
      *provider = str_field("model_provider").or_else(|| str_field("originator"));
    }
  }
  if session_id.is_none() {
    *session_id = parsed.id.clone();
  }
  if cwd.is_none() {
    *cwd = parsed.cwd.clone();
  }
  if model.is_none() {
    *model = parsed.model.clone();
  }
  if provider.is_none() {
    *provider = parsed.originator.clone();
  }
}

/// Extract the per-turn token usage from a `token_count` payload, updating
/// the cumulative snapshot when present.
fn extract_turn_usage(payload: &serde_json::Value, prev_total: &mut Option<TokenUsage>) -> Option<TokenUsage> {
  let info = payload.get("info").unwrap_or(payload);
  let last = info
    .get("last_token_usage")
    .and_then(|v| serde_json::from_value::<TokenUsage>(v.clone()).ok());
  let total = info
    .get("total_token_usage")
    .and_then(|v| serde_json::from_value::<TokenUsage>(v.clone()).ok());

  match (last, total) {
    (Some(usage), total) => {
      if let Some(t) = total {
        *prev_total = Some(t);
      }
      Some(usage)
    }
    (None, Some(total)) => {
      let delta = match prev_total {
        Some(prev) => sub_usage(&total, prev),
        None => total.clone(),
      };
      *prev_total = Some(total);
      Some(delta)
    }
    (None, None) => None,
  }
}

fn token_stats_from_usage(usage: &TokenUsage) -> TokenUsageStats {
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
      let Some(role) = message_role(payload) else {
        return ResponseItemAnalysis::empty();
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
    Some("function_call_output") => tool_result_spans(payload, call_id),
    Some("custom_tool_call") => {
      let mut stats = string_field_stats(payload, "name");
      stats.add(all_strings::<StatsSink>(payload.get("input")));
      let text = dump_tool_call_text(payload.get("name").and_then(|v| v.as_str()), payload.get("input"));
      vec![TextSpan::new("tool_call", text).with_stats(stats).with_call_id(call_id)]
    }
    Some("custom_tool_call_output") => tool_result_spans(payload, call_id),
    Some("reasoning") => match payload.get("encrypted_content").and_then(|v| v.as_str()) {
      Some(text) => vec![TextSpan::encrypted("reasoning", text.to_string(), stats_for_str(text))],
      None => return ResponseItemAnalysis::empty(),
    },
    _ => Vec::new(),
  };

  let mut bytes = BytesUsage::default();
  let mut dump = None;
  for span in spans {
    accumulate_span_bytes(&mut bytes, &span);
    if dump.is_none() {
      dump = DumpSink::record_from(span);
    }
  }
  ResponseItemAnalysis { bytes, dump }
}

impl ResponseItemAnalysis {
  fn empty() -> Self {
    Self {
      bytes: BytesUsage::default(),
      dump: None,
    }
  }
}

fn message_role(payload: &serde_json::Value) -> Option<&'static str> {
  match payload.get("role").and_then(|v| v.as_str())? {
    "user" => Some("user"),
    "assistant" => Some("assistant"),
    "developer" => Some("developer"),
    "system" => Some("system"),
    _ => None,
  }
}

fn tool_result_spans(payload: &serde_json::Value, call_id: Option<String>) -> Vec<TextSpan<'_>> {
  let stats = all_strings::<StatsSink>(payload.get("output"));
  let text = nested_fields::<StringSink>(payload.get("output"));
  vec![TextSpan::new("tool_call_result", text)
    .with_stats(stats)
    .with_call_id(call_id)]
}

fn accumulate_span_bytes(usage: &mut BytesUsage, span: &TextSpan<'_>) {
  let stats = span.resolved_stats();
  match span.role {
    "user" | "system" | "developer" | "tool_call_result" => usage.add_input(stats.bytes),
    "assistant" | "tool_call" => usage.add_output(stats.bytes),
    "reasoning" => usage.add_reasoning_output(stats.bytes),
    _ => {}
  }
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

  read_jsonl::<RolloutLine, _>(path, |parsed| match parsed.kind.as_deref() {
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
        if let Some(record) = analyze_response_item(payload).dump {
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
    session_id: session_id.unwrap_or_else(|| file_stem_or(path, "unknown")),
    records,
  }))
}

fn dump_tool_call_text(name: Option<&str>, body: Option<&serde_json::Value>) -> String {
  let name = name.unwrap_or("tool");
  let args = json_serialized_or_string::<StringSink>(body);
  if args.is_empty() {
    name.to_string()
  } else {
    format!("{name}: {args}")
  }
}

fn file_stem_or(path: &Path, fallback: &str) -> String {
  path
    .file_stem()
    .and_then(|s| s.to_str())
    .unwrap_or(fallback)
    .to_string()
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
