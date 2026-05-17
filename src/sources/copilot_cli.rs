use crate::model::{Source, UsageRecord};
use crate::sources::copilot_shutdown::{
  normalize_copilot_model, records_from_shutdown_model_metrics, timestamp_from_event, ShutdownRecordArgs,
};
use crate::sources::dump::{DumpRecord, DumpedSession};
use crate::sources::{read_jsonl_collect, summarize_records, UsageSource};
use crate::text_count::{
  all_strings, json_serialized_or_string, DumpSink, SpanSink, StatsSink, StringSink, TextSpan, TokenSpan,
  TokenStatsSink,
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
    parse_session(path)
  }

  pub fn dump_session_messages(path: &Path) -> Result<Option<DumpedSession>> {
    dump_session(path)
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
      if let Ok(Some(recs)) = parse_session(&path) {
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
  read_jsonl_collect(path)
}

fn dump_session(path: &Path) -> Result<Option<DumpedSession>> {
  let events = read_jsonl_events(path)?;
  if events.is_empty() {
    return Ok(None);
  }

  let session_id = events
    .iter()
    .find_map(|event| {
      if event.get("type").and_then(|v| v.as_str()) == Some("session.start") {
        event.pointer("/data/sessionId").and_then(|v| v.as_str())
      } else {
        None
      }
    })
    .map(str::to_string)
    .unwrap_or_else(|| fallback_session_id(path));

  let mut records = Vec::new();
  let mut emitted_tool_call_ids: HashSet<String> = HashSet::new();
  for event in &events {
    match event.get("type").and_then(|v| v.as_str()) {
      Some("system.message") => push_message_record(&mut records, "system", event.pointer("/data/content"), None),
      Some("user.message") => push_message_record(&mut records, "user", event.pointer("/data/content"), None),
      Some("assistant.message") => {
        push_message_record(&mut records, "assistant", event.pointer("/data/content"), None);
        if let Some(tool_requests) = event.pointer("/data/toolRequests").and_then(|v| v.as_array()) {
          for request in tool_requests {
            if let Some(id) = push_tool_request_record(&mut records, request) {
              emitted_tool_call_ids.insert(id);
            }
          }
        }
      }
      Some("tool.execution_start") => {
        let data = event.get("data").unwrap_or(&Value::Null);
        let tool_call_id = data.get("toolCallId").and_then(|v| v.as_str());
        if tool_call_id.is_some_and(|id| emitted_tool_call_ids.contains(id)) {
          continue;
        }
        push_tool_call_record(
          &mut records,
          data.get("toolName").and_then(|v| v.as_str()).unwrap_or("tool"),
          data.get("arguments").unwrap_or(&Value::Null),
          tool_call_id,
        );
      }
      Some("tool.execution_complete") => {
        push_tool_result_record(&mut records, event.get("data").unwrap_or(&Value::Null))
      }
      _ => {}
    }
  }

  if records.is_empty() {
    Ok(None)
  } else {
    Ok(Some(DumpedSession { session_id, records }))
  }
}

fn push_message_record(
  records: &mut Vec<DumpRecord>,
  role: &'static str,
  text: Option<&Value>,
  call_id: Option<String>,
) {
  let Some(text) = text.and_then(|v| v.as_str()).filter(|s| !s.is_empty()) else {
    return;
  };
  emit_dump_span(records, TextSpan::new(role, text.to_string()).with_call_id(call_id));
}

fn push_tool_request_record(records: &mut Vec<DumpRecord>, request: &Value) -> Option<String> {
  push_tool_call_record(
    records,
    request.get("name").and_then(|v| v.as_str()).unwrap_or("tool"),
    request.get("arguments").unwrap_or(&Value::Null),
    request.get("toolCallId").and_then(|v| v.as_str()),
  )
}

fn push_tool_call_record(
  records: &mut Vec<DumpRecord>,
  name: &str,
  args: &Value,
  call_id: Option<&str>,
) -> Option<String> {
  let args = json_serialized_or_string::<StringSink>(Some(args));
  let text = if args.is_empty() {
    name.to_string()
  } else {
    format!("{name}: {args}")
  };
  if text.is_empty() {
    return None;
  }
  emit_dump_span(
    records,
    TextSpan::new("tool_call", text).with_call_id(call_id.map(str::to_string)),
  );
  call_id.map(str::to_string)
}

fn push_tool_result_record(records: &mut Vec<DumpRecord>, data: &Value) {
  let text = data
    .pointer("/result/detailedContent")
    .and_then(|v| v.as_str())
    .or_else(|| data.pointer("/result/content").and_then(|v| v.as_str()))
    .or_else(|| data.pointer("/error/message").and_then(|v| v.as_str()))
    .unwrap_or("");
  if text.is_empty() {
    return;
  }
  emit_dump_span(
    records,
    TextSpan::new("tool_call_result", text.to_string())
      .with_call_id(data.get("toolCallId").and_then(|v| v.as_str()).map(str::to_string)),
  );
}

fn emit_dump_span(records: &mut Vec<DumpRecord>, span: TextSpan<'_>) {
  if let Some(record) = DumpSink::record_from(span) {
    records.push(record);
  }
}

fn estimate_records_from_events(path: &Path, session_id: Option<String>, events: &[Value]) -> Vec<UsageRecord> {
  let mut records = Vec::new();
  let mut current_model = FALLBACK_MODEL.to_string();
  let mut pending_input = 0;
  let mut pending_input_bytes = 0;

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
      let data = event.get("data").unwrap_or(&Value::Null);
      pending_input += rough_tokens(data);
      pending_input_bytes += rough_bytes(data);
    }

    if event_type == Some("assistant.message") {
      let (provider, model) = normalize_copilot_model(current_model.clone());
      let output_exact = event.pointer("/data/outputTokens").and_then(|v| v.as_u64());
      let content = event.pointer("/data/content").unwrap_or(&Value::Null);
      let tool_requests = event.pointer("/data/toolRequests").unwrap_or(&Value::Null);
      let output_estimated_tokens = rough_tokens(content) + rough_tokens(tool_requests);
      let output_estimated_bytes = rough_bytes(content) + rough_bytes(tool_requests);
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
        input_bytes: pending_input_bytes,
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
      pending_input = rough_tokens(content) + rough_tokens(tool_requests);
      pending_input_bytes = rough_bytes(content) + rough_bytes(tool_requests);
    }

    if event_type == Some("session.compaction_complete") {
      if let Some(usage) = event.pointer("/data/compactionTokensUsed") {
        let model_raw = usage
          .get("model")
          .and_then(|v| v.as_str())
          .map(str::to_string)
          .unwrap_or_else(|| current_model.clone());
        let (provider, model) = normalize_copilot_model(model_raw);
        let tokens = token_stats_from_compaction_usage(usage);
        records.push(UsageRecord {
          source: Source::CopilotCli,
          session_id: session_id.clone().unwrap_or_else(|| fallback_session_id(path)),
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
          reasoning: 0,
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
  }

  records
}

fn token_stats_from_compaction_usage(usage: &Value) -> crate::text_count::TokenUsageStats {
  let mut sink = TokenStatsSink::default();
  sink.token(TokenSpan::usage(
    token_alias(usage, "inputTokens", "input"),
    token_alias(usage, "outputTokens", "output"),
    0,
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
