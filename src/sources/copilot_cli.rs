use crate::model::{ParsedUsageFile, ParsedUsageRecord, Source, UsageRecord};
use crate::sources::copilot_shutdown::{
  normalize_copilot_model, records_from_shutdown_model_metrics, timestamp_from_event, ShutdownRecordArgs,
};
use crate::sources::dump::{DumpRecord, DumpSink, DumpedSession};
use crate::sources::{read_jsonl_collect, read_jsonl_collect_with_status, summarize_records, JsonlEntry, UsageSource};
use crate::text_count::{
  all_strings, json_serialized_or_string, SpanSink, StatsSink, StringSink, TextSpan, TokenSpan, TokenStatsSink,
  TokenUsageStats,
};
use anyhow::Result;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use tracing::debug;
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
    let events = read_jsonl_collect::<Value>(path)?;
    let records = parse_event_records(path, &events);
    Ok(if records.is_empty() { None } else { Some(records) })
  }

  /// Parse a cacheable event stream into records with stable source-event
  /// identities. Invalid JSONL entries retain valid records but mark the
  /// result incomplete, so cache reconciliation cannot remove missing rows.
  pub fn parse_cache_file(path: &Path) -> Result<ParsedUsageFile> {
    let jsonl = read_jsonl_collect_with_status::<Value>(path)?;
    let complete = jsonl.complete;
    let records = parse_cache_event_records(path, &jsonl.entries);
    Ok(ParsedUsageFile::new(complete, records))
  }

  pub fn dump_session_messages(path: &Path) -> Result<Option<DumpedSession>> {
    let events = read_jsonl_collect::<Value>(path)?;
    if events.is_empty() {
      return Ok(None);
    }
    let session_id = find_session_id(&events).unwrap_or_else(|| fallback_session_id(path));
    let mut builder = DumpBuilder::new(session_id);
    walk_events(&events, &mut builder);
    Ok(builder.into_session())
  }
}

impl UsageSource for CopilotCliSource {
  fn name(&self) -> &'static str {
    "copilot-cli"
  }

  fn collect(&self) -> Result<Vec<UsageRecord>> {
    let mut out = Vec::new();
    for path in self.discover_files() {
      debug!(source = "copilot-cli", file = %path.display(), "processing file");
      if let Ok(Some(recs)) = Self::parse_file(&path) {
        debug!(
          source = "copilot-cli",
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

fn parse_event_records(path: &Path, events: &[Value]) -> Vec<UsageRecord> {
  if events.is_empty() {
    return Vec::new();
  }
  let session_id = find_session_id(events);

  // Always walk events to estimate bytes and count rounds/calls, even when
  // shutdown metrics provide the exact token totals.
  let mut bytes_collector = BytesCollector::default();
  walk_events(events, &mut bytes_collector);

  let mut shutdown = shutdown_records(path, events, session_id.clone());
  if !shutdown.is_empty() {
    apply_shutdown_byte_estimates(&mut shutdown, &bytes_collector);
    return shutdown;
  }

  let mut builder = RecordBuilder::new(path, session_id);
  walk_events(events, &mut builder);
  builder.into_records()
}

fn parse_cache_event_records(path: &Path, entries: &[JsonlEntry<Value>]) -> Vec<ParsedUsageRecord> {
  if entries.is_empty() {
    return Vec::new();
  }

  let session_id = find_session_id(entries.iter().map(|entry| &entry.value));
  let mut bytes_collector = BytesCollector::default();
  walk_events(entries.iter().map(|entry| &entry.value), &mut bytes_collector);

  let mut shutdown_records = Vec::new();
  let mut shutdown_origin_keys = Vec::new();
  let mut origin_key_counts = HashMap::new();
  for entry in entries {
    let mut event_records = records_from_shutdown_model_metrics(ShutdownRecordArgs {
      source: Source::CopilotCli,
      source_path: path,
      session_id: session_id.clone(),
      project_cwd: None,
      project_name: None,
      event: &entry.value,
    });
    event_records.sort_by(|a, b| a.provider.cmp(&b.provider).then_with(|| a.model.cmp(&b.model)));
    for record in event_records {
      let provider = record.provider.as_deref().unwrap_or("unknown");
      let model = record.model.as_deref().unwrap_or("unknown");
      let origin_key = unique_event_origin_key(
        format!("{}:shutdown:provider:{provider}:model:{model}", event_origin_key(entry)),
        &mut origin_key_counts,
      );
      shutdown_origin_keys.push(origin_key);
      shutdown_records.push(record);
    }
  }

  if !shutdown_records.is_empty() {
    apply_shutdown_byte_estimates(&mut shutdown_records, &bytes_collector);
    return shutdown_origin_keys
      .into_iter()
      .zip(shutdown_records)
      .map(|(origin_key, record)| ParsedUsageRecord { origin_key, record })
      .collect();
  }

  let mut builder = RecordBuilder::new(path, session_id);
  walk_events(entries.iter().map(|entry| &entry.value), &mut builder);
  let records = builder.into_records();
  let origin_keys = fallback_record_origin_keys(entries);
  debug_assert_eq!(records.len(), origin_keys.len());
  records
    .into_iter()
    .zip(origin_keys)
    .map(|(record, origin_key)| ParsedUsageRecord { origin_key, record })
    .collect()
}

fn shutdown_records(path: &Path, events: &[Value], session_id: Option<String>) -> Vec<UsageRecord> {
  events
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
    .collect()
}

fn apply_shutdown_byte_estimates(records: &mut [UsageRecord], bytes_collector: &BytesCollector) {
  let token_total: u64 = records
    .iter()
    .map(|record| {
      record
        .prompt
        .saturating_add(record.completion)
        .saturating_add(record.reasoning)
    })
    .sum();
  for record in records {
    if token_total > 0 {
      let share = record
        .prompt
        .saturating_add(record.completion)
        .saturating_add(record.reasoning) as f64
        / token_total as f64;
      record.input_bytes = (bytes_collector.input_bytes as f64 * share).round() as u64;
      record.output_bytes = (bytes_collector.output_bytes as f64 * share).round() as u64;
    }
    record.rounds = bytes_collector.rounds;
    record.calls = bytes_collector.calls;
  }
}

fn fallback_record_origin_keys(entries: &[JsonlEntry<Value>]) -> Vec<String> {
  let mut origin_keys = Vec::new();
  let mut counts = HashMap::new();
  for entry in entries {
    let slot = match entry.value.get("type").and_then(|value| value.as_str()) {
      Some("assistant.message") => Some("assistant"),
      Some("session.compaction_complete") if entry.value.pointer("/data/compactionTokensUsed").is_some() => {
        Some("compaction")
      }
      _ => None,
    };
    if let Some(slot) = slot {
      origin_keys.push(unique_event_origin_key(
        format!("{}:{slot}", event_origin_key(entry)),
        &mut counts,
      ));
    }
  }
  origin_keys
}

fn event_origin_key(entry: &JsonlEntry<Value>) -> String {
  let event = &entry.value;
  let id = event
    .get("id")
    .and_then(|value| value.as_str())
    .filter(|id| !id.is_empty())
    .map(|id| format!("event:{id}"))
    .or_else(|| {
      event
        .pointer("/data/messageId")
        .and_then(|value| value.as_str())
        .filter(|id| !id.is_empty())
        .map(|id| format!("message:{id}"))
    });
  id.unwrap_or_else(|| format!("offset:{}", entry.position.byte_offset))
}

fn unique_event_origin_key(base: String, counts: &mut HashMap<String, usize>) -> String {
  let count = counts.entry(base.clone()).or_default();
  let origin_key = if *count == 0 {
    base
  } else {
    format!("{base}:duplicate:{count}")
  };
  *count += 1;
  origin_key
}

// ---------------------------------------------------------------------------
// Walker + Visitor
// ---------------------------------------------------------------------------

/// Visitor over a Copilot CLI `events.jsonl` stream.
///
/// Every callback that corresponds to an event receives the full event object
/// so consumers can inspect fields beyond the common ones used here. High-level
/// `assistant_message` fires before low-level `assistant_text`/`tool_request`.
trait EventsVisitor {
  /// Called for every event with an RFC3339 `timestamp` field.
  fn timestamp(&mut self, _ts: chrono::DateTime<chrono::Utc>) {}

  /// `event.type == "session.start"`; full event with `data.sessionId`.
  fn session_start(&mut self, _event: &Value) {}

  /// `event.type == "session.model_change"`; full event with `data.newModel`.
  fn model_change(&mut self, _event: &Value) {}

  /// `event.type == "system.message"`; full event with `data.content`.
  fn system_message(&mut self, _event: &Value) {}

  /// `event.type == "user.message"`; full event with `data.content`.
  fn user_message(&mut self, _event: &Value) {}

  /// `event.type == "assistant.message"`; full event, before low-level callbacks.
  fn assistant_message(&mut self, _event: &Value) {}

  /// Low-level assistant text from `assistant.message`; receives the full event.
  fn assistant_text(&mut self, _event: &Value) {}

  /// Low-level tool request from `assistant.message.data.toolRequests[]`.
  /// Receives the full assistant event and the full request object.
  fn tool_request(&mut self, _event: &Value, _request: &Value) {}

  /// `event.type == "tool.execution_start"`; full event with `data.toolName`,
  /// `data.arguments`, and `data.toolCallId`.
  fn tool_execution_start(&mut self, _event: &Value) {}

  /// `event.type == "tool.execution_complete"`; full event with result/error.
  fn tool_execution_complete(&mut self, _event: &Value) {}

  /// `event.type == "session.compaction_complete"`; full event with
  /// `data.compactionTokensUsed`.
  fn compaction_complete(&mut self, _event: &Value) {}
}

fn walk_events<'a, V, I>(events: I, visitor: &mut V)
where
  V: EventsVisitor,
  I: IntoIterator<Item = &'a Value>,
{
  for event in events {
    if let Some(ts) = timestamp_from_event_opt(event) {
      visitor.timestamp(ts);
    }
    match event.get("type").and_then(|v| v.as_str()) {
      Some("session.start") => visitor.session_start(event),
      Some("session.model_change") => visitor.model_change(event),
      Some("system.message") => visitor.system_message(event),
      Some("user.message") => visitor.user_message(event),
      Some("assistant.message") => {
        visitor.assistant_message(event);
        if event.pointer("/data/content").is_some() {
          visitor.assistant_text(event);
        }
        if let Some(tool_requests) = event.pointer("/data/toolRequests").and_then(|v| v.as_array()) {
          for request in tool_requests {
            visitor.tool_request(event, request);
          }
        }
      }
      Some("tool.execution_start") => visitor.tool_execution_start(event),
      Some("tool.execution_complete") => visitor.tool_execution_complete(event),
      Some("session.compaction_complete") => visitor.compaction_complete(event),
      _ => {}
    }
  }
}

fn timestamp_from_event_opt(event: &Value) -> Option<chrono::DateTime<chrono::Utc>> {
  event
    .get("timestamp")
    .and_then(|v| v.as_str())
    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
    .map(|dt| dt.with_timezone(&chrono::Utc))
}

fn find_session_id<'a, I>(events: I) -> Option<String>
where
  I: IntoIterator<Item = &'a Value>,
{
  events.into_iter().find_map(|event| {
    if event.get("type").and_then(|v| v.as_str()) == Some("session.start") {
      event
        .pointer("/data/sessionId")
        .and_then(|v| v.as_str())
        .map(str::to_string)
    } else {
      None
    }
  })
}

/// Lightweight visitor that only accumulates input/output byte estimates
/// across the full event stream, used to fill `input_bytes`/`output_bytes`
/// on shutdown records.
#[derive(Default)]
struct BytesCollector {
  input_bytes: u64,
  output_bytes: u64,
  pending_input_bytes: u64,
  rounds: u64,
  calls: u64,
}

impl EventsVisitor for BytesCollector {
  fn system_message(&mut self, event: &Value) {
    self.pending_input_bytes += rough_bytes(event.get("data").unwrap_or(&Value::Null));
  }

  fn user_message(&mut self, event: &Value) {
    self.pending_input_bytes += rough_bytes(event.get("data").unwrap_or(&Value::Null));
    self.rounds += 1;
  }

  fn tool_execution_complete(&mut self, event: &Value) {
    self.pending_input_bytes += rough_bytes(event.get("data").unwrap_or(&Value::Null));
  }

  fn assistant_message(&mut self, event: &Value) {
    let content = event.pointer("/data/content").unwrap_or(&Value::Null);
    let tool_requests = event.pointer("/data/toolRequests").unwrap_or(&Value::Null);
    self.input_bytes += self.pending_input_bytes;
    self.output_bytes += rough_bytes(content) + rough_bytes(tool_requests);
    self.pending_input_bytes = rough_bytes(content) + rough_bytes(tool_requests);
    self.calls += 1;
  }

  fn compaction_complete(&mut self, _event: &Value) {
    self.calls += 1;
  }
}

// ---------------------------------------------------------------------------
// Parse visitor: estimates per-turn usage
// ---------------------------------------------------------------------------

struct RecordBuilder<'a> {
  path: &'a Path,
  session_id: Option<String>,
  current_model: String,
  pending_user: u64,
  pending_input: u64,
  pending_input_bytes: u64,
  records: Vec<UsageRecord>,
}

impl<'a> RecordBuilder<'a> {
  fn new(path: &'a Path, session_id: Option<String>) -> Self {
    Self {
      path,
      session_id,
      current_model: FALLBACK_MODEL.to_string(),
      pending_user: 0,
      pending_input: 0,
      pending_input_bytes: 0,
      records: Vec::new(),
    }
  }

  fn into_records(self) -> Vec<UsageRecord> {
    self.records
  }

  fn resolved_session_id(&self) -> String {
    self
      .session_id
      .clone()
      .unwrap_or_else(|| fallback_session_id(self.path))
  }

  fn add_pending(&mut self, data: &Value) {
    self.pending_input += rough_tokens(data);
    self.pending_input_bytes += rough_bytes(data);
  }
}

impl EventsVisitor for RecordBuilder<'_> {
  fn model_change(&mut self, event: &Value) {
    if let Some(new_model) = event.pointer("/data/newModel").and_then(|v| v.as_str()) {
      self.current_model = normalize_copilot_model(new_model.to_string()).1;
    }
  }

  fn system_message(&mut self, event: &Value) {
    self.add_pending(event.get("data").unwrap_or(&Value::Null));
  }

  fn user_message(&mut self, event: &Value) {
    self.pending_user += 1;
    self.add_pending(event.get("data").unwrap_or(&Value::Null));
  }

  fn tool_execution_complete(&mut self, event: &Value) {
    self.add_pending(event.get("data").unwrap_or(&Value::Null));
  }

  fn assistant_message(&mut self, event: &Value) {
    let (provider, model) = normalize_copilot_model(self.current_model.clone());
    let output_exact = event.pointer("/data/outputTokens").and_then(|v| v.as_u64());
    let content = event.pointer("/data/content").unwrap_or(&Value::Null);
    let tool_requests = event.pointer("/data/toolRequests").unwrap_or(&Value::Null);
    let output_estimated_tokens = rough_tokens(content) + rough_tokens(tool_requests);
    let output_estimated_bytes = rough_bytes(content) + rough_bytes(tool_requests);
    let sid = self.resolved_session_id();
    self.records.push(UsageRecord {
      source: Source::CopilotCli,
      session_id: sid,
      session_kind: crate::model::SessionKind::Root,
      parent_session_id: None,
      session_title: None,
      project_cwd: None,
      project_name: None,
      provider: Some(provider),
      model: Some(model),
      ts: timestamp_from_event(event),
      prompt: self.pending_input,
      completion: output_exact.unwrap_or(output_estimated_tokens),
      input_bytes: self.pending_input_bytes,
      output_bytes: output_estimated_bytes,
      input_estimated: true,
      output_estimated: output_exact.is_none(),
      input_bytes_estimated: true,
      output_bytes_estimated: true,
      reasoning: 0,
      cache_read: 0,
      cache_write: 0,
      total_direct: None,
      mode: None,
      agent: None,
      is_compaction: false,
      rounds: if self.pending_user > 0 { 1 } else { 0 },
      calls: 1,
      cost_embedded: None,
    });
    self.pending_input = rough_tokens(content) + rough_tokens(tool_requests);
    self.pending_input_bytes = rough_bytes(content) + rough_bytes(tool_requests);
  }

  fn compaction_complete(&mut self, event: &Value) {
    let Some(usage) = event.pointer("/data/compactionTokensUsed") else {
      return;
    };
    let model_raw = usage
      .get("model")
      .and_then(|v| v.as_str())
      .map(str::to_string)
      .unwrap_or_else(|| self.current_model.clone());
    let (provider, model) = normalize_copilot_model(model_raw);
    let tokens = token_stats_from_compaction_usage(usage);
    let sid = self.resolved_session_id();
    self.records.push(UsageRecord {
      source: Source::CopilotCli,
      session_id: sid,
      session_kind: crate::model::SessionKind::Root,
      parent_session_id: None,
      session_title: None,
      project_cwd: None,
      project_name: None,
      provider: Some(provider),
      model: Some(model),
      ts: timestamp_from_event(event),
      prompt: tokens.prompt,
      completion: tokens.completion,
      input_bytes: 0,
      output_bytes: 0,
      input_estimated: false,
      output_estimated: false,
      input_bytes_estimated: true,
      output_bytes_estimated: true,
      reasoning: tokens.reasoning,
      cache_read: tokens.cache_read,
      cache_write: tokens.cache_write,
      total_direct: None,
      mode: Some("compaction".to_string()),
      agent: Some("compaction".to_string()),
      is_compaction: true,
      rounds: 0,
      calls: 1,
      cost_embedded: None,
    });
  }
}

// ---------------------------------------------------------------------------

struct DumpBuilder {
  session_id: String,
  records: Vec<DumpRecord>,
  emitted_tool_call_ids: HashSet<String>,
}

impl DumpBuilder {
  fn new(session_id: String) -> Self {
    Self {
      session_id,
      records: Vec::new(),
      emitted_tool_call_ids: HashSet::new(),
    }
  }

  fn into_session(self) -> Option<DumpedSession> {
    if self.records.is_empty() {
      None
    } else {
      Some(DumpedSession {
        session_id: self.session_id,
        records: self.records,
      })
    }
  }

  fn push_message(&mut self, role: &'static str, text: Option<&Value>, call_id: Option<String>) {
    let Some(text) = text.and_then(|v| v.as_str()).filter(|s| !s.is_empty()) else {
      return;
    };
    self.emit_span(TextSpan::new(role, text.to_string()).with_call_id(call_id));
  }

  fn push_tool_call(&mut self, name: &str, args: &Value, call_id: Option<&str>) -> Option<String> {
    let args = json_serialized_or_string::<StringSink>(Some(args));
    let text = if args.is_empty() {
      name.to_string()
    } else {
      format!("{name}: {args}")
    };
    if text.is_empty() {
      return None;
    }
    self.emit_span(TextSpan::new("tool_call", text).with_call_id(call_id.map(str::to_string)));
    call_id.map(str::to_string)
  }

  fn emit_span(&mut self, span: TextSpan<'_>) {
    if let Some(record) = DumpSink::record_from(span) {
      self.records.push(record);
    }
  }
}

impl EventsVisitor for DumpBuilder {
  fn system_message(&mut self, event: &Value) {
    self.push_message("system", event.pointer("/data/content"), None);
  }

  fn user_message(&mut self, event: &Value) {
    self.push_message("user", event.pointer("/data/content"), None);
  }

  fn assistant_text(&mut self, event: &Value) {
    self.push_message("assistant", event.pointer("/data/content"), None);
  }

  fn tool_request(&mut self, _event: &Value, request: &Value) {
    if let Some(id) = self.push_tool_call(
      request.get("name").and_then(|v| v.as_str()).unwrap_or("tool"),
      request.get("arguments").unwrap_or(&Value::Null),
      request.get("toolCallId").and_then(|v| v.as_str()),
    ) {
      self.emitted_tool_call_ids.insert(id);
    }
  }

  fn tool_execution_start(&mut self, event: &Value) {
    let data = event.get("data").unwrap_or(&Value::Null);
    let tool_call_id = data.get("toolCallId").and_then(|v| v.as_str());
    if tool_call_id.is_some_and(|id| self.emitted_tool_call_ids.contains(id)) {
      return;
    }
    self.push_tool_call(
      data.get("toolName").and_then(|v| v.as_str()).unwrap_or("tool"),
      data.get("arguments").unwrap_or(&Value::Null),
      tool_call_id,
    );
  }

  fn tool_execution_complete(&mut self, event: &Value) {
    let data = event.get("data").unwrap_or(&Value::Null);
    let text = data
      .pointer("/result/detailedContent")
      .and_then(|v| v.as_str())
      .or_else(|| data.pointer("/result/content").and_then(|v| v.as_str()))
      .or_else(|| data.pointer("/error/message").and_then(|v| v.as_str()))
      .unwrap_or("");
    if text.is_empty() {
      return;
    }
    self.emit_span(
      TextSpan::new("tool_call_result", text.to_string())
        .with_call_id(data.get("toolCallId").and_then(|v| v.as_str()).map(str::to_string)),
    );
  }
}

// ---------------------------------------------------------------------------
// Token + byte helpers
// ---------------------------------------------------------------------------

fn token_stats_from_compaction_usage(usage: &Value) -> TokenUsageStats {
  let mut sink = TokenStatsSink::default();
  sink.token(TokenSpan::usage(
    token_alias(usage, "inputTokens", "input"),
    token_alias(usage, "outputTokens", "output"),
    token_alias(usage, "reasoningTokens", "reasoning"),
    token_alias(usage, "cacheReadTokens", "cachedInput"),
    usage.get("cacheWriteTokens").and_then(|v| v.as_u64()).unwrap_or(0),
    usage.get("totalTokens").and_then(|v| v.as_u64()),
  ));
  sink.usage
}

fn rough_tokens(value: &Value) -> u64 {
  rough_chars(value).div_ceil(4)
}

fn rough_bytes(value: &Value) -> u64 {
  all_strings::<StatsSink>(Some(value)).bytes
}

fn rough_chars(value: &Value) -> u64 {
  all_strings::<StatsSink>(Some(value)).chars
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

#[cfg(test)]
mod tests {
  use super::CopilotCliSource;
  use std::path::Path;

  #[test]
  fn cache_parser_uses_event_ids_and_slots() {
    let path =
      Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/copilot_cli/session-state/session1/events.jsonl");

    let parsed = CopilotCliSource::parse_cache_file(&path).expect("parse fixture");

    assert!(parsed.complete);
    let origin_keys: Vec<_> = parsed.records.iter().map(|record| record.origin_key.as_str()).collect();
    assert_eq!(origin_keys, ["event:a1:assistant", "event:c1:compaction"]);
  }

  #[test]
  fn cache_parser_keys_shutdown_metrics_by_event_offset_and_model() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
      .join("tests/fixtures/copilot_cli_shutdown/session-state/session2/events.jsonl");

    let parsed = CopilotCliSource::parse_cache_file(&path).expect("parse fixture");

    assert!(parsed.complete);
    assert_eq!(parsed.records.len(), 1);
    assert!(
      parsed.records[0]
        .origin_key
        .ends_with(":shutdown:provider:github-copilot:model:gpt-5-mini"),
      "unexpected key: {}",
      parsed.records[0].origin_key
    );
  }
}
