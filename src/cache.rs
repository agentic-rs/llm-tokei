use crate::model::{Source, UsageRecord};
use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{params, Connection, OpenFlags};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const CACHE_SCHEMA_VERSION: i64 = 7;

const SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS sessions (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    source        TEXT NOT NULL,
    session_id    TEXT NOT NULL,
    session_title TEXT,
    project_cwd   TEXT,
    project_name  TEXT,
    file_path     TEXT NOT NULL,
    first_ts      TEXT NOT NULL,
    last_ts       TEXT NOT NULL,
    file_mtime    INTEGER NOT NULL,
    pruned        INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS records (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    session_rowid INTEGER NOT NULL REFERENCES sessions(id),
    provider      TEXT,
    model         TEXT,
    ts            TEXT NOT NULL,
    prompt        INTEGER NOT NULL,
    completion    INTEGER NOT NULL,
    input_bytes   INTEGER NOT NULL,
    output_bytes  INTEGER NOT NULL,
    input_estimated INTEGER NOT NULL,
    output_estimated INTEGER NOT NULL,
    input_bytes_estimated INTEGER NOT NULL,
    output_bytes_estimated INTEGER NOT NULL,
    reasoning     INTEGER NOT NULL,
    cache_read    INTEGER NOT NULL,
    cache_write   INTEGER NOT NULL,
    total         INTEGER,
    mode          TEXT,
    agent         TEXT,
    is_compaction INTEGER NOT NULL,
    rounds        INTEGER NOT NULL,
    calls         INTEGER NOT NULL,
    cost_embedded REAL
);
CREATE INDEX IF NOT EXISTS idx_sessions_source_file ON sessions(source, file_path);
CREATE INDEX IF NOT EXISTS idx_sessions_pruned ON sessions(pruned);
CREATE INDEX IF NOT EXISTS idx_records_session ON records(session_rowid);
";

const EXPECTED_SESSIONS_COLUMNS: &[&str] = &[
  "id",
  "source",
  "session_id",
  "session_title",
  "project_cwd",
  "project_name",
  "file_path",
  "first_ts",
  "last_ts",
  "file_mtime",
  "pruned",
];

const EXPECTED_RECORDS_COLUMNS: &[&str] = &[
  "id",
  "session_rowid",
  "provider",
  "model",
  "ts",
  "prompt",
  "completion",
  "input_bytes",
  "output_bytes",
  "input_estimated",
  "output_estimated",
  "input_bytes_estimated",
  "output_bytes_estimated",
  "reasoning",
  "cache_read",
  "cache_write",
  "total",
  "mode",
  "agent",
  "is_compaction",
  "rounds",
  "calls",
  "cost_embedded",
];

pub struct CacheDb {
  conn: Connection,
}

pub struct CacheStats {
  pub scanned: usize,
  pub cached: usize,
  pub added: usize,
  pub updated: usize,
  pub pruned: usize,
}

impl CacheStats {
  pub fn new() -> Self {
    Self {
      scanned: 0,
      cached: 0,
      added: 0,
      updated: 0,
      pruned: 0,
    }
  }
}

impl CacheDb {
  pub fn open() -> Result<Self> {
    let path = Self::db_path()?;
    if let Some(parent) = path.parent() {
      std::fs::create_dir_all(parent)?;
    }

    let mut conn = Self::open_conn(&path)?;
    if Self::needs_recreate(&conn)? {
      drop(conn);
      let _ = std::fs::remove_file(&path);
      conn = Self::open_conn(&path)?;
    }

    conn.execute_batch(SCHEMA)?;
    conn.pragma_update(None, "user_version", CACHE_SCHEMA_VERSION)?;
    Ok(Self { conn })
  }

  fn open_conn(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(
      path,
      OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE | OpenFlags::SQLITE_OPEN_URI,
    )
    .with_context(|| format!("opening cache db {}", path.display()))?;
    conn
      .execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
      .ok();
    Ok(conn)
  }

  fn needs_recreate(conn: &Connection) -> Result<bool> {
    let schema_version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if schema_version != CACHE_SCHEMA_VERSION {
      return Ok(true);
    }
    let has_sessions = table_exists(conn, "sessions")?;
    let has_records = table_exists(conn, "records")?;
    if !has_sessions && !has_records {
      return Ok(false);
    }
    if !has_sessions || !has_records {
      return Ok(true);
    }
    let sessions_cols = table_columns(conn, "sessions")?;
    let records_cols = table_columns(conn, "records")?;
    Ok(
      !columns_match(&sessions_cols, EXPECTED_SESSIONS_COLUMNS)
        || !columns_match(&records_cols, EXPECTED_RECORDS_COLUMNS),
    )
  }

  fn db_path() -> Result<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
      .map(PathBuf::from)
      .or_else(|| std::env::var_os("HOME").map(PathBuf::from).map(|p| p.join(".cache")))
      .context("cannot determine cache directory")?;
    Ok(base.join("llm-tokei.db"))
  }

  pub fn load_active_for_file(&self, source: &str, file_path: &Path) -> Result<Vec<UsageRecord>> {
    let fp_str = file_path.to_string_lossy();
    let mut stmt = self.conn.prepare(
      "SELECT s.source, s.session_id, s.session_title, s.project_cwd, s.project_name, \
               r.provider, r.model, r.ts, r.prompt, r.completion, r.input_bytes, r.output_bytes, \
               r.input_estimated, r.output_estimated, r.input_bytes_estimated, r.output_bytes_estimated, \
               r.reasoning, r.cache_read, r.cache_write, r.total, r.mode, r.agent, r.is_compaction, r.rounds, \
               r.calls, r.cost_embedded \
       FROM records r \
       INNER JOIN sessions s ON s.id = r.session_rowid \
       WHERE s.pruned = 0 AND s.source = ?1 AND s.file_path = ?2",
    )?;
    let rows = stmt.query_map(params![source, fp_str.as_ref()], |row| {
      let source_str: String = row.get(0)?;
      let ts_str: String = row.get(7)?;
      Ok(row_to_record(row, &source_str, &ts_str))
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
  }

  pub fn file_mtimes_for(&self, source: &str) -> Result<HashMap<PathBuf, i64>> {
    let mut stmt = self.conn.prepare(
      "SELECT file_path, MAX(file_mtime) FROM sessions \
       WHERE source = ?1 AND pruned = 0 \
       GROUP BY file_path",
    )?;
    let rows = stmt.query_map(params![source], |row| {
      let fp: String = row.get(0)?;
      let mt: i64 = row.get(1)?;
      Ok((PathBuf::from(fp), mt))
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
  }

  pub fn upsert_file(&self, file_path: &Path, mtime: i64, source: &str, records: &[UsageRecord]) -> Result<()> {
    let fp_str = file_path.to_string_lossy();
    self.conn.execute(
      "UPDATE sessions SET pruned = 1 WHERE file_path = ?1 AND source = ?2 AND pruned = 0",
      params![fp_str.as_ref(), source],
    )?;

    let grouped = group_by_session(records);
    for (_, session_records) in grouped {
      let first = session_records.first().expect("session group is non-empty");
      let (first_ts, last_ts) = ts_range(&session_records);
      self.conn.execute(
        "INSERT INTO sessions (source, session_id, session_title, project_cwd, project_name, \
                              file_path, first_ts, last_ts, file_mtime, pruned) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0)",
        params![
          source,
          first.session_id,
          first.session_title,
          first.project_cwd,
          first.project_name,
          fp_str.as_ref(),
          first_ts,
          last_ts,
          mtime,
        ],
      )?;
      let sid = self.conn.last_insert_rowid();
      let mut insert_record = self.conn.prepare(
        "INSERT INTO records (session_rowid, provider, model, ts, prompt, completion, input_bytes, output_bytes, \
                             input_estimated, output_estimated, input_bytes_estimated, output_bytes_estimated, \
                             reasoning, cache_read, cache_write, total, mode, agent, is_compaction, rounds, calls, \
                             cost_embedded) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22)",
      )?;
      for record in session_records {
        insert_record.execute(params![
          sid,
          record.provider,
          record.model,
          record.ts.to_rfc3339(),
          to_sql_i64(record.prompt),
          to_sql_i64(record.completion),
          to_sql_i64(record.input_bytes),
          to_sql_i64(record.output_bytes),
          if record.input_estimated { 1 } else { 0 },
          if record.output_estimated { 1 } else { 0 },
          if record.input_bytes_estimated { 1 } else { 0 },
          if record.output_bytes_estimated { 1 } else { 0 },
          to_sql_i64(record.reasoning),
          to_sql_i64(record.cache_read),
          to_sql_i64(record.cache_write),
          record.total_direct.map(to_sql_i64),
          record.mode,
          record.agent,
          if record.is_compaction { 1 } else { 0 },
          to_sql_i64(record.rounds),
          to_sql_i64(record.calls),
          record.cost_embedded,
        ])?;
      }
    }

    Ok(())
  }

  pub fn prune_files(&self, source: &str, file_paths: &[PathBuf]) -> Result<usize> {
    if file_paths.is_empty() {
      return Ok(0);
    }
    let mut total = 0;
    for fp in file_paths {
      let fp_str = fp.to_string_lossy();
      total += self.conn.execute(
        "UPDATE sessions SET pruned = 1 WHERE file_path = ?1 AND source = ?2 AND pruned = 0",
        params![fp_str.as_ref(), source],
      )?;
    }
    Ok(total)
  }

  pub fn active_file_paths(&self, source: &str) -> Result<Vec<PathBuf>> {
    let mut stmt = self
      .conn
      .prepare("SELECT DISTINCT file_path FROM sessions WHERE source = ?1 AND pruned = 0")?;
    let rows = stmt.query_map(params![source], |row| {
      let fp: String = row.get(0)?;
      Ok(PathBuf::from(fp))
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
  }
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
  let exists: i64 = conn.query_row(
    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
    params![table],
    |row| row.get(0),
  )?;
  Ok(exists == 1)
}

fn table_columns(conn: &Connection, table: &str) -> Result<Vec<String>> {
  let sql = format!("PRAGMA table_info({})", table);
  let mut stmt = conn.prepare(&sql)?;
  let cols = stmt
    .query_map([], |row| row.get::<_, String>(1))?
    .filter_map(|r| r.ok())
    .collect();
  Ok(cols)
}

fn columns_match(actual: &[String], expected: &[&str]) -> bool {
  actual.len() == expected.len() && actual.iter().zip(expected.iter()).all(|(a, e)| a == e)
}

fn ts_range(records: &[UsageRecord]) -> (String, String) {
  let mut min_ts = records[0].ts;
  let mut max_ts = records[0].ts;
  for r in records.iter().skip(1) {
    if r.ts < min_ts {
      min_ts = r.ts;
    }
    if r.ts > max_ts {
      max_ts = r.ts;
    }
  }
  (min_ts.to_rfc3339(), max_ts.to_rfc3339())
}

fn group_by_session(records: &[UsageRecord]) -> HashMap<&str, Vec<UsageRecord>> {
  let mut grouped: HashMap<&str, Vec<UsageRecord>> = HashMap::new();
  for record in records {
    grouped
      .entry(record.session_id.as_str())
      .or_default()
      .push(record.clone());
  }
  grouped
}

fn row_to_record(row: &rusqlite::Row<'_>, source_str: &str, ts_str: &str) -> UsageRecord {
  let source = match source_str {
    "codex" => Source::Codex,
    "opencode" => Source::OpenCode,
    "claude" => Source::Claude,
    "copilot" => Source::Copilot,
    "copilot-cli" => Source::CopilotCli,
    "pi-agent" => Source::PiAgent,
    _ => Source::Codex,
  };
  let ts = DateTime::parse_from_rfc3339(ts_str)
    .map(|dt| dt.with_timezone(&Utc))
    .unwrap_or_else(|_| Utc.timestamp_opt(0, 0).single().unwrap_or_else(Utc::now));
  UsageRecord {
    source,
    session_id: row.get(1).unwrap_or_default(),
    session_title: row.get(2).unwrap_or(None),
    project_cwd: row.get(3).unwrap_or(None),
    project_name: row.get(4).unwrap_or(None),
    provider: row.get(5).unwrap_or(None),
    model: row.get(6).unwrap_or(None),
    ts,
    prompt: row.get::<_, i64>(8).ok().map(from_sql_i64).unwrap_or(0),
    completion: row.get::<_, i64>(9).ok().map(from_sql_i64).unwrap_or(0),
    input_bytes: row.get::<_, i64>(10).ok().map(from_sql_i64).unwrap_or(0),
    output_bytes: row.get::<_, i64>(11).ok().map(from_sql_i64).unwrap_or(0),
    input_estimated: row.get::<_, i64>(12).unwrap_or(0) != 0,
    output_estimated: row.get::<_, i64>(13).unwrap_or(0) != 0,
    input_bytes_estimated: row.get::<_, i64>(14).unwrap_or(0) != 0,
    output_bytes_estimated: row.get::<_, i64>(15).unwrap_or(0) != 0,
    reasoning: row.get::<_, i64>(16).ok().map(from_sql_i64).unwrap_or(0),
    cache_read: row.get::<_, i64>(17).ok().map(from_sql_i64).unwrap_or(0),
    cache_write: row.get::<_, i64>(18).ok().map(from_sql_i64).unwrap_or(0),
    total_direct: row.get::<_, Option<i64>>(19).unwrap_or(None).map(from_sql_i64),
    mode: row.get(20).unwrap_or(None),
    agent: row.get(21).unwrap_or(None),
    is_compaction: row.get::<_, i64>(22).unwrap_or(0) != 0,
    rounds: row.get::<_, i64>(23).ok().map(from_sql_i64).unwrap_or(0),
    calls: row.get::<_, i64>(24).ok().map(from_sql_i64).unwrap_or(0),
    cost_embedded: row.get(25).unwrap_or(None),
  }
}

fn to_sql_i64(value: u64) -> i64 {
  i64::try_from(value).unwrap_or(i64::MAX)
}

fn from_sql_i64(value: i64) -> u64 {
  u64::try_from(value).unwrap_or(0)
}
