use crate::model::{Source, UsageRecord};
use crate::sources::copilot_shutdown::{
  normalize_copilot_model, records_from_shutdown_model_metrics, ShutdownRecordArgs,
};
use crate::sources::dump::{DumpRecord, DumpedSession};
use crate::sources::UsageSource;
use crate::text_count::{Bytes, Chars, Counter};
use anyhow::Result;
use chrono::{DateTime, TimeZone, Utc};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};
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
          summary = %summarize(&recs),
          "file summary"
        );
        out.extend(recs);
      }
    }
    CopilotSource::dedupe_exact_sessions(&mut out);
    Ok(out)
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
  let f = File::open(path)?;
  let reader = BufReader::new(f);
  let project_name = project_cwd
    .as_ref()
    .and_then(|p| Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()));

  let mut session_id: Option<String> = None;
  let mut records = Vec::new();
  for line in reader.lines() {
    let line = match line {
      Ok(l) => l,
      Err(_) => continue,
    };
    if line.trim().is_empty() {
      continue;
    }
    let event: Value = match serde_json::from_str(&line) {
      Ok(v) => v,
      Err(_) => continue,
    };
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
  let f = File::open(path)?;
  let reader = BufReader::new(f);

  // Replay patches into a single JSON document.
  let mut state: Value = Value::Null;
  let mut requests_by_id: HashMap<String, Value> = HashMap::new();
  for line in reader.lines() {
    let line = match line {
      Ok(l) => l,
      Err(_) => continue,
    };
    if line.trim().is_empty() {
      continue;
    }
    let rec: Value = match serde_json::from_str(&line) {
      Ok(v) => v,
      Err(_) => continue,
    };
    let kind = rec.get("kind").and_then(|v| v.as_i64()).unwrap_or(-1);
    match kind {
      0 => {
        if let Some(v) = rec.get("v") {
          state = v.clone();
          capture_requests_from_state(&state, &mut requests_by_id);
        }
      }
      1 | 2 => {
        let v = match rec.get("v") {
          Some(v) => v.clone(),
          None => continue,
        };
        let path_arr = match rec.get("k").and_then(|v| v.as_array()) {
          Some(a) => a.clone(),
          None => continue,
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

  // Extract metadata.
  let session_id = state
    .get("sessionId")
    .and_then(|v| v.as_str())
    .map(|s| s.to_string())
    .unwrap_or_else(|| {
      path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string()
    });

  let creation_ms = state.get("creationDate").and_then(|v| v.as_i64());
  let title = state.get("customTitle").and_then(|v| v.as_str()).map(|s| s.to_string());

  let default_model = state
    .pointer("/inputState/selectedModel/metadata/family")
    .and_then(|v| v.as_str())
    .or_else(|| {
      state
        .pointer("/inputState/selectedModel/metadata/id")
        .and_then(|v| v.as_str())
    })
    .map(|s| s.to_string());

  let project_name = project_cwd
    .as_ref()
    .and_then(|p| Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()));

  let mut records: Vec<UsageRecord> = Vec::new();

  let requests: Vec<Value> = if requests_by_id.is_empty() {
    state
      .get("requests")
      .and_then(|v| v.as_array())
      .cloned()
      .unwrap_or_default()
  } else {
    requests_by_id.into_values().collect()
  };

  for req in &requests {
    if !req.is_object() {
      continue;
    }
    let req_ts_ms = req.get("timestamp").and_then(|v| v.as_i64()).or(creation_ms);
    let req_model_raw = req
      .pointer("/modelId")
      .and_then(|v| v.as_str())
      .or_else(|| req.pointer("/agent/modelId").and_then(|v| v.as_str()))
      .map(|s| s.to_string())
      .or_else(|| default_model.clone());
    let (provider, req_model) = match req_model_raw {
      Some(m) => {
        let (p, mm) = normalize_copilot_model(m);
        (Some(p), Some(mm))
      }
      None => (Some("github-copilot".to_string()), None),
    };

    // --- Input estimate ---
    let mut input_chars: u64 = 0;
    let mut input_bytes: u64 = 0;
    let rendered_user_chars = sum_text_chars(req.pointer("/result/metadata/renderedUserMessage"));
    let rendered_user_bytes = sum_text_bytes(req.pointer("/result/metadata/renderedUserMessage"));
    input_chars = input_chars.saturating_add(rendered_user_chars);
    input_bytes = input_bytes.saturating_add(rendered_user_bytes);
    input_chars = input_chars.saturating_add(sum_text_chars(req.pointer("/result/metadata/renderedGlobalContext")));
    input_bytes = input_bytes.saturating_add(sum_text_bytes(req.pointer("/result/metadata/renderedGlobalContext")));
    // Fallback to request.message.text when no rendered user message is present
    // (older session shapes / queued-but-unsent prompts).
    if rendered_user_chars == 0 {
      input_chars = input_chars.saturating_add(message_text(&Chars, req));
      input_bytes = input_bytes.saturating_add(message_text(&Bytes, req));
    }

    // --- Output estimate ---
    let mut output_chars: u64 = 0;
    let mut output_bytes: u64 = 0;
    if let Some(resp) = req.get("response").and_then(|v| v.as_array()) {
      for it in resp {
        output_chars = output_chars.saturating_add(response_item_chars(it));
        output_bytes = output_bytes.saturating_add(response_item_bytes(it));
      }
    }

    // --- toolCallRounds: thinking tokens (exact) ---
    // Tool call arguments are model output.
    // Tool call results/responses are fed back as model input.
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
                round_result_chars = round_result_chars.saturating_add(tool_result_chars(result));
                input_bytes = input_bytes.saturating_add(tool_result_bytes(result));
              }
            }
            if let Some(args) = call.get("arguments").and_then(|v| v.as_str()) {
              output_chars = output_chars.saturating_add(Chars.count(args));
              output_bytes = output_bytes.saturating_add(Bytes.count(args));
            }
          }
        }
        if round_result_chars == 0 {
          if let Some(resp) = round.get("response").and_then(|v| v.as_str()) {
            round_result_chars = round_result_chars.saturating_add(Chars.count(resp));
            input_bytes = input_bytes.saturating_add(Bytes.count(resp));
          }
        }
        input_chars = input_chars.saturating_add(round_result_chars);
      }
    }

    let input = input_chars.div_ceil(4);
    let output_exact = req.get("completionTokens").and_then(|v| v.as_u64());
    let output = output_exact.unwrap_or_else(|| output_chars.div_ceil(4));
    let ts = req_ts_ms
      .map(ms_to_dt)
      .unwrap_or_else(|| Utc.timestamp_opt(0, 0).single().unwrap_or_else(Utc::now));
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

    records.push(UsageRecord {
      source: Source::Copilot,
      session_id: session_id.clone(),
      session_title: title.clone(),
      project_cwd: project_cwd.clone(),
      project_name: project_name.clone(),
      provider,
      model: req_model,
      ts,
      input,
      output,
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
      turns: 1 + extra_turns,
      cost_embedded: None,
    });
  }

  if records.is_empty() {
    return Ok(None);
  }

  Ok(Some(records))
}

fn dump_session(path: &Path) -> Result<Option<DumpedSession>> {
  let f = File::open(path)?;
  let reader = BufReader::new(f);
  let mut state: Value = Value::Null;
  let mut requests_by_id: HashMap<String, Value> = HashMap::new();

  for line in reader.lines() {
    let line = match line {
      Ok(l) => l,
      Err(_) => continue,
    };
    if line.trim().is_empty() {
      continue;
    }
    let rec: Value = match serde_json::from_str(&line) {
      Ok(v) => v,
      Err(_) => continue,
    };
    let kind = rec.get("kind").and_then(|v| v.as_i64()).unwrap_or(-1);
    match kind {
      0 => {
        if let Some(v) = rec.get("v") {
          state = v.clone();
          capture_requests_from_state(&state, &mut requests_by_id);
        }
      }
      1 | 2 => {
        let v = match rec.get("v") {
          Some(v) => v.clone(),
          None => continue,
        };
        let path_arr = match rec.get("k").and_then(|v| v.as_array()) {
          Some(a) => a.clone(),
          None => continue,
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

  let session_id = state
    .get("sessionId")
    .and_then(|v| v.as_str())
    .map(|s| s.to_string())
    .unwrap_or_else(|| {
      path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string()
    });

  // Use the deduped per-id request map when available; fall back to
  // state.requests for sessions emitted as a single snapshot.
  let mut requests: Vec<Value> = if requests_by_id.is_empty() {
    state
      .get("requests")
      .and_then(|v| v.as_array())
      .cloned()
      .unwrap_or_default()
  } else {
    requests_by_id.into_values().collect()
  };
  // Stable order by timestamp where present (HashMap iteration is undefined).
  requests.sort_by_key(|r| r.get("timestamp").and_then(|v| v.as_i64()).unwrap_or(0));

  let mut out: Vec<DumpRecord> = Vec::new();
  for req in &requests {
    if !req.is_object() {
      continue;
    }

    // 1) User prompt: prefer renderedUserMessage (post-template), fall back to
    //    the raw `message.text` / `message.parts[].text`.
    let mut prompt = collect_text_array(req.pointer("/result/metadata/renderedUserMessage"));
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
    if !prompt.is_empty() {
      out.push(DumpRecord {
        role: "user",
        text: prompt,
        display: None,
        call_id: None,
      });
    }

    let tool_call_results = req
      .pointer("/result/metadata/toolCallResults")
      .and_then(|v| v.as_object());
    let mut emitted_tool_call_ids: HashSet<String> = HashSet::new();

    // 2) Assistant response: visible text/progress messages. Tool invocation
    //    records are emitted as tool_call/tool_call_result pairs.
    if let Some(resp) = req.get("response").and_then(|v| v.as_array()) {
      for item in resp {
        if item.get("kind").and_then(|v| v.as_str()) == Some("toolInvocationSerialized") {
          if let Some(id) = emit_tool_invocation_pair(item, tool_call_results, &mut out) {
            emitted_tool_call_ids.insert(id);
          }
          continue;
        }
        let text = collect_response_item_text(item);
        if !text.is_empty() {
          out.push(DumpRecord {
            role: "assistant",
            text,
            display: None,
            call_id: item.get("toolCallId").and_then(|v| v.as_str()).map(str::to_string),
          });
        }
      }
    }

    // 3) Tool call rounds: emit tool_call/tool_call_result pairs with matching
    //    call_id so consumers can replay round-trips exactly.
    if let Some(rounds) = req
      .pointer("/result/metadata/toolCallRounds")
      .and_then(|v| v.as_array())
    {
      for round in rounds {
        if let Some(calls) = round.get("toolCalls").and_then(|v| v.as_array()) {
          for call in calls {
            let Some(id) = call.get("id").and_then(|v| v.as_str()) else {
              continue;
            };
            if emitted_tool_call_ids.contains(id) {
              continue;
            }
            if let Some(args) = call.get("arguments").and_then(|v| v.as_str()) {
              if !args.is_empty() {
                let name = call.get("name").and_then(|v| v.as_str()).unwrap_or("tool");
                out.push(DumpRecord {
                  role: "tool_call",
                  text: format!("{name}: {args}"),
                  display: None,
                  call_id: Some(id.to_string()),
                });
              }
            }
            let text = match tool_call_results.and_then(|m| m.get(id)) {
              Some(result) => collect_tool_result_text(result),
              None => round
                .get("response")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_default(),
            };
            if !text.is_empty() {
              let display = round
                .get("response")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string);
              out.push(DumpRecord {
                role: "tool_call_result",
                text,
                display,
                call_id: Some(id.to_string()),
              });
            }
          }
        }
      }
    }
  }

  Ok(Some(DumpedSession {
    session_id,
    records: out,
  }))
}

fn collect_text_array(node: Option<&Value>) -> String {
  let arr = match node.and_then(|v| v.as_array()) {
    Some(a) => a,
    None => return String::new(),
  };
  let mut buf = String::new();
  for item in arr {
    let chunk = item
      .get("text")
      .and_then(|v| v.as_str())
      .or_else(|| item.get("value").and_then(|v| v.as_str()));
    if let Some(s) = chunk {
      if !buf.is_empty() {
        buf.push('\n');
      }
      buf.push_str(s);
    }
  }
  buf
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
  let kind = item.get("kind").and_then(|v| v.as_str());
  if kind == Some("toolInvocationSerialized") {
    return join_non_empty([
      collect_text_like(item.get("invocationMessage")),
      collect_text_like(item.get("pastTenseMessage")),
    ]);
  }
  if kind == Some("progressTaskSerialized") {
    return collect_text_like(item.get("content"));
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
    return String::new();
  }
  item
    .get("value")
    .and_then(|v| v.as_str())
    .map(str::to_string)
    .unwrap_or_default()
}

fn collect_text_like(value: Option<&Value>) -> String {
  match value {
    Some(Value::String(s)) => s.clone(),
    Some(Value::Object(map)) => map
      .get("text")
      .or_else(|| map.get("value"))
      .and_then(|v| v.as_str())
      .map(str::to_string)
      .unwrap_or_default(),
    Some(Value::Array(items)) => join_non_empty(items.iter().map(|item| collect_text_like(Some(item)))),
    _ => String::new(),
  }
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
  let mut buf = String::new();
  if let Some(items) = result.get("content").and_then(|v| v.as_array()) {
    for item in items {
      let s = collect_rich_text(item.get("value").unwrap_or(item));
      if !s.is_empty() {
        if !buf.is_empty() {
          buf.push('\n');
        }
        buf.push_str(&s);
      }
    }
  }
  buf
}

fn collect_rich_text(value: &Value) -> String {
  match value {
    Value::String(s) => s.clone(),
    Value::Array(items) => {
      let mut buf = String::new();
      for it in items {
        let s = collect_rich_text(it);
        if !s.is_empty() {
          if !buf.is_empty() {
            buf.push('\n');
          }
          buf.push_str(&s);
        }
      }
      buf
    }
    Value::Object(map) => {
      let mut buf = String::new();
      if let Some(t) = map.get("text").and_then(|v| v.as_str()) {
        buf.push_str(t);
      }
      if let Some(children) = map.get("children").and_then(|v| v.as_array()) {
        for c in children {
          let s = collect_rich_text(c);
          if !s.is_empty() {
            if !buf.is_empty() {
              buf.push('\n');
            }
            buf.push_str(&s);
          }
        }
      }
      if let Some(node) = map.get("node") {
        let s = collect_rich_text(node);
        if !s.is_empty() {
          if !buf.is_empty() {
            buf.push('\n');
          }
          buf.push_str(&s);
        }
      }
      buf
    }
    _ => String::new(),
  }
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

fn message_text<C: Counter>(counter: &C, req: &Value) -> u64 {
  if let Some(text) = req.pointer("/message/text").and_then(|v| v.as_str()) {
    return counter.count(text);
  }
  req
    .pointer("/message/parts")
    .and_then(|v| v.as_array())
    .map(|parts| {
      parts
        .iter()
        .filter_map(|part| part.get("text").and_then(|v| v.as_str()))
        .map(|text| counter.count(text))
        .sum()
    })
    .unwrap_or(0)
}

fn sum_text_chars(node: Option<&Value>) -> u64 {
  sum_text(&Chars, node)
}

fn sum_text_bytes(node: Option<&Value>) -> u64 {
  sum_text(&Bytes, node)
}

fn sum_text<C: Counter>(counter: &C, node: Option<&Value>) -> u64 {
  let value = match node {
    Some(v) => v,
    None => return 0,
  };
  match value {
    // Plain string (e.g. invocationMessage as a markdown string).
    Value::String(s) => counter.count(s),
    // Single object form, e.g. invocationMessage as { value: "..." }.
    Value::Object(map) => map
      .get("text")
      .or_else(|| map.get("value"))
      .and_then(|v| v.as_str())
      .map(|s| counter.count(s))
      .unwrap_or(0),
    Value::Array(arr) => {
      let mut total: u64 = 0;
      for item in arr {
        if let Some(t) = item.get("text").and_then(|v| v.as_str()) {
          total = total.saturating_add(counter.count(t));
        } else if let Some(t) = item.get("value").and_then(|v| v.as_str()) {
          total = total.saturating_add(counter.count(t));
        }
      }
      total
    }
    _ => 0,
  }
}

fn tool_result_chars(result: &Value) -> u64 {
  tool_result(&Chars, result)
}

fn tool_result_bytes(result: &Value) -> u64 {
  tool_result(&Bytes, result)
}

fn tool_result<C: Counter>(counter: &C, result: &Value) -> u64 {
  result
    .get("content")
    .and_then(|v| v.as_array())
    .map(|items| items.iter().map(|item| tool_result_content(counter, item)).sum())
    .unwrap_or(0)
}

fn tool_result_content<C: Counter>(counter: &C, item: &Value) -> u64 {
  if let Some(value) = item.get("value") {
    return rich_text(counter, value);
  }
  rich_text(counter, item)
}

fn rich_text<C: Counter>(counter: &C, value: &Value) -> u64 {
  match value {
    Value::String(s) => counter.count(s),
    Value::Array(items) => items.iter().map(|item| rich_text(counter, item)).sum(),
    Value::Object(map) => {
      let mut total: u64 = map
        .get("text")
        .and_then(|v| v.as_str())
        .map(|s| counter.count(s))
        .unwrap_or(0);
      if let Some(children) = map.get("children").and_then(|v| v.as_array()) {
        total = total.saturating_add(children.iter().map(|item| rich_text(counter, item)).sum::<u64>());
      }
      if let Some(node) = map.get("node") {
        total = total.saturating_add(rich_text(counter, node));
      }
      total
    }
    _ => 0,
  }
}

fn response_item_chars(item: &Value) -> u64 {
  response_item(&Chars, item)
}

fn response_item_bytes(item: &Value) -> u64 {
  response_item(&Bytes, item)
}

fn response_item<C: Counter>(counter: &C, item: &Value) -> u64 {
  // Plain `{value: "..."}` text segments and `{kind: "text", value: "..."}`
  // are LLM-generated text. For tool invocations, only count user-visible
  // invocation/past-tense text; skip tool payloads (tool output/input blobs).
  let kind = item.get("kind").and_then(|v| v.as_str());
  if kind == Some("toolInvocationSerialized") {
    let mut total: u64 = 0;
    total = total.saturating_add(sum_text(counter, item.pointer("/invocationMessage")));
    total = total.saturating_add(sum_text(counter, item.pointer("/pastTenseMessage")));
    return total;
  }
  if kind == Some("progressTaskSerialized") {
    // content can be {value: "..."} or {value: "...", uris: {...}}; recurse.
    return sum_text(counter, item.get("content"));
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
    return 0;
  }
  if let Some(s) = item.get("value").and_then(|v| v.as_str()) {
    counter.count(s)
  } else {
    0
  }
}

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

fn ms_to_dt(ms: i64) -> DateTime<Utc> {
  let secs = ms.div_euclid(1000);
  let nanos = (ms.rem_euclid(1000) * 1_000_000) as u32;
  Utc.timestamp_opt(secs, nanos).single().unwrap_or_else(Utc::now)
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
