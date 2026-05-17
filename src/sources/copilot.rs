use crate::model::{Source, UsageRecord};
use crate::sources::copilot_shutdown::{
  normalize_copilot_model, records_from_shutdown_model_metrics, ShutdownRecordArgs,
};
use crate::sources::dump::{DumpRecord, DumpedSession};
use crate::sources::{ms_to_dt, read_jsonl_collect, summarize_records, UsageSource};
use crate::text_count::{rich_text, text_value, SpanSink, SpanStatsSink, StatsSink, StringSink, TextSpan, TextStats};
use anyhow::Result;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use tracing::debug;
use walkdir::WalkDir;

pub struct CopilotSource {
  pub roots: Vec<PathBuf>,
}

impl CopilotSource {
  pub fn new(roots: Vec<PathBuf>) -> Self {
    Self { roots }
  }

  /// Default `workspaceStorage` directories across known VS Code variants.
  pub fn default_paths() -> Vec<PathBuf> {
    let variants = ["Code", "Code - Insiders", "VSCodium", "VSCodium - Insiders", "Cursor"];
    let mut bases: Vec<PathBuf> = Vec::new();

    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
      // Linux
      for v in variants.iter() {
        bases.push(home.join(".config").join(v).join("User/workspaceStorage"));
      }
      // macOS
      for v in variants.iter() {
        bases.push(
          home
            .join("Library/Application Support")
            .join(v)
            .join("User/workspaceStorage"),
        );
      }
    }
    // Windows
    if let Some(appdata) = std::env::var_os("APPDATA").map(PathBuf::from) {
      for v in variants.iter() {
        bases.push(appdata.join(v).join("User/workspaceStorage"));
      }
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
        .min_depth(3)
        .max_depth(4)
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
        let parent = path.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str());
        if parent == Some("chatSessions") || parent == Some("transcripts") {
          files.push(path.to_path_buf());
        }
      }
    }
    files
  }

  pub fn parse_file(path: &Path) -> Result<Option<Vec<UsageRecord>>> {
    let ws_dir = match workspace_dir_for_file(path) {
      Some(d) => d.to_path_buf(),
      None => return Ok(None),
    };
    let cwd = read_workspace_folder(&ws_dir);
    if is_transcript_file(path) {
      parse_transcript(path, cwd)
    } else {
      parse_session(path, cwd)
    }
  }

  /// Replay a copilot session JSONL into per-message dump records.
  /// Returns `None` for transcript files (no replayable state) or if the file
  /// has no parseable state.
  pub fn dump_session_messages(path: &Path) -> Result<Option<DumpedSession>> {
    if is_transcript_file(path) {
      return Ok(None);
    }
    dump_session(path)
  }

  pub fn dedupe_exact_sessions(records: &mut Vec<UsageRecord>) {
    let exact: HashSet<String> = records
      .iter()
      .filter(|r| r.mode.as_deref() == Some("session.shutdown"))
      .map(|r| r.session_id.clone())
      .collect();
    if exact.is_empty() {
      return;
    }
    records.retain(|r| r.mode.as_deref() == Some("session.shutdown") || !exact.contains(&r.session_id));
  }
}

impl UsageSource for CopilotSource {
  fn name(&self) -> &'static str {
    "copilot"
  }

  fn collect(&self) -> Result<Vec<UsageRecord>> {
    let mut out = Vec::new();
    let mut workspace_cache: HashMap<PathBuf, Option<String>> = HashMap::new();
    for path in self.discover_files() {
      debug!(source = "copilot", file = %path.display(), "processing file");
      let ws_dir = match workspace_dir_for_file(&path) {
        Some(d) => d.to_path_buf(),
        None => continue,
      };
      let cwd = workspace_cache
        .entry(ws_dir.clone())
        .or_insert_with(|| read_workspace_folder(&ws_dir))
        .clone();
      let parsed = if is_transcript_file(&path) {
        parse_transcript(&path, cwd)
      } else {
        parse_session(&path, cwd)
      };
      if let Ok(Some(recs)) = parsed {
        debug!(
          source = "copilot",
          file = %path.display(),
          summary = %summarize_records(&recs),
          "file summary"
        );
        out.extend(recs);
      }
    }
    CopilotSource::dedupe_exact_sessions(&mut out);
    Ok(out)
  }
}

// ---------------------------------------------------------------------------
// State replay + visitor
// ---------------------------------------------------------------------------

/// Visitor over a replayed Copilot chat-session JSONL file.
///
/// The walker exposes both state-machine events (`session_snapshot`,
/// `patch_applied`) and the final, deduped request stream (`request_finalized`).
/// Current consumers only need the final state, but the replay callbacks keep
/// the state-machine boundary explicit.
/// Visitor over the final replayed Copilot chat-session state.
///
/// `walk_session` replays the JSONL state-machine protocol (`kind: 0` full
/// snapshot and `kind: 1|2` patches), deduplicates requests by `requestId`, then
/// calls these methods. The patch stream itself is an implementation detail;
/// visitors receive the full final state and each full finalized request.
trait SessionVisitor {
  /// Called once after all JSONL records have been replayed. `state` is the
  /// full reconstructed session object (commonly contains `sessionId`,
  /// `creationDate`, `customTitle`, and `inputState.selectedModel`).
  fn replay_complete(&mut self, _state: &Value) {}

  /// Called once per deduped request from `state.requests[]`. `request` is the
  /// full request object so consumers can inspect any fields they care about.
  fn request(&mut self, _request: &Value) {}
}

fn walk_session<V: SessionVisitor>(path: &Path, visitor: &mut V) -> Result<Option<()>> {
  let mut state = Value::Null;
  let mut requests_by_id: HashMap<String, Value> = HashMap::new();

  for rec in read_jsonl_collect::<Value>(path)? {
    let kind = rec.get("kind").and_then(|v| v.as_i64()).unwrap_or(-1);
    match kind {
      0 => {
        if let Some(v) = rec.get("v") {
          state = v.clone();
          capture_requests_from_state(&state, &mut requests_by_id);
        }
      }
      1 | 2 => {
        let Some(v) = rec.get("v").cloned() else {
          continue;
        };
        let Some(path_arr) = rec.get("k").and_then(|v| v.as_array()).cloned() else {
          continue;
        };
        let segments: Vec<PathSeg> = path_arr.iter().filter_map(PathSeg::from_value).collect();
        apply_patch(&mut state, &segments, v);
        capture_request_patch(&state, &path_arr, &mut requests_by_id);
      }
      _ => {}
    }
  }

  if state.is_null() {
    return Ok(None);
  }

  visitor.replay_complete(&state);
  let requests = if requests_by_id.is_empty() {
    state
      .get("requests")
      .and_then(|v| v.as_array())
      .cloned()
      .unwrap_or_default()
  } else {
    requests_by_id.into_values().collect()
  };
  for request in requests {
    visitor.request(&request);
  }

  Ok(Some(()))
}

/// Visitor over the inside of a single Copilot request.
///
/// `walk_request` passes the full request/item/round/call/result objects into
/// callbacks. High-level callbacks (`response_item`, `tool_round`) fire before
/// their lower-level callbacks, letting consumers pick their granularity.
trait RequestVisitor {
  /// User prompt for this request. `request` is the full request; `text` is the
  /// display text from `result.metadata.renderedUserMessage`, falling back to
  /// `message.text` or joined `message.parts[].text`.
  fn user_prompt(&mut self, _request: &Value, _text: &str) {}

  /// High-level callback for every item in `request.response[]`.
  fn response_item(&mut self, _item: &Value) {}

  /// Low-level callback for assistant-visible text/progress response items.
  fn assistant_text(&mut self, _item: &Value) {}

  /// Low-level callback for `kind == "toolInvocationSerialized"` response items.
  /// `results` is `request.result.metadata.toolCallResults`.
  fn tool_invocation(&mut self, _item: &Value, _results: Option<&serde_json::Map<String, Value>>) {}

  /// High-level callback for each `request.result.metadata.toolCallRounds[]` item.
  fn tool_round(&mut self, _round: &Value) {}

  /// Low-level reasoning token count from `round.thinking.tokens`.
  fn thinking(&mut self, _tokens: u64) {}

  /// Low-level callback for a single entry in `round.toolCalls[]`.
  fn tool_call(&mut self, _call: &Value) {}

  /// Low-level callback for the result corresponding to a tool call.
  /// `result` comes from `toolCallResults[call_id]`; `fallback_text` is
  /// `round.response` when no result entry exists.
  fn tool_call_result(
    &mut self,
    _result: Option<&Value>,
    _fallback_text: Option<&str>,
    _round: &Value,
    _call_id: &str,
  ) {
  }
}

fn walk_request<V: RequestVisitor>(request: &Value, visitor: &mut V) {
  let prompt = request_prompt_text(request);
  if !prompt.is_empty() {
    visitor.user_prompt(request, &prompt);
  }

  let tool_call_results = request
    .pointer("/result/metadata/toolCallResults")
    .and_then(|v| v.as_object());

  if let Some(resp) = request.get("response").and_then(|v| v.as_array()) {
    for item in resp {
      visitor.response_item(item);
      if item.get("kind").and_then(|v| v.as_str()) == Some("toolInvocationSerialized") {
        visitor.tool_invocation(item, tool_call_results);
      } else if response_item_span(item).is_some() {
        visitor.assistant_text(item);
      }
    }
  }

  if let Some(rounds) = request
    .pointer("/result/metadata/toolCallRounds")
    .and_then(|v| v.as_array())
  {
    for round in rounds {
      visitor.tool_round(round);
      if let Some(tokens) = round.pointer("/thinking/tokens").and_then(|v| v.as_u64()) {
        visitor.thinking(tokens);
      }
      if let Some(calls) = round.get("toolCalls").and_then(|v| v.as_array()) {
        for call in calls {
          visitor.tool_call(call);
          if let Some(id) = call.get("id").and_then(|v| v.as_str()) {
            visitor.tool_call_result(
              tool_call_results.and_then(|results| results.get(id)),
              round.get("response").and_then(|v| v.as_str()),
              round,
              id,
            );
          }
        }
      }
    }
  }
}

fn is_transcript_file(path: &Path) -> bool {
  path.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str()) == Some("transcripts")
}

fn workspace_dir_for_file(path: &Path) -> Option<&Path> {
  if is_transcript_file(path) {
    path.parent()?.parent()?.parent()
  } else {
    path.parent()?.parent()
  }
}

fn parse_transcript(path: &Path, project_cwd: Option<String>) -> Result<Option<Vec<UsageRecord>>> {
  let events = read_jsonl_collect::<Value>(path)?;
  let project_name = project_cwd
    .as_ref()
    .and_then(|p| Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()));

  let mut session_id: Option<String> = None;
  let mut records = Vec::new();
  for event in events {
    if event.get("type").and_then(|v| v.as_str()) == Some("session.start") {
      session_id = event
        .pointer("/data/sessionId")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or(session_id);
    }
    records.extend(records_from_shutdown_model_metrics(ShutdownRecordArgs {
      source: Source::Copilot,
      source_path: path,
      session_id: session_id.clone(),
      project_cwd: project_cwd.clone(),
      project_name: project_name.clone(),
      event: &event,
    }));
  }

  if records.is_empty() {
    Ok(None)
  } else {
    Ok(Some(records))
  }
}

fn read_workspace_folder(ws_dir: &Path) -> Option<String> {
  let p = ws_dir.join("workspace.json");
  let s = std::fs::read_to_string(&p).ok()?;
  let v: Value = serde_json::from_str(&s).ok()?;
  let folder = v.get("folder")?.as_str()?;
  // Prefer file:// URIs; otherwise return as-is.
  if let Some(rest) = folder.strip_prefix("file://") {
    // URL-decode minimally (%20 → space). serde_json is fine for our purposes;
    // for windows file:///C:/... the leading slash before the drive is fine.
    Some(percent_decode(rest))
  } else {
    Some(folder.to_string())
  }
}

fn percent_decode(s: &str) -> String {
  let bytes = s.as_bytes();
  let mut out = Vec::with_capacity(bytes.len());
  let mut i = 0;
  while i < bytes.len() {
    if bytes[i] == b'%' && i + 2 < bytes.len() {
      if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
        out.push((h << 4) | l);
        i += 3;
        continue;
      }
    }
    out.push(bytes[i]);
    i += 1;
  }
  String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

fn hex(b: u8) -> Option<u8> {
  match b {
    b'0'..=b'9' => Some(b - b'0'),
    b'a'..=b'f' => Some(b - b'a' + 10),
    b'A'..=b'F' => Some(b - b'A' + 10),
    _ => None,
  }
}

fn parse_session(path: &Path, project_cwd: Option<String>) -> Result<Option<Vec<UsageRecord>>> {
  let mut builder = RecordBuilder::new(path, project_cwd);
  if walk_session(path, &mut builder)?.is_none() {
    return Ok(None);
  }
  let records = builder.into_records();
  Ok(if records.is_empty() { None } else { Some(records) })
}

struct RecordBuilder<'a> {
  path: &'a Path,
  project_cwd: Option<String>,
  project_name: Option<String>,
  session_id: String,
  creation_ms: Option<i64>,
  title: Option<String>,
  default_model: Option<String>,
  records: Vec<UsageRecord>,
}

impl<'a> RecordBuilder<'a> {
  fn new(path: &'a Path, project_cwd: Option<String>) -> Self {
    let project_name = project_cwd
      .as_ref()
      .and_then(|p| Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()));
    Self {
      path,
      project_cwd,
      project_name,
      session_id: file_stem_or(path, "unknown"),
      creation_ms: None,
      title: None,
      default_model: None,
      records: Vec::new(),
    }
  }

  fn into_records(self) -> Vec<UsageRecord> {
    self.records
  }
}

impl SessionVisitor for RecordBuilder<'_> {
  fn replay_complete(&mut self, state: &Value) {
    self.session_id = state
      .get("sessionId")
      .and_then(|v| v.as_str())
      .map(str::to_string)
      .unwrap_or_else(|| file_stem_or(self.path, "unknown"));
    self.creation_ms = state.get("creationDate").and_then(|v| v.as_i64());
    self.title = state.get("customTitle").and_then(|v| v.as_str()).map(str::to_string);
    self.default_model = state
      .pointer("/inputState/selectedModel/metadata/family")
      .and_then(|v| v.as_str())
      .or_else(|| {
        state
          .pointer("/inputState/selectedModel/metadata/id")
          .and_then(|v| v.as_str())
      })
      .map(str::to_string);
  }

  fn request(&mut self, req: &Value) {
    if !req.is_object() {
      return;
    }
    let req_ts_ms = req.get("timestamp").and_then(|v| v.as_i64()).or(self.creation_ms);
    let req_model_raw = req
      .pointer("/modelId")
      .and_then(|v| v.as_str())
      .or_else(|| req.pointer("/agent/modelId").and_then(|v| v.as_str()))
      .map(str::to_string)
      .or_else(|| self.default_model.clone());
    let (provider, req_model) = match req_model_raw {
      Some(m) => {
        let (p, mm) = normalize_copilot_model(m);
        (Some(p), Some(mm))
      }
      None => (Some("github-copilot".to_string()), None),
    };

    let mut input_chars: u64 = 0;
    let mut input_bytes: u64 = 0;
    let rendered_user = text_like_usage(req.pointer("/result/metadata/renderedUserMessage"));
    input_chars = input_chars.saturating_add(rendered_user.chars);
    input_bytes = input_bytes.saturating_add(rendered_user.bytes);
    let rendered_global_context = text_like_usage(req.pointer("/result/metadata/renderedGlobalContext"));
    input_chars = input_chars.saturating_add(rendered_global_context.chars);
    input_bytes = input_bytes.saturating_add(rendered_global_context.bytes);
    if rendered_user.chars == 0 {
      let usage = message_text_usage(req);
      input_chars = input_chars.saturating_add(usage.chars);
      input_bytes = input_bytes.saturating_add(usage.bytes);
    }

    let mut output_chars: u64 = 0;
    let mut output_bytes: u64 = 0;
    if let Some(resp) = req.get("response").and_then(|v| v.as_array()) {
      for it in resp {
        let usage = response_item_usage(it);
        output_chars = output_chars.saturating_add(usage.chars);
        output_bytes = output_bytes.saturating_add(usage.bytes);
      }
    }

    let mut reasoning: u64 = 0;
    let mut extra_turns: u64 = 0;
    let tool_call_results = req
      .pointer("/result/metadata/toolCallResults")
      .and_then(|v| v.as_object());
    if let Some(rounds) = req
      .pointer("/result/metadata/toolCallRounds")
      .and_then(|v| v.as_array())
    {
      extra_turns = rounds.len() as u64;
      for round in rounds {
        if let Some(t) = round.pointer("/thinking/tokens").and_then(|v| v.as_u64()) {
          reasoning = reasoning.saturating_add(t);
        }
        let mut round_result_chars: u64 = 0;
        if let Some(calls) = round.get("toolCalls").and_then(|v| v.as_array()) {
          for call in calls {
            if let Some(id) = call.get("id").and_then(|v| v.as_str()) {
              if let Some(result) = tool_call_results.and_then(|results| results.get(id)) {
                let usage = tool_result_usage(result);
                round_result_chars = round_result_chars.saturating_add(usage.chars);
                input_bytes = input_bytes.saturating_add(usage.bytes);
              }
            }
            if let Some(args) = call.get("arguments").and_then(|v| v.as_str()) {
              let usage = TextStats::from_str(args);
              output_chars = output_chars.saturating_add(usage.chars);
              output_bytes = output_bytes.saturating_add(usage.bytes);
            }
          }
        }
        if round_result_chars == 0 {
          if let Some(resp) = round.get("response").and_then(|v| v.as_str()) {
            let usage = TextStats::from_str(resp);
            round_result_chars = round_result_chars.saturating_add(usage.chars);
            input_bytes = input_bytes.saturating_add(usage.bytes);
          }
        }
        input_chars = input_chars.saturating_add(round_result_chars);
      }
    }

    let output_exact = req.get("completionTokens").and_then(|v| v.as_u64());
    let command = req
      .get("command")
      .and_then(|v| v.as_str())
      .or_else(|| req.pointer("/slashCommand/command").and_then(|v| v.as_str()));
    let is_compaction =
      command == Some("compact") || req.pointer("/slashCommand/name").and_then(|v| v.as_str()) == Some("compact");
    let mode = if is_compaction {
      Some("compaction".to_string())
    } else {
      req
        .pointer("/modeInfo/modeId")
        .and_then(|v| v.as_str())
        .map(str::to_string)
    };

    self.records.push(UsageRecord {
      source: Source::Copilot,
      session_id: self.session_id.clone(),
      session_title: self.title.clone(),
      project_cwd: self.project_cwd.clone(),
      project_name: self.project_name.clone(),
      provider,
      model: req_model,
      ts: req_ts_ms.map(ms_to_dt).unwrap_or_else(|| ms_to_dt(0)),
      input: input_chars.div_ceil(4),
      output: output_exact.unwrap_or_else(|| output_chars.div_ceil(4)),
      input_bytes,
      output_bytes,
      input_estimated: true,
      output_estimated: output_exact.is_none(),
      input_bytes_estimated: true,
      output_bytes_estimated: true,
      reasoning,
      cache_read: 0,
      cache_write: 0,
      mode,
      agent: req.pointer("/agent/id").and_then(|v| v.as_str()).map(str::to_string),
      is_compaction,
      rounds: 1,
      calls: 1 + extra_turns,
      cost_embedded: None,
    });
  }
}

fn dump_session(path: &Path) -> Result<Option<DumpedSession>> {
  let mut builder = DumpBuilder::new(path);
  if walk_session(path, &mut builder)?.is_none() {
    return Ok(None);
  }
  Ok(Some(builder.into_session()))
}

struct DumpBuilder<'a> {
  path: &'a Path,
  session_id: String,
  requests: Vec<Value>,
}

impl<'a> DumpBuilder<'a> {
  fn new(path: &'a Path) -> Self {
    Self {
      path,
      session_id: file_stem_or(path, "unknown"),
      requests: Vec::new(),
    }
  }

  fn into_session(mut self) -> DumpedSession {
    self
      .requests
      .sort_by_key(|r| r.get("timestamp").and_then(|v| v.as_i64()).unwrap_or(0));
    let mut out: Vec<DumpRecord> = Vec::new();
    for req in &self.requests {
      dump_request(req, &mut out);
    }
    DumpedSession {
      session_id: self.session_id,
      records: out,
    }
  }
}

impl SessionVisitor for DumpBuilder<'_> {
  fn replay_complete(&mut self, state: &Value) {
    self.session_id = state
      .get("sessionId")
      .and_then(|v| v.as_str())
      .map(str::to_string)
      .unwrap_or_else(|| file_stem_or(self.path, "unknown"));
  }

  fn request(&mut self, request: &Value) {
    self.requests.push(request.clone());
  }
}

fn dump_request(req: &Value, out: &mut Vec<DumpRecord>) {
  let mut visitor = DumpRequestVisitor {
    out,
    emitted_tool_call_ids: HashSet::new(),
  };
  walk_request(req, &mut visitor);
}

struct DumpRequestVisitor<'a> {
  out: &'a mut Vec<DumpRecord>,
  emitted_tool_call_ids: HashSet<String>,
}

impl RequestVisitor for DumpRequestVisitor<'_> {
  fn user_prompt(&mut self, _request: &Value, text: &str) {
    self.out.push(DumpRecord {
      role: "user",
      text: text.to_string(),
      encrypted_text: None,
      display: None,
      call_id: None,
    });
  }

  fn assistant_text(&mut self, item: &Value) {
    let text = collect_response_item_text(item);
    if !text.is_empty() {
      self.out.push(DumpRecord {
        role: "assistant",
        text,
        encrypted_text: None,
        display: None,
        call_id: item.get("toolCallId").and_then(|v| v.as_str()).map(str::to_string),
      });
    }
  }

  fn tool_invocation(&mut self, item: &Value, results: Option<&serde_json::Map<String, Value>>) {
    if let Some(id) = emit_tool_invocation_pair(item, results, self.out) {
      self.emitted_tool_call_ids.insert(id);
    }
  }

  fn tool_call(&mut self, call: &Value) {
    let Some(id) = call.get("id").and_then(|v| v.as_str()) else {
      return;
    };
    if self.emitted_tool_call_ids.contains(id) {
      return;
    }
    if let Some(args) = call.get("arguments").and_then(|v| v.as_str()) {
      if !args.is_empty() {
        let name = call.get("name").and_then(|v| v.as_str()).unwrap_or("tool");
        self.out.push(DumpRecord {
          role: "tool_call",
          text: format!("{name}: {args}"),
          encrypted_text: None,
          display: None,
          call_id: Some(id.to_string()),
        });
      }
    }
  }

  fn tool_call_result(&mut self, result: Option<&Value>, fallback_text: Option<&str>, round: &Value, call_id: &str) {
    if self.emitted_tool_call_ids.contains(call_id) {
      return;
    }
    let text = match result {
      Some(result) => collect_tool_result_text(result),
      None => fallback_text.map(str::to_string).unwrap_or_default(),
    };
    if text.is_empty() {
      return;
    }
    let display = round
      .get("response")
      .and_then(|v| v.as_str())
      .filter(|s| !s.is_empty())
      .map(str::to_string);
    self.out.push(DumpRecord {
      role: "tool_call_result",
      text,
      encrypted_text: None,
      display,
      call_id: Some(call_id.to_string()),
    });
  }
}

fn emit_tool_invocation_pair(
  item: &Value,
  tool_call_results: Option<&serde_json::Map<String, Value>>,
  out: &mut Vec<DumpRecord>,
) -> Option<String> {
  let id = item.get("toolCallId").and_then(|v| v.as_str())?;
  let display = tool_invocation_display(item);
  let name = item
    .get("toolId")
    .and_then(|v| v.as_str())
    .or_else(|| item.pointer("/source/label").and_then(|v| v.as_str()))
    .unwrap_or("tool");
  let args = item
    .pointer("/toolSpecificData/prompt")
    .and_then(|v| v.as_str())
    .or_else(|| item.pointer("/toolSpecificData/description").and_then(|v| v.as_str()))
    .unwrap_or_default();

  out.push(DumpRecord {
    role: "tool_call",
    text: if args.is_empty() {
      name.to_string()
    } else {
      format!("{name}: {args}")
    },
    encrypted_text: None,
    display: if display.is_empty() {
      None
    } else {
      Some(display.clone())
    },
    call_id: Some(id.to_string()),
  });

  if let Some(text) = tool_call_results
    .and_then(|results| results.get(id))
    .map(collect_tool_result_text)
  {
    if !text.is_empty() {
      out.push(DumpRecord {
        role: "tool_call_result",
        text,
        encrypted_text: None,
        display: if display.is_empty() { None } else { Some(display) },
        call_id: Some(id.to_string()),
      });
    }
  }

  Some(id.to_string())
}

fn tool_invocation_display(item: &Value) -> String {
  join_non_empty([
    collect_text_like(item.get("invocationMessage")),
    collect_text_like(item.get("pastTenseMessage")),
  ])
}

fn collect_response_item_text(item: &Value) -> String {
  response_item_span(item)
    .map(|span| span.text.into_owned())
    .unwrap_or_default()
}

fn join_non_empty(parts: impl IntoIterator<Item = String>) -> String {
  let mut buf = String::new();
  for part in parts {
    if part.is_empty() {
      continue;
    }
    if !buf.is_empty() {
      buf.push('\n');
    }
    buf.push_str(&part);
  }
  buf
}

fn collect_tool_result_text(result: &Value) -> String {
  join_non_empty(tool_result_spans(result).into_iter().map(|span| span.text.into_owned()))
}

#[derive(Debug, Clone)]
enum PathSeg {
  Key(String),
  Index(usize),
}

impl PathSeg {
  fn from_value(v: &Value) -> Option<Self> {
    if let Some(s) = v.as_str() {
      Some(PathSeg::Key(s.to_string()))
    } else if let Some(i) = v.as_u64() {
      Some(PathSeg::Index(i as usize))
    } else if let Some(i) = v.as_i64() {
      if i >= 0 {
        Some(PathSeg::Index(i as usize))
      } else {
        None
      }
    } else {
      None
    }
  }
}

fn apply_patch(state: &mut Value, segments: &[PathSeg], value: Value) {
  if segments.is_empty() {
    *state = value;
    return;
  }
  let (head, tail) = segments.split_first().unwrap();
  match head {
    PathSeg::Key(k) => {
      if !state.is_object() {
        *state = Value::Object(serde_json::Map::new());
      }
      let map = state.as_object_mut().unwrap();
      let entry = map.entry(k.clone()).or_insert(if tail.is_empty() {
        Value::Null
      } else {
        placeholder_for(&tail[0])
      });
      apply_patch(entry, tail, value);
    }
    PathSeg::Index(i) => {
      if !state.is_array() {
        *state = Value::Array(Vec::new());
      }
      let arr = state.as_array_mut().unwrap();
      while arr.len() <= *i {
        arr.push(Value::Null);
      }
      apply_patch(&mut arr[*i], tail, value);
    }
  }
}

fn placeholder_for(seg: &PathSeg) -> Value {
  match seg {
    PathSeg::Key(_) => Value::Object(serde_json::Map::new()),
    PathSeg::Index(_) => Value::Array(Vec::new()),
  }
}

fn message_text_usage(req: &Value) -> TextStats {
  if let Some(text) = req.pointer("/message/text").and_then(|v| v.as_str()) {
    return TextStats::from_str(text);
  }
  req
    .pointer("/message/parts")
    .and_then(|v| v.as_array())
    .map(|parts| {
      let mut stats = TextStats::default();
      parts
        .iter()
        .filter_map(|part| part.get("text").and_then(|v| v.as_str()))
        .for_each(|text| stats.add(TextStats::from_str(text)));
      stats
    })
    .unwrap_or_default()
}

fn collect_text_like(value: Option<&Value>) -> String {
  text_value::<StringSink>(value)
}

fn request_prompt_text(req: &Value) -> String {
  let mut prompt = collect_text_like(req.pointer("/result/metadata/renderedUserMessage"));
  if prompt.is_empty() {
    if let Some(t) = req.pointer("/message/text").and_then(|v| v.as_str()) {
      prompt = t.to_string();
    } else if let Some(parts) = req.pointer("/message/parts").and_then(|v| v.as_array()) {
      let mut buf = String::new();
      for p in parts {
        if let Some(t) = p.get("text").and_then(|v| v.as_str()) {
          if !buf.is_empty() {
            buf.push('\n');
          }
          buf.push_str(t);
        }
      }
      prompt = buf;
    }
  }
  prompt
}

fn text_like_usage(node: Option<&Value>) -> TextStats {
  text_value::<StatsSink>(node)
}

fn tool_result_usage(result: &Value) -> TextStats {
  let mut sink = SpanStatsSink::default();
  for span in tool_result_spans(result) {
    sink.text(span);
  }
  sink.stats
}

fn response_item_usage(item: &Value) -> TextStats {
  let Some(span) = response_item_span(item) else {
    return TextStats::default();
  };
  let mut sink = SpanStatsSink::default();
  sink.text(span);
  sink.stats
}

fn response_item_span(item: &Value) -> Option<TextSpan<'static>> {
  // Plain `{value: "..."}` text segments and `{kind: "text", value: "..."}`
  // are LLM-generated text. For tool invocations, only count user-visible
  // invocation/past-tense text; skip tool payloads (tool output/input blobs).
  let kind = item.get("kind").and_then(|v| v.as_str());
  if kind == Some("toolInvocationSerialized") {
    let text = join_non_empty([
      collect_text_like(item.get("invocationMessage")),
      collect_text_like(item.get("pastTenseMessage")),
    ]);
    let mut stats = text_like_usage(item.pointer("/invocationMessage"));
    stats.add(text_like_usage(item.pointer("/pastTenseMessage")));
    return Some(TextSpan::new("assistant", text).with_stats(stats));
  }
  if kind == Some("progressTaskSerialized") {
    // content can be {value: "..."} or {value: "...", uris: {...}}; recurse.
    let text = text_value::<StringSink>(item.get("content"));
    let stats = text_like_usage(item.get("content"));
    return Some(TextSpan::new("assistant", text).with_stats(stats));
  }
  if matches!(
    kind,
    Some("codeblockUri")
      | Some("textEditGroup")
      | Some("undoStop")
      | Some("inlineReference")
      | Some("reference")
      | Some("mcpServersStarting")
      | Some("promptFile")
      | Some("agent")
      | Some("thinking")
  ) {
    // Thinking is reasoning content, accounted for via toolCallRounds.thinking.tokens.
    return None;
  }
  item
    .get("value")
    .and_then(|v| v.as_str())
    .map(|text| TextSpan::new("assistant", text.to_string()).with_stats(text_like_usage(item.get("value"))))
}

fn tool_result_spans(result: &Value) -> Vec<TextSpan<'static>> {
  let mut spans = Vec::new();
  if let Some(items) = result.get("content").and_then(|v| v.as_array()) {
    for item in items {
      let value = item.get("value").unwrap_or(item);
      let text = rich_text::<StringSink>(Some(value));
      if !text.is_empty() {
        spans.push(TextSpan::new("tool_call_result", text).with_stats(rich_text::<StatsSink>(Some(value))));
      }
    }
  }
  spans
}

fn capture_requests_from_state(state: &Value, requests_by_id: &mut HashMap<String, Value>) {
  if let Some(requests) = state.get("requests").and_then(|v| v.as_array()) {
    for request in requests {
      capture_request(request, requests_by_id);
    }
  }
}

fn capture_request_patch(state: &Value, path_arr: &[Value], requests_by_id: &mut HashMap<String, Value>) {
  if path_arr.first().and_then(|v| v.as_str()) != Some("requests") {
    return;
  }
  if path_arr.len() == 1 {
    capture_requests_from_state(state, requests_by_id);
    return;
  }
  let Some(index) = path_arr.get(1).and_then(|v| v.as_u64()).map(|i| i as usize) else {
    return;
  };
  if let Some(request) = state.pointer(&format!("/requests/{index}")) {
    capture_request(request, requests_by_id);
  }
}

fn capture_request(request: &Value, requests_by_id: &mut HashMap<String, Value>) {
  let Some(request_id) = request.get("requestId").and_then(|v| v.as_str()) else {
    return;
  };
  if request_id.is_empty() {
    return;
  }
  if let Some(existing) = requests_by_id.get_mut(request_id) {
    let prev_tokens = existing.get("completionTokens").and_then(|v| v.as_u64()).unwrap_or(0);
    let new_tokens = request.get("completionTokens").and_then(|v| v.as_u64()).unwrap_or(0);
    let mut merged = existing.clone();
    merge_objects(&mut merged, request);
    if new_tokens < prev_tokens {
      if let Some(map) = merged.as_object_mut() {
        map.insert("completionTokens".to_string(), Value::from(prev_tokens));
      }
    }
    *existing = merged;
  } else {
    requests_by_id.insert(request_id.to_string(), request.clone());
  }
}

fn merge_objects(base: &mut Value, next: &Value) {
  let (Some(base_obj), Some(next_obj)) = (base.as_object_mut(), next.as_object()) else {
    *base = next.clone();
    return;
  };
  for (key, value) in next_obj {
    base_obj.insert(key.clone(), value.clone());
  }
}

fn file_stem_or(path: &Path, fallback: &str) -> String {
  path
    .file_stem()
    .and_then(|s| s.to_str())
    .unwrap_or(fallback)
    .to_string()
}
