use crate::model::{Source, UsageRecord};
use crate::sources::UsageSource;
use anyhow::Result;
use chrono::{DateTime, TimeZone, Utc};
use serde_json::Value;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
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
}

impl UsageSource for CopilotSource {
  fn name(&self) -> &'static str {
    "copilot"
  }

  fn collect(&self) -> Result<Vec<UsageRecord>> {
    let mut out = Vec::new();
    let mut workspace_cache: HashMap<PathBuf, Option<String>> = HashMap::new();

    for root in &self.roots {
      if !root.exists() {
        continue;
      }
      // Walk up to a fixed depth: <root>/<wsid>/chatSessions/<file>.jsonl
      for entry in WalkDir::new(root)
        .min_depth(3)
        .max_depth(3)
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
        if path.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str()) != Some("chatSessions") {
          continue;
        }

        let ws_dir = match path.parent().and_then(|p| p.parent()) {
          Some(d) => d.to_path_buf(),
          None => continue,
        };
        let cwd = workspace_cache
          .entry(ws_dir.clone())
          .or_insert_with(|| read_workspace_folder(&ws_dir))
          .clone();

        if let Ok(Some(rec)) = parse_session(path, cwd) {
          out.push(rec);
        }
      }
    }
    Ok(out)
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

fn parse_session(path: &Path, project_cwd: Option<String>) -> Result<Option<UsageRecord>> {
  let f = File::open(path)?;
  let reader = BufReader::new(f);

  // Replay patches into a single JSON document.
  let mut state: Value = Value::Null;
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

  let mut latest_model = default_model.clone();
  let mut latest_ts_ms: Option<i64> = creation_ms;

  let mut input_chars: u64 = 0;
  let mut output_chars: u64 = 0;
  let mut reasoning: u64 = 0;
  let mut turns: u64 = 0;

  if let Some(requests) = state.get("requests").and_then(|v| v.as_array()) {
    for req in requests {
      if !req.is_object() {
        continue;
      }
      turns += 1;
      if let Some(ts) = req.get("timestamp").and_then(|v| v.as_i64()) {
        latest_ts_ms = Some(latest_ts_ms.map(|x| x.max(ts)).unwrap_or(ts));
      }
      if let Some(m) = req
        .pointer("/modelId")
        .and_then(|v| v.as_str())
        .or_else(|| req.pointer("/agent/modelId").and_then(|v| v.as_str()))
      {
        latest_model = Some(m.to_string());
      }

      // --- Input estimate ---
      // renderedUserMessage: list of {type, text, cacheType?}
      input_chars = input_chars.saturating_add(sum_text_chars(req.pointer("/result/metadata/renderedUserMessage")));
      input_chars = input_chars.saturating_add(sum_text_chars(req.pointer("/result/metadata/renderedGlobalContext")));

      // --- Output estimate ---
      // response: list of items; only count text-bearing kinds
      if let Some(resp) = req.get("response").and_then(|v| v.as_array()) {
        for it in resp {
          output_chars = output_chars.saturating_add(response_item_chars(it));
        }
      }

      // --- toolCallRounds: thinking tokens (exact) + tool call args (output) ---
      if let Some(rounds) = req
        .pointer("/result/metadata/toolCallRounds")
        .and_then(|v| v.as_array())
      {
        for round in rounds {
          if let Some(t) = round.pointer("/thinking/tokens").and_then(|v| v.as_u64()) {
            reasoning = reasoning.saturating_add(t);
          }
          if let Some(resp) = round.get("response").and_then(|v| v.as_str()) {
            output_chars = output_chars.saturating_add(resp.chars().count() as u64);
          }
          if let Some(calls) = round.get("toolCalls").and_then(|v| v.as_array()) {
            for call in calls {
              if let Some(args) = call.get("arguments").and_then(|v| v.as_str()) {
                output_chars = output_chars.saturating_add(args.chars().count() as u64);
              }
            }
          }
        }
      }
    }
  }

  if turns == 0 {
    return Ok(None);
  }

  // ~4 chars per token, round up.
  let input = (input_chars + 3) / 4;
  let output = (output_chars + 3) / 4;

  let ts = latest_ts_ms
    .map(ms_to_dt)
    .unwrap_or_else(|| Utc.timestamp_opt(0, 0).single().unwrap_or_else(Utc::now));

  Ok(Some(UsageRecord {
    source: Source::Copilot,
    session_id,
    session_title: title,
    project_cwd: project_cwd.clone(),
    project_name: project_cwd
      .as_ref()
      .and_then(|p| Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned())),
    provider: Some("github-copilot".to_string()),
    model: latest_model,
    ts,
    input,
    output,
    reasoning,
    cache_read: 0,
    cache_write: 0,
    cost_embedded: None,
  }))
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

fn sum_text_chars(node: Option<&Value>) -> u64 {
  let arr = match node.and_then(|v| v.as_array()) {
    Some(a) => a,
    None => return 0,
  };
  let mut total: u64 = 0;
  for item in arr {
    if let Some(t) = item.get("text").and_then(|v| v.as_str()) {
      total = total.saturating_add(t.chars().count() as u64);
    } else if let Some(t) = item.get("value").and_then(|v| v.as_str()) {
      total = total.saturating_add(t.chars().count() as u64);
    }
  }
  total
}

fn response_item_chars(item: &Value) -> u64 {
  // Plain `{value: "..."}` text segments and `{kind: "text", value: "..."}`
  // are LLM-generated text. Skip tool invocations, codeblockUri, undoStop, etc.
  let kind = item.get("kind").and_then(|v| v.as_str());
  let skip = matches!(
    kind,
    Some("toolInvocationSerialized")
      | Some("codeblockUri")
      | Some("textEditGroup")
      | Some("undoStop")
      | Some("inlineReference")
      | Some("reference")
      | Some("mcpServersStarting")
      | Some("promptFile")
      | Some("agent")
  );
  if skip {
    return 0;
  }
  if let Some(s) = item.get("value").and_then(|v| v.as_str()) {
    s.chars().count() as u64
  } else {
    0
  }
}

fn ms_to_dt(ms: i64) -> DateTime<Utc> {
  let secs = ms.div_euclid(1000);
  let nanos = (ms.rem_euclid(1000) * 1_000_000) as u32;
  Utc.timestamp_opt(secs, nanos).single().unwrap_or_else(Utc::now)
}
