use crate::model::{Source, UsageRecord};
use crate::sources::copilot_shutdown::{
  normalize_copilot_model, records_from_shutdown_model_metrics, timestamp_from_event, ShutdownRecordArgs,
};
use crate::sources::dump::{DumpRecord, DumpSink, DumpedSession};
use crate::sources::{read_jsonl_collect, summarize_records, UsageSource};
use crate::text_count::{
  all_strings, json_serialized_or_string, SpanSink, StatsSink, StringSink, TextSpan, TokenSpan, TokenStatsSink,
  TokenUsageStats,
};
use anyhow::Result;
use serde_json::Value;
use std::collections::HashSet;
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
    if events.is_empty() {
      return Ok(None);
    }
    let session_id = find_session_id(&events);

    // Shutdown metrics take precedence when present.
    let shutdown: Vec<UsageRecord> = events
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
    if !shutdown.is_empty() {
      return Ok(Some(shutdown));
    }

    let mut builder = RecordBuilder::new(path, session_id);
    walk_events(&events, &mut builder);
    let records = builder.into_records();
    Ok(if records.is_empty() { None } else { Some(records) })
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

// ---------------------------------------------------------------------------
// Walker + Visitor
// ---------------------------------------------------------------------------

/// Visitor over a copilot-cli `events.jsonl` stream. Methods correspond to the
/// event kinds we actually care about; the rest are ignored.
trait EventsVisitor {
  fn session_start(&mut self, _event: &Value) {}
  fn model_change(&mut self, _event: &Value, _new_model: &str) {}
  fn system_message(&mut self, _event: &Value) {}
  fn user_message(&mut self, _event: &Value) {}
  fn assistant_message(&mut self, _event: &Value) {}
  fn tool_execution_start(&mut self, _event: &Value) {}
  fn tool_execution_complete(&mut self, _event: &Value) {}
  fn compaction_complete(&mut self, _event: &Value) {}
}

fn walk_events<V: EventsVisitor>(events: &[Value], visitor: &mut V) {
  for event in events {
    match event.get("type").and_then(|v| v.as_str()) {
      Some("session.start") => visitor.session_start(event),
      Some("session.model_change") => {
        if let Some(model) = event.pointer("/data/newModel").and_then(|v| v.as_str()) {
          visitor.model_change(event, model);
        }
      }
      Some("system.message") => visitor.system_message(event),
      Some("user.message") => visitor.user_message(event),
      Some("assistant.message") => visitor.assistant_message(event),
      Some("tool.execution_start") => visitor.tool_execution_start(event),
      Some("tool.execution_complete") => visitor.tool_execution_complete(event),
      Some("session.compaction_complete") => visitor.compaction_complete(event),
      _ => {}
    }
  }
}

fn find_session_id(events: &[Value]) -> Option<String> {
  events.iter().find_map(|event| {
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

// ---------------------------------------------------------------------------
// Parse visitor: estimates per-turn usage
// ---------------------------------------------------------------------------

struct RecordBuilder<'a> {
  path: &'a Path,
  session_id: Option<String>,
  current_model: String,
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
  fn model_change(&mut self, _event: &Value, new_model: &str) {
    self.current_model = normalize_copilot_model(new_model.to_string()).1;
  }

  fn system_message(&mut self, event: &Value) {
    self.add_pending(event.get("data").unwrap_or(&Value::Null));
  }

  fn user_message(&mut self, event: &Value) {
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
      session_title: None,
      project_cwd: None,
      project_name: None,
      provider: Some(provider),
      model: Some(model),
      ts: timestamp_from_event(event),
      input: self.pending_input,
      output: output_exact.unwrap_or(output_estimated_tokens),
      input_bytes: self.pending_input_bytes,
      output_bytes: output_estimated_bytes,
      input_estimated: true,
      output_estimated: output_exact.is_none(),
      input_bytes_estimated: true,
      output_bytes_estimated: true,
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
      session_title: None,
      project_cwd: None,
      project_name: None,
      provider: Some(provider),
      model: Some(model),
      ts: timestamp_from_event(event),
      input: tokens.input,
      output: tokens.output,
      input_bytes: 0,
      output_bytes: 0,
      input_estimated: false,
      output_estimated: false,
      input_bytes_estimated: true,
      output_bytes_estimated: true,
      reasoning: tokens.reasoning,
      cache_read: tokens.cache_read,
      cache_write: tokens.cache_write,
      mode: Some("compaction".to_string()),
      agent: Some("compaction".to_string()),
      is_compaction: true,
      rounds: 1,
      turns: 1,
      cost_embedded: None,
    });
  }
}

// ---------------------------------------------------------------------------
// Dump visitor: builds DumpedSession
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

  fn assistant_message(&mut self, event: &Value) {
    self.push_message("assistant", event.pointer("/data/content"), None);
    if let Some(tool_requests) = event.pointer("/data/toolRequests").and_then(|v| v.as_array()) {
      for request in tool_requests {
        if let Some(id) = self.push_tool_call(
          request.get("name").and_then(|v| v.as_str()).unwrap_or("tool"),
          request.get("arguments").unwrap_or(&Value::Null),
          request.get("toolCallId").and_then(|v| v.as_str()),
        ) {
          self.emitted_tool_call_ids.insert(id);
        }
      }
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
