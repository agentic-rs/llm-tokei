use crate::model::{Source, UsageRecord};
use crate::sources::{ms_to_dt, summarize_records, UsageSource};
use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
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
}

#[derive(Debug, Deserialize)]
struct AssistantMessage {
  #[serde(default)]
  role: Option<String>,
  #[serde(default, rename = "parentID")]
  parent_id: Option<String>,
  #[serde(default)]
  tokens: Option<TokensField>,
  #[serde(default)]
  cost: Option<f64>,
  #[serde(default, rename = "modelID")]
  model_id: Option<String>,
  #[serde(default, rename = "providerID")]
  provider_id: Option<String>,
  #[serde(default)]
  path: Option<PathField>,
  #[serde(default)]
  time: Option<TimeField>,
}

#[derive(Debug, Deserialize)]
struct TokensField {
  #[serde(default)]
  input: u64,
  #[serde(default)]
  output: u64,
  #[serde(default)]
  reasoning: u64,
  #[serde(default)]
  cache: Option<CacheField>,
}

#[derive(Debug, Deserialize, Default)]
struct CacheField {
  #[serde(default)]
  read: u64,
  #[serde(default)]
  write: u64,
}

#[derive(Debug, Deserialize)]
struct PathField {
  #[serde(default)]
  cwd: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TimeField {
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
    let conn = Connection::open_with_flags(
      &self.db_path,
      OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .with_context(|| format!("opening {}", self.db_path.display()))?;

    // Pre-load session and project metadata for joins.
    let session_meta = load_session_meta(&conn).unwrap_or_default();

    let mut stmt = conn.prepare(
      "SELECT session_id, time_created, data FROM message \
             WHERE data LIKE '%\"role\":\"assistant\"%'",
    )?;

    let rows = stmt.query_map([], |row| {
      let session_id: String = row.get(0)?;
      let time_created: i64 = row.get(1)?;
      let data: String = row.get(2)?;
      Ok((session_id, time_created, data))
    })?;

    let mut records = Vec::new();
    let mut seen_parent_ids: HashSet<String> = HashSet::new();
    for r in rows {
      let (session_id, time_created, data) = match r {
        Ok(v) => v,
        Err(_) => continue,
      };
      let parsed: AssistantMessage = match serde_json::from_str(&data) {
        Ok(p) => p,
        Err(_) => continue,
      };
      if parsed.role.as_deref() != Some("assistant") {
        continue;
      }
      let tokens = match parsed.tokens {
        Some(t) => t,
        None => continue,
      };
      let cache = tokens.cache.unwrap_or_default();
      // OpenCode uses ms epoch.
      let ts_ms = parsed
        .time
        .as_ref()
        .and_then(|t| t.completed.or(t.created))
        .unwrap_or(time_created);
      let ts = ms_to_dt(ts_ms);

      let meta = session_meta.get(&session_id).cloned().unwrap_or_default();
      let cwd = parsed
        .path
        .as_ref()
        .and_then(|p| p.cwd.clone())
        .or(meta.directory.clone());

      let is_new_round = parsed
        .parent_id
        .as_deref()
        .is_none_or(|pid| seen_parent_ids.insert(pid.to_string()));
      let rounds = if is_new_round { 1 } else { 0 };

      records.push(UsageRecord {
        source: Source::OpenCode,
        session_id,
        session_title: meta.title.clone(),
        project_cwd: cwd,
        project_name: meta.project_name.clone(),
        provider: parsed.provider_id,
        model: parsed.model_id,
        ts,
        // Keep `input` as uncached prompt tokens only.
        input: tokens.input,
        output: tokens.output,
        input_bytes: 0,
        output_bytes: 0,
        input_estimated: false,
        output_estimated: false,
        input_bytes_estimated: true,
        output_bytes_estimated: true,
        reasoning: tokens.reasoning,
        cache_read: cache.read,
        cache_write: cache.write,
        mode: None,
        agent: None,
        is_compaction: false,
        rounds,
        calls: 1,
        cost_embedded: parsed.cost.filter(|c| *c > 0.0),
      });
    }
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
