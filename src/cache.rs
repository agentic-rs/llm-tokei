use crate::model::{Source, UsageRecord};
use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{params, Connection, OpenFlags};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS records (
    source        TEXT NOT NULL,
    session_id    TEXT NOT NULL,
    session_title TEXT,
    project_cwd   TEXT,
    project_name  TEXT,
    provider      TEXT,
    model         TEXT,
    ts            TEXT NOT NULL,
    input         INTEGER NOT NULL,
    output        INTEGER NOT NULL,
    reasoning     INTEGER NOT NULL,
    cache_read    INTEGER NOT NULL,
    cache_write   INTEGER NOT NULL,
    rounds        INTEGER NOT NULL,
    turns         INTEGER NOT NULL,
    cost_embedded REAL,
    file_path     TEXT NOT NULL,
    updated_at    INTEGER NOT NULL,
    pruned        INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_records_file ON records(file_path, updated_at);
CREATE INDEX IF NOT EXISTS idx_records_pruned ON records(pruned);
";

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
    let conn = Connection::open_with_flags(
      &path,
      OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE | OpenFlags::SQLITE_OPEN_URI,
    )
    .with_context(|| format!("opening cache db {}", path.display()))?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;").ok();
    conn.execute_batch(SCHEMA)?;
    let add_pruned = {
      let mut stmt = conn.prepare("PRAGMA table_info(records)")?;
      let cols: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .collect();
      !cols.iter().any(|c| c == "pruned")
    };
    if add_pruned {
      conn.execute_batch("ALTER TABLE records ADD COLUMN pruned INTEGER NOT NULL DEFAULT 0")?;
    }
    Ok(Self { conn })
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
      "SELECT source, session_id, session_title, project_cwd, project_name, \
              provider, model, ts, input, output, reasoning, cache_read, \
              cache_write, rounds, turns, cost_embedded \
       FROM records WHERE pruned = 0 AND source = ?1 AND file_path = ?2",
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
      "SELECT file_path, MAX(updated_at) FROM records \
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
      "UPDATE records SET pruned = 1 WHERE file_path = ?1 AND source = ?2 AND pruned = 0",
      params![fp_str.as_ref(), source],
    )?;
    let mut insert_stmt = self.conn.prepare(
      "INSERT INTO records (source, session_id, session_title, project_cwd, project_name, \
                           provider, model, ts, input, output, reasoning, cache_read, \
                           cache_write, rounds, turns, cost_embedded, file_path, updated_at, pruned) \
       VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,0)",
    )?;
    for r in records {
      let ts_str = r.ts.to_rfc3339();
      insert_stmt.execute(params![
        source,
        r.session_id,
        r.session_title,
        r.project_cwd,
        r.project_name,
        r.provider,
        r.model,
        ts_str,
        r.input,
        r.output,
        r.reasoning,
        r.cache_read,
        r.cache_write,
        r.rounds,
        r.turns,
        r.cost_embedded,
        fp_str.as_ref(),
        mtime,
      ])?;
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
        "UPDATE records SET pruned = 1 WHERE file_path = ?1 AND source = ?2 AND pruned = 0",
        params![fp_str.as_ref(), source],
      )?;
    }
    Ok(total)
  }

  pub fn active_file_paths(&self, source: &str) -> Result<Vec<PathBuf>> {
    let mut stmt = self
      .conn
      .prepare("SELECT DISTINCT file_path FROM records WHERE source = ?1 AND pruned = 0")?;
    let rows = stmt.query_map(params![source], |row| {
      let fp: String = row.get(0)?;
      Ok(PathBuf::from(fp))
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
  }
}

fn row_to_record(row: &rusqlite::Row<'_>, source_str: &str, ts_str: &str) -> UsageRecord {
  let source = match source_str {
    "codex" => Source::Codex,
    "opencode" => Source::OpenCode,
    "claude" => Source::Claude,
    "copilot" => Source::Copilot,
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
    input: row.get(8).unwrap_or(0),
    output: row.get(9).unwrap_or(0),
    reasoning: row.get(10).unwrap_or(0),
    cache_read: row.get(11).unwrap_or(0),
    cache_write: row.get(12).unwrap_or(0),
    rounds: row.get(13).unwrap_or(0),
    turns: row.get(14).unwrap_or(0),
    cost_embedded: row.get(15).unwrap_or(None),
  }
}
