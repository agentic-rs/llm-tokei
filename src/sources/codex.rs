use crate::model::{Source, UsageRecord};
use crate::sources::dump::{DumpSink, DumpedSession};
use crate::sources::{read_jsonl, summarize_records, UsageSource};
use crate::text_count::{
  all_strings, json_serialized_or_string, message_content, nested_fields, BytesSink, SpanSink, StatsSink, StringSink,
  TextSpan, TextStats, TokenSpan, TokenStatsSink, TokenUsageStats,
};
use anyhow::Result;
use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;
use serde_json::Value;
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
    let mut builder = RecordBuilder::new(path);
    walk_rollout(path, &mut builder)?;
    Ok(builder.into_records())
  }

  pub fn dump_session_messages(path: &Path) -> Result<Option<DumpedSession>> {
    let mut builder = DumpBuilder::new(path);
    walk_rollout(path, &mut builder)?;
    Ok(builder.into_session())
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

// ---------------------------------------------------------------------------
// Walker + Visitor
// ---------------------------------------------------------------------------

/// Visitor over a Codex rollout JSONL file.
///
/// The walker parses each JSONL line as a full `Value` and passes the full
/// line/payload to visitors. High-level callbacks fire before low-level
/// response-item callbacks so consumers can choose their granularity.
trait RolloutVisitor {
  /// Called for every line with a top-level RFC3339 `timestamp` field.
  fn timestamp(&mut self, _ts: DateTime<Utc>) {}

  /// Called for `line.type == "session_meta"`; receives the full JSONL line.
  fn session_meta(&mut self, _line: &Value) {}

  /// Called for `line.type == "turn_context"`; receives `line.payload`.
  fn turn_context(&mut self, _payload: &Value) {}

  /// Called for `line.type == "event_msg" && line.payload.type == "token_count"`.
  /// Receives the parsed line timestamp and the full `line.payload` object.
  fn turn_end(&mut self, _ts: DateTime<Utc>, _payload: &Value) {}

  /// Called for every `line.type == "response_item"`, before low-level dispatch.
  fn response_item(&mut self, _payload: &Value) {}

  /// `response_item.payload.type == "message"`; full response-item payload.
  fn message(&mut self, _payload: &Value) {}

  /// `response_item.payload.type == "function_call"`; full response-item payload.
  fn function_call(&mut self, _payload: &Value) {}

  /// `response_item.payload.type == "function_call_output"`; full payload.
  fn function_call_output(&mut self, _payload: &Value) {}

  /// `response_item.payload.type == "custom_tool_call"`; full payload.
  fn custom_tool_call(&mut self, _payload: &Value) {}

  /// `response_item.payload.type == "custom_tool_call_output"`; full payload.
  fn custom_tool_call_output(&mut self, _payload: &Value) {}

  /// `response_item.payload.type == "reasoning"`; full payload.
  fn reasoning(&mut self, _payload: &Value) {}
}

fn walk_rollout<V: RolloutVisitor>(path: &Path, visitor: &mut V) -> Result<()> {
  read_jsonl::<Value, _>(path, |line| {
    let ts = parse_rfc3339(line.get("timestamp").and_then(|v| v.as_str()));
    if let Some(ts) = ts {
      visitor.timestamp(ts);
    }
    match line.get("type").and_then(|v| v.as_str()) {
      Some("session_meta") => visitor.session_meta(&line),
      Some("event_msg") => {
        if let Some(payload) = line.get("payload") {
          if payload.get("type").and_then(|v| v.as_str()) == Some("token_count") {
            let ts = ts.unwrap_or_else(epoch_utc);
            visitor.turn_end(ts, payload);
          }
        }
      }
      Some("turn_context") => {
        if let Some(payload) = line.get("payload") {
          visitor.turn_context(payload);
        }
      }
      Some("response_item") => {
        if let Some(payload) = line.get("payload") {
          visitor.response_item(payload);
          match payload.get("type").and_then(|v| v.as_str()) {
            Some("message") => visitor.message(payload),
            Some("function_call") => visitor.function_call(payload),
            Some("function_call_output") => visitor.function_call_output(payload),
            Some("custom_tool_call") => visitor.custom_tool_call(payload),
            Some("custom_tool_call_output") => visitor.custom_tool_call_output(payload),
            Some("reasoning") => visitor.reasoning(payload),
            _ => {}
          }
        }
      }
      _ => {}
    }
  })
}

fn parse_rfc3339(s: Option<&str>) -> Option<DateTime<Utc>> {
  DateTime::parse_from_rfc3339(s?).ok().map(|dt| dt.with_timezone(&Utc))
}

fn epoch_utc() -> DateTime<Utc> {
  Utc.timestamp_opt(0, 0).single().unwrap_or_else(Utc::now)
}

// ---------------------------------------------------------------------------
// Session metadata (shared by every visitor)
// ---------------------------------------------------------------------------

#[derive(Default)]
struct SessionMeta {
  session_id: Option<String>,
  forked_from_id: Option<String>,
  cwd: Option<String>,
  model: Option<String>,
  provider: Option<String>,
}

impl SessionMeta {
  fn apply(&mut self, line: &Value) {
    if let Some(payload) = line.get("payload") {
      let meta = payload.get("meta").unwrap_or(payload);
      let str_field = |key: &str| meta.get(key).and_then(|v| v.as_str()).map(str::to_string);
      if self.session_id.is_none() {
        self.session_id = str_field("id");
      }
      if self.forked_from_id.is_none() {
        self.forked_from_id = str_field("forked_from_id");
      }
      if self.cwd.is_none() {
        self.cwd = str_field("cwd");
      }
      if self.model.is_none() {
        self.model = str_field("model");
      }
      if self.provider.is_none() {
        self.provider = str_field("model_provider").or_else(|| str_field("originator"));
      }
    }
    if self.session_id.is_none() {
      self.session_id = line.get("id").and_then(|v| v.as_str()).map(str::to_string);
    }
    if self.cwd.is_none() {
      self.cwd = line.get("cwd").and_then(|v| v.as_str()).map(str::to_string);
    }
    if self.model.is_none() {
      self.model = line.get("model").and_then(|v| v.as_str()).map(str::to_string);
    }
    if self.provider.is_none() {
      self.provider = line.get("originator").and_then(|v| v.as_str()).map(str::to_string);
    }
  }

  fn resolved_session_id(&self, path: &Path) -> String {
    self.session_id.clone().unwrap_or_else(|| file_stem_or(path, "unknown"))
  }
}

// ---------------------------------------------------------------------------
// Parse visitor: builds UsageRecord per token_count event
// ---------------------------------------------------------------------------

struct RecordBuilder<'a> {
  path: &'a Path,
  meta: SessionMeta,
  session_ts: Option<DateTime<Utc>>,
  last_ts: Option<DateTime<Utc>>,
  prev_total: Option<TokenUsageStats>,
  pending_bytes: BytesSink,
  pending_rounds: u64,
  inherited_turn: bool,
  calls: Vec<Turn>,
}

struct Turn {
  ts: DateTime<Utc>,
  model: Option<String>,
  provider: Option<String>,
  tokens: TokenUsageStats,
  bytes: BytesSink,
  rounds: u64,
}

impl<'a> RecordBuilder<'a> {
  fn new(path: &'a Path) -> Self {
    Self {
      path,
      meta: SessionMeta::default(),
      session_ts: None,
      last_ts: None,
      prev_total: None,
      pending_bytes: BytesSink::default(),
      pending_rounds: 0,
      inherited_turn: false,
      calls: Vec::new(),
    }
  }

  fn into_records(mut self) -> Option<Vec<UsageRecord>> {
    if self.calls.is_empty() {
      return None;
    }
    // If no turn_context events were observed, attribute one round to the
    // first turn so totals match historical behavior.
    if self.calls.iter().all(|t| t.rounds == 0) {
      self.calls[0].rounds = 1;
    }
    let sid = self.meta.resolved_session_id(self.path);
    Some(
      self
        .calls
        .into_iter()
        .map(|t| UsageRecord {
          source: Source::Codex,
          session_id: sid.clone(),
          session_title: None,
          project_cwd: self.meta.cwd.clone(),
          project_name: None,
          provider: t.provider,
          model: t.model,
          ts: t.ts,
          prompt: t.tokens.prompt,
          completion: t.tokens.completion,
          input_bytes: t.bytes.input,
          output_bytes: t.bytes.output,
          input_estimated: false,
          output_estimated: false,
          input_bytes_estimated: true,
          output_bytes_estimated: true,
          reasoning: t.tokens.reasoning,
          cache_read: t.tokens.cache_read,
          cache_write: t.tokens.cache_write,
          total_direct: t.tokens.total,
          mode: None,
          agent: None,
          is_compaction: false,
          rounds: t.rounds,
          calls: 1,
          cost_embedded: None,
        })
        .collect(),
    )
  }
}

impl RolloutVisitor for RecordBuilder<'_> {
  fn timestamp(&mut self, ts: DateTime<Utc>) {
    self.last_ts = Some(ts);
    self.session_ts.get_or_insert(ts);
  }

  fn session_meta(&mut self, line: &Value) {
    self.meta.apply(line);
  }

  fn turn_end(&mut self, ts: DateTime<Utc>, payload: &Value) {
    let Some(tokens) = extract_turn_usage(payload, &mut self.prev_total) else {
      return;
    };
    if self.inherited_turn {
      self.pending_bytes.take();
      self.pending_rounds = 0;
      return;
    }
    let ts = self.last_ts.or(self.session_ts).unwrap_or(ts);
    self.calls.push(Turn {
      ts,
      model: self.meta.model.clone(),
      provider: self.meta.provider.clone(),
      tokens,
      bytes: self.pending_bytes.take(),
      rounds: std::mem::take(&mut self.pending_rounds),
    });
  }

  fn turn_context(&mut self, payload: &Value) {
    self.inherited_turn = is_inherited_fork_turn(&self.meta, payload);
    self.pending_rounds += 1;
    if let Some(m) = payload.get("model").and_then(|v| v.as_str()) {
      self.meta.model = Some(m.to_string());
    }
    if let Some(p) = payload.get("model_provider").and_then(|v| v.as_str()) {
      self.meta.provider = Some(p.to_string());
    }
  }

  fn response_item(&mut self, payload: &Value) {
    if let Some(m) = payload.get("model").and_then(|v| v.as_str()) {
      self.meta.model.get_or_insert_with(|| m.to_string());
    }
  }

  fn message(&mut self, payload: &Value) {
    visit_message(payload, &mut self.pending_bytes);
  }

  fn function_call(&mut self, payload: &Value) {
    visit_tool_call(payload, "name", "arguments", &mut self.pending_bytes);
  }

  fn function_call_output(&mut self, payload: &Value) {
    visit_tool_call_output(payload, &mut self.pending_bytes);
  }

  fn custom_tool_call(&mut self, payload: &Value) {
    visit_tool_call(payload, "name", "input", &mut self.pending_bytes);
  }

  fn custom_tool_call_output(&mut self, payload: &Value) {
    visit_tool_call_output(payload, &mut self.pending_bytes);
  }

  fn reasoning(&mut self, payload: &Value) {
    visit_reasoning(payload, &mut self.pending_bytes);
  }
}

/// Forked Codex rollouts begin with a replay of the parent thread. UUIDv7 turn
/// IDs preserve creation order, so turns older than the fork's own session ID
/// belong to that inherited replay and must not be counted again.
fn is_inherited_fork_turn(meta: &SessionMeta, payload: &Value) -> bool {
  let Some(session_id) = meta.session_id.as_deref().filter(|_| meta.forked_from_id.is_some()) else {
    return false;
  };
  let Some(turn_id) = payload.get("turn_id").and_then(Value::as_str) else {
    return false;
  };
  is_uuid_v7(session_id) && is_uuid_v7(turn_id) && turn_id < session_id
}

fn is_uuid_v7(value: &str) -> bool {
  value.len() == 36
    && value.as_bytes().get(14) == Some(&b'7')
    && value.bytes().enumerate().all(|(idx, byte)| match idx {
      8 | 13 | 18 | 23 => byte == b'-',
      _ => byte.is_ascii_hexdigit(),
    })
}

// ---------------------------------------------------------------------------
// Dump visitor: builds DumpedSession
// ---------------------------------------------------------------------------

struct DumpBuilder<'a> {
  path: &'a Path,
  meta: SessionMeta,
  sink: DumpSink,
}

impl<'a> DumpBuilder<'a> {
  fn new(path: &'a Path) -> Self {
    Self {
      path,
      meta: SessionMeta::default(),
      sink: DumpSink::default(),
    }
  }

  fn into_session(self) -> Option<DumpedSession> {
    if self.sink.records.is_empty() {
      return None;
    }
    Some(DumpedSession {
      session_id: self.meta.resolved_session_id(self.path),
      records: self.sink.records,
    })
  }
}

impl RolloutVisitor for DumpBuilder<'_> {
  fn session_meta(&mut self, line: &Value) {
    self.meta.apply(line);
  }

  fn message(&mut self, payload: &Value) {
    let mut once = OnceDump::new(&mut self.sink);
    visit_message(payload, &mut once);
  }

  fn function_call(&mut self, payload: &Value) {
    let mut once = OnceDump::new(&mut self.sink);
    visit_tool_call(payload, "name", "arguments", &mut once);
  }

  fn function_call_output(&mut self, payload: &Value) {
    let mut once = OnceDump::new(&mut self.sink);
    visit_tool_call_output(payload, &mut once);
  }

  fn custom_tool_call(&mut self, payload: &Value) {
    let mut once = OnceDump::new(&mut self.sink);
    visit_tool_call(payload, "name", "input", &mut once);
  }

  fn custom_tool_call_output(&mut self, payload: &Value) {
    let mut once = OnceDump::new(&mut self.sink);
    visit_tool_call_output(payload, &mut once);
  }

  fn reasoning(&mut self, payload: &Value) {
    let mut once = OnceDump::new(&mut self.sink);
    visit_reasoning(payload, &mut once);
  }
}

// ---------------------------------------------------------------------------
// Per-kind response-item extractors. Each writes to an arbitrary SpanSink
// (BytesSink for parse, OnceDump for dump, SpanStatsSink in tests).
// ---------------------------------------------------------------------------

fn visit_message<S: SpanSink>(payload: &Value, sink: &mut S) {
  let Some(role) = message_role(payload) else {
    return;
  };
  let call_id = payload.get("call_id").and_then(|v| v.as_str()).map(str::to_string);
  let text = message_content::<StringSink>(payload.get("content"));
  let stats = message_content::<StatsSink>(payload.get("content"));
  sink.text(TextSpan::new(role, text).with_stats(stats).with_call_id(call_id));
}

/// Extracts a tool-call span from `function_call` or `custom_tool_call`.
/// `name_field` and `body_field` select which JSON keys carry the tool name
/// and arguments/input respectively.
fn visit_tool_call<S: SpanSink>(payload: &Value, name_field: &str, body_field: &str, sink: &mut S) {
  let call_id = payload.get("call_id").and_then(|v| v.as_str()).map(str::to_string);
  let mut stats = string_field_stats(payload, name_field);
  stats.add(all_strings::<StatsSink>(payload.get(body_field)));
  let name = payload.get(name_field).and_then(|v| v.as_str()).unwrap_or("tool");
  let args = json_serialized_or_string::<StringSink>(payload.get(body_field));
  let text = if args.is_empty() {
    name.to_string()
  } else {
    format!("{name}: {args}")
  };
  sink.text(TextSpan::new("tool_call", text).with_stats(stats).with_call_id(call_id));
}

/// Extracts a tool-call-result span from `function_call_output` or
/// `custom_tool_call_output`.
fn visit_tool_call_output<S: SpanSink>(payload: &Value, sink: &mut S) {
  let call_id = payload.get("call_id").and_then(|v| v.as_str()).map(str::to_string);
  let stats = all_strings::<StatsSink>(payload.get("output"));
  let text = nested_fields::<StringSink>(payload.get("output"));
  sink.text(
    TextSpan::new("tool_call_result", text)
      .with_stats(stats)
      .with_call_id(call_id),
  );
}

fn visit_reasoning<S: SpanSink>(payload: &Value, sink: &mut S) {
  if let Some(text) = payload.get("encrypted_content").and_then(|v| v.as_str()) {
    let stats = TextStats::from_str(text);
    sink.text(TextSpan::encrypted("reasoning", text.to_string(), stats));
  }
}

/// SpanSink wrapper that forwards only the first non-empty span to the inner
/// DumpSink, matching the legacy semantics where dump emits at most one record
/// per response_item.
struct OnceDump<'a> {
  inner: &'a mut DumpSink,
  done: bool,
}

impl<'a> OnceDump<'a> {
  fn new(inner: &'a mut DumpSink) -> Self {
    Self { inner, done: false }
  }
}

impl SpanSink for OnceDump<'_> {
  fn text(&mut self, span: TextSpan<'_>) {
    if self.done {
      return;
    }
    if let Some(record) = DumpSink::record_from(span) {
      self.inner.records.push(record);
      self.done = true;
    }
  }
}

fn message_role(payload: &Value) -> Option<&'static str> {
  match payload.get("role").and_then(|v| v.as_str())? {
    "user" => Some("user"),
    "assistant" => Some("assistant"),
    "developer" => Some("developer"),
    "system" => Some("system"),
    _ => None,
  }
}

fn string_field_stats(value: &Value, field: &str) -> TextStats {
  value
    .get(field)
    .and_then(|v| v.as_str())
    .map(TextStats::from_str)
    .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Token-usage delta extraction
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone, Copy, Deserialize)]
struct RawTokenUsage {
  #[serde(default)]
  input_tokens: u64,
  #[serde(default)]
  cached_input_tokens: u64,
  #[serde(default)]
  output_tokens: u64,
  #[serde(default)]
  reasoning_output_tokens: u64,
  #[serde(default)]
  total_tokens: Option<u64>,
}

impl RawTokenUsage {
  fn into_stats(self) -> TokenUsageStats {
    let mut sink = TokenStatsSink::default();
    sink.token(TokenSpan::usage(
      self.input_tokens.saturating_sub(self.cached_input_tokens),
      self.output_tokens.saturating_sub(self.reasoning_output_tokens),
      self.reasoning_output_tokens,
      self.cached_input_tokens,
      0,
      self.total_tokens,
    ));
    sink.usage.total = self.total_tokens;
    sink.usage
  }
}

/// Returns the per-turn delta as `TokenUsageStats`, updating the cumulative
/// snapshot when only `total_token_usage` is present. The `.total` field
/// carries the source-reported direct total when available.
fn extract_turn_usage(payload: &Value, prev_total: &mut Option<TokenUsageStats>) -> Option<TokenUsageStats> {
  let info = payload.get("info").unwrap_or(payload);
  let last = info
    .get("last_token_usage")
    .and_then(|v| serde_json::from_value::<RawTokenUsage>(v.clone()).ok());
  let total = info
    .get("total_token_usage")
    .and_then(|v| serde_json::from_value::<RawTokenUsage>(v.clone()).ok())
    .map(RawTokenUsage::into_stats);

  match (last, total) {
    (Some(usage), total) => {
      if let Some(t) = total {
        *prev_total = Some(t);
      }
      Some(usage.into_stats())
    }
    (None, Some(total)) => {
      let delta = match prev_total {
        Some(prev) => total.sub(*prev),
        None => total,
      };
      *prev_total = Some(total);
      Some(delta)
    }
    (None, None) => None,
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
  use crate::text_count::SpanStatsSink;

  fn parse_fixture() -> Vec<UsageRecord> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
      .join("tests/fixtures/codex/sessions/2025/01/02/rollout-2025-01-02T10-00-00-test.jsonl");
    CodexSource::parse_file(&path).expect("parse fixture").expect("records")
  }

  #[test]
  fn response_item_bytes_are_attached_to_each_pushed_turn() {
    let records = parse_fixture();

    assert_eq!(records.len(), 4);
    assert_eq!(
      records.iter().map(|r| r.input_bytes).collect::<Vec<_>>(),
      vec![21, 4, 7, 5]
    );
    assert_eq!(
      records.iter().map(|r| r.output_bytes).collect::<Vec<_>>(),
      vec![20, 4, 5, 5]
    );
    // `prompt` stores the uncached portion (input_tokens - cached_input_tokens).
    assert_eq!(
      records.iter().map(|r| r.prompt).collect::<Vec<_>>(),
      vec![60, 60, 120, 60]
    );
    assert_eq!(
      records.iter().map(|r| r.completion).collect::<Vec<_>>(),
      vec![40, 30, 70, 30]
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
    let mut bytes = BytesSink::default();
    visit_reasoning(&payload, &mut bytes);

    assert_eq!(bytes.input, 0);
    assert_eq!(bytes.output, 0);
    assert_eq!(bytes.reasoning, 6);
    assert_eq!(bytes.total(), 6);
  }

  #[test]
  fn response_item_can_drive_stats_sink_for_aggregate_text() {
    let payload = serde_json::json!({
      "type": "message",
      "role": "assistant",
      "content": [{ "type": "output_text", "text": "hello world" }]
    });
    let mut stats = SpanStatsSink::default();
    visit_message(&payload, &mut stats);
    assert_eq!(stats.stats.bytes, "hello world".len() as u64);
  }

  #[test]
  fn forked_rollout_ignores_parent_replay_but_keeps_new_turns() {
    let path = Path::new("forked.jsonl");
    let mut builder = RecordBuilder::new(path);
    builder.session_meta(&serde_json::json!({
      "payload": {
        "id": "019f57cd-7555-7292-a6cc-540fc0df1778",
        "forked_from_id": "019f4a9e-3a88-7a00-9989-2d12dda99487"
      }
    }));

    builder.turn_context(&serde_json::json!({
      "turn_id": "019f4a9e-7b5c-71c1-b7a5-7b8c6805bc6e",
      "model": "gpt-5.6-sol"
    }));
    builder.message(&serde_json::json!({
      "type": "message",
      "role": "user",
      "content": [{ "type": "input_text", "text": "inherited" }]
    }));
    builder.turn_end(
      epoch_utc(),
      &serde_json::json!({
        "info": {
          "last_token_usage": {
            "input_tokens": 100,
            "cached_input_tokens": 80,
            "output_tokens": 10,
            "reasoning_output_tokens": 2,
            "total_tokens": 110
          }
        }
      }),
    );

    builder.turn_context(&serde_json::json!({
      "turn_id": "019f57cd-7c10-7aa1-b465-056defebbe28",
      "model": "gpt-5.6-sol"
    }));
    builder.message(&serde_json::json!({
      "type": "message",
      "role": "user",
      "content": [{ "type": "input_text", "text": "new" }]
    }));
    builder.turn_end(
      epoch_utc(),
      &serde_json::json!({
        "info": {
          "last_token_usage": {
            "input_tokens": 40,
            "cached_input_tokens": 30,
            "output_tokens": 5,
            "reasoning_output_tokens": 1,
            "total_tokens": 45
          }
        }
      }),
    );

    let records = builder.into_records().expect("new fork turn should produce a record");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].prompt, 10);
    assert_eq!(records[0].cache_read, 30);
    assert_eq!(records[0].completion, 4);
    assert_eq!(records[0].reasoning, 1);
    assert_eq!(records[0].input_bytes, 3);
    assert_eq!(records[0].rounds, 1);
  }
}
