use crate::model::{ParsedUsageFile, ParsedUsageRecord, Source, UsageRecord};
use crate::sources::{ms_to_dt, summarize_records, UsageSource};
use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use tokn_opencode_protocol::v1::MessageData;
use tracing::debug;

pub struct OpenCodeSource {
  pub db_path: PathBuf,
}

impl OpenCodeSource {
  pub fn new(db_path: PathBuf) -> Self {
    Self { db_path }
  }

  pub fn default_path() -> Option<PathBuf> {
    let base = std::env::var_os("OPENCODE_DATA_DIR")
      .map(PathBuf::from)
      .or_else(|| {
        std::env::var_os("XDG_DATA_HOME")
          .map(PathBuf::from)
          .map(|p| p.join("opencode"))
      })
      .or_else(|| {
        std::env::var_os("HOME")
          .map(PathBuf::from)
          .map(|p| p.join(".local/share/opencode"))
      })?;
    Some(base.join("opencode.db"))
  }

  /// Parse an OpenCode database into cache records keyed by the source message
  /// identifier. SQLite rows are ordered deterministically so round counting
  /// and fallback rowid identities are stable across reads.
  pub fn parse_cache_file(path: &Path) -> Result<ParsedUsageFile> {
    if !path.exists() {
      return Ok(ParsedUsageFile::new(true, Vec::new()));
    }

    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI)
      .with_context(|| format!("opening {}", path.display()))?;

    // Pre-load session and project metadata for joins.
    let session_meta = load_session_meta(&conn).unwrap_or_default();

    let mut stmt = conn.prepare(
      "SELECT rowid, id, session_id, time_created, data FROM message \
             WHERE data LIKE '%\"role\":\"assistant\"%' \
             ORDER BY time_created, rowid",
    )?;

    let rows = stmt.query_map([], |row| {
      let rowid: i64 = row.get(0)?;
      let message_id: Option<String> = row.get(1)?;
      let session_id: String = row.get(2)?;
      let time_created: i64 = row.get(3)?;
      let data: String = row.get(4)?;
      Ok((rowid, message_id, session_id, time_created, data))
    })?;

    let mut complete = true;
    let mut records = Vec::new();
    let mut seen_parent_ids: HashSet<String> = HashSet::new();
    for row in rows {
      let (rowid, message_id, session_id, time_created, data) = match row {
        Ok(value) => value,
        Err(_) => {
          complete = false;
          continue;
        }
      };
      let parsed: MessageData = match serde_json::from_str(&data) {
        Ok(parsed) => parsed,
        Err(_) => {
          complete = false;
          continue;
        }
      };
      if !parsed.native().is_object() {
        complete = false;
        continue;
      }
      if parsed.native_role() != Some("assistant") {
        continue;
      }
      // Keep the projection deliberately narrow: OpenCode can add or change
      // unrelated fields without making an otherwise usable usage record fail.
      let message: AssistantUsagePayload = match serde_json::from_value(parsed.into_native()) {
        Ok(message) => message,
        Err(_) => {
          complete = false;
          continue;
        }
      };
      let Some(tokens) = message.tokens else {
        continue;
      };
      let cache = tokens.cache.unwrap_or_default();
      // OpenCode uses ms epoch.
      let ts_ms = message
        .time
        .as_ref()
        .and_then(|time| time.completed.or(time.created))
        .unwrap_or(time_created);
      let ts = ms_to_dt(ts_ms);

      let meta = session_meta.get(&session_id).cloned().unwrap_or_default();
      let cwd = message
        .path
        .as_ref()
        .and_then(|path| path.cwd.clone())
        .or(meta.directory.clone());

      let is_new_round = message
        .parent_id
        .as_deref()
        .is_none_or(|parent_id| seen_parent_ids.insert(parent_id.to_string()));
      let rounds = if is_new_round { 1 } else { 0 };
      let origin_key = message_id
        .filter(|id| !id.is_empty())
        .map(|id| format!("message:{id}"))
        .unwrap_or_else(|| format!("rowid:{rowid}"));

      records.push(ParsedUsageRecord {
        origin_key,
        record: UsageRecord {
          source: Source::OpenCode,
          session_id,
          session_kind: crate::model::SessionKind::Root,
          parent_session_id: None,
          session_title: meta.title.clone(),
          project_cwd: cwd,
          project_name: meta.project_name.clone(),
          provider: message.provider_id,
          model: message.model_id,
          ts,
          // Keep `input` as uncached prompt tokens only.
          prompt: tokens.input,
          completion: tokens.output,
          input_bytes: 0,
          output_bytes: 0,
          input_estimated: false,
          output_estimated: false,
          input_bytes_estimated: true,
          output_bytes_estimated: true,
          reasoning: tokens.reasoning,
          cache_read: cache.read,
          cache_write: cache.write,
          total_direct: None,
          mode: None,
          agent: None,
          is_compaction: false,
          rounds,
          calls: 1,
          cost_embedded: message.cost.filter(|cost| *cost > 0.0),
        },
      });
    }

    Ok(ParsedUsageFile::new(complete, records))
  }
}

#[derive(Debug, Deserialize)]
struct AssistantUsagePayload {
  #[serde(default, rename = "parentID")]
  parent_id: Option<String>,
  #[serde(default)]
  tokens: Option<TokenUsagePayload>,
  #[serde(default)]
  cost: Option<f64>,
  #[serde(default, rename = "modelID")]
  model_id: Option<String>,
  #[serde(default, rename = "providerID")]
  provider_id: Option<String>,
  #[serde(default)]
  path: Option<MessagePath>,
  #[serde(default)]
  time: Option<MessageTime>,
}

#[derive(Debug, Deserialize)]
struct TokenUsagePayload {
  #[serde(default)]
  input: u64,
  #[serde(default)]
  output: u64,
  #[serde(default)]
  reasoning: u64,
  #[serde(default)]
  cache: Option<CacheUsagePayload>,
}

#[derive(Debug, Deserialize, Default)]
struct CacheUsagePayload {
  #[serde(default)]
  read: u64,
  #[serde(default)]
  write: u64,
}

#[derive(Debug, Deserialize)]
struct MessagePath {
  #[serde(default)]
  cwd: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MessageTime {
  #[serde(default)]
  created: Option<i64>,
  #[serde(default)]
  completed: Option<i64>,
}

#[derive(Debug, Default, Clone)]
struct SessionMeta {
  title: Option<String>,
  directory: Option<String>,
  #[allow(dead_code)]
  project_id: Option<String>,
  project_name: Option<String>,
}

impl UsageSource for OpenCodeSource {
  fn name(&self) -> &'static str {
    "opencode"
  }

  fn collect(&self) -> Result<Vec<UsageRecord>> {
    if !self.db_path.exists() {
      return Ok(Vec::new());
    }
    debug!(source = "opencode", file = %self.db_path.display(), "processing file");
    let records = Self::parse_cache_file(&self.db_path)?.into_usage_records();
    debug!(
      source = "opencode",
      file = %self.db_path.display(),
      summary = %summarize_records(&records),
      "file summary"
    );
    Ok(records)
  }
}

fn load_session_meta(conn: &Connection) -> Result<HashMap<String, SessionMeta>> {
  // project name lookup
  let mut projects: HashMap<String, String> = HashMap::new();
  if let Ok(mut stmt) = conn.prepare("SELECT id, name FROM project") {
    let iter = stmt.query_map([], |row| {
      let id: String = row.get(0)?;
      let name: Option<String> = row.get(1)?;
      Ok((id, name))
    })?;
    for r in iter.flatten() {
      if let Some(n) = r.1 {
        projects.insert(r.0, n);
      }
    }
  }

  let mut out = HashMap::new();
  let mut stmt = conn.prepare("SELECT id, project_id, directory, title FROM session")?;
  let iter = stmt.query_map([], |row| {
    let id: String = row.get(0)?;
    let project_id: Option<String> = row.get(1)?;
    let directory: Option<String> = row.get(2)?;
    let title: Option<String> = row.get(3)?;
    Ok((id, project_id, directory, title))
  })?;
  for r in iter.flatten() {
    let project_name = r.1.as_ref().and_then(|pid| projects.get(pid).cloned());
    out.insert(
      r.0,
      SessionMeta {
        title: r.3,
        directory: r.2,
        project_id: r.1,
        project_name,
      },
    );
  }
  Ok(out)
}

#[cfg(test)]
mod tests {
  use super::*;
  use rusqlite::{params, Connection};
  use std::time::{SystemTime, UNIX_EPOCH};

  #[test]
  fn cache_parser_uses_message_ids_and_stable_message_order() {
    let unique = SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .expect("clock after epoch")
      .as_nanos();
    let path = std::env::temp_dir().join(format!("llm-tokei-opencode-cache-{unique}.db"));

    {
      let conn = Connection::open(&path).expect("create sqlite fixture");
      conn
        .execute_batch(
          "
          CREATE TABLE project (id TEXT PRIMARY KEY, name TEXT);
          CREATE TABLE session (id TEXT PRIMARY KEY, project_id TEXT, directory TEXT, title TEXT);
          CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, data TEXT);
          INSERT INTO project VALUES ('project-1', 'fixture-project');
          INSERT INTO session VALUES ('session-1', 'project-1', '/tmp/fixture', 'Fixture session');
          ",
        )
        .expect("create schema");
      let first = r#"{"role":"assistant","parentID":"parent-1","tokens":{"input":3,"output":5},"modelID":"model-1","providerID":"provider-1"}"#;
      let second = r#"{"role":"assistant","parentID":"parent-1","tokens":{"input":7,"output":11},"modelID":"model-1","providerID":"provider-1"}"#;
      // Insert out of chronological order to ensure the parser's ORDER BY is
      // what determines round counting and output order.
      conn
        .execute(
          "INSERT INTO message (id, session_id, time_created, data) VALUES (?1, ?2, ?3, ?4)",
          params!["message-2", "session-1", 2000_i64, second],
        )
        .expect("insert second message");
      conn
        .execute(
          "INSERT INTO message (id, session_id, time_created, data) VALUES (?1, ?2, ?3, ?4)",
          params!["message-1", "session-1", 1000_i64, first],
        )
        .expect("insert first message");
    }

    let parsed = OpenCodeSource::parse_cache_file(&path).expect("parse fixture");

    assert!(parsed.complete);
    let origin_keys: Vec<_> = parsed.records.iter().map(|record| record.origin_key.as_str()).collect();
    assert_eq!(origin_keys, ["message:message-1", "message:message-2"]);
    assert_eq!(parsed.records[0].record.rounds, 1);
    assert_eq!(parsed.records[1].record.rounds, 0);

    std::fs::remove_file(path).expect("remove sqlite fixture");
  }

  #[test]
  fn invalid_message_payloads_keep_valid_rows_and_mark_snapshot_incomplete() {
    let unique = SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .expect("clock after epoch")
      .as_nanos();
    let path = std::env::temp_dir().join(format!("llm-tokei-opencode-protocol-{unique}.db"));

    {
      let conn = Connection::open(&path).expect("create sqlite fixture");
      conn
        .execute_batch(
          "
          CREATE TABLE session (id TEXT PRIMARY KEY, project_id TEXT, directory TEXT, title TEXT);
          CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, data TEXT);
          INSERT INTO session VALUES ('session-1', NULL, '/tmp/fixture', 'Fixture session');
          ",
        )
        .expect("create schema");
      let null_token = r#"{"role":"assistant","tokens":{"input":null,"output":1}}"#;
      let invalid_shape = r#"[{"role":"assistant","tokens":{"input":1,"output":1}}]"#;
      let valid = r#"{"role":"assistant","tokens":{"input":7,"output":11,"reasoning":3,"cache":{"read":2,"write":1}},"modelID":"model-1","providerID":"provider-1"}"#;
      conn
        .execute(
          "INSERT INTO message (id, session_id, time_created, data) VALUES (?1, ?2, ?3, ?4)",
          params!["message-null-token", "session-1", 1000_i64, null_token],
        )
        .expect("insert null-token message");
      conn
        .execute(
          "INSERT INTO message (id, session_id, time_created, data) VALUES (?1, ?2, ?3, ?4)",
          params!["message-invalid-shape", "session-1", 1500_i64, invalid_shape],
        )
        .expect("insert invalid-shape message");
      conn
        .execute(
          "INSERT INTO message (id, session_id, time_created, data) VALUES (?1, ?2, ?3, ?4)",
          params!["message-valid", "session-1", 2000_i64, valid],
        )
        .expect("insert valid message");
    }

    let parsed = OpenCodeSource::parse_cache_file(&path).expect("parse fixture");
    std::fs::remove_file(path).expect("remove sqlite fixture");

    assert!(!parsed.complete);
    assert_eq!(parsed.records.len(), 1);
    assert_eq!(parsed.records[0].origin_key, "message:message-valid");
    let record = &parsed.records[0].record;
    assert_eq!(record.prompt, 7);
    assert_eq!(record.completion, 11);
    assert_eq!(record.reasoning, 3);
    assert_eq!(record.cache_read, 2);
    assert_eq!(record.cache_write, 1);
  }

  #[test]
  fn irrelevant_protocol_schema_changes_do_not_hide_usage() {
    let unique = SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .expect("clock after epoch")
      .as_nanos();
    let path = std::env::temp_dir().join(format!("llm-tokei-opencode-tolerant-{unique}.db"));

    {
      let conn = Connection::open(&path).expect("create sqlite fixture");
      conn
        .execute_batch(
          "
          CREATE TABLE session (id TEXT PRIMARY KEY, project_id TEXT, directory TEXT, title TEXT);
          CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, data TEXT);
          INSERT INTO session VALUES ('session-1', NULL, '/tmp/fixture', 'Fixture session');
          ",
        )
        .expect("create schema");
      let message = r#"{"role":"assistant","mode":{},"tokens":{"input":7,"output":11}}"#;
      conn
        .execute(
          "INSERT INTO message (id, session_id, time_created, data) VALUES (?1, ?2, ?3, ?4)",
          params!["message-1", "session-1", 1000_i64, message],
        )
        .expect("insert message");
    }

    let parsed = OpenCodeSource::parse_cache_file(&path).expect("parse fixture");
    std::fs::remove_file(path).expect("remove sqlite fixture");

    assert!(parsed.complete);
    assert_eq!(parsed.records.len(), 1);
    assert_eq!(parsed.records[0].record.prompt, 7);
    assert_eq!(parsed.records[0].record.completion, 11);
  }
}
