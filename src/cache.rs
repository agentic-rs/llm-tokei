use crate::model::{ParsedUsageFile, ParsedUsageRecord, SessionKind, Source, UsageRecord};
use anyhow::{Context, Result};
use blake3::hash;
use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{params, Connection, OpenFlags, Transaction};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

const CACHE_SCHEMA_VERSION: i64 = 9;

const FILES_SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS files (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    source        TEXT NOT NULL,
    file_path     TEXT NOT NULL,
    file_mtime_ns INTEGER NOT NULL,
    file_size     INTEGER NOT NULL,
    file_aux_mtime_ns INTEGER NOT NULL,
    file_aux_size INTEGER NOT NULL,
    UNIQUE(source, file_path)
);\
";

const SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS files (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    source        TEXT NOT NULL,
    file_path     TEXT NOT NULL,
    file_mtime_ns INTEGER NOT NULL,
    file_size     INTEGER NOT NULL,
    file_aux_mtime_ns INTEGER NOT NULL,
    file_aux_size INTEGER NOT NULL,
    UNIQUE(source, file_path)
);
CREATE TABLE IF NOT EXISTS sessions (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    source        TEXT NOT NULL,
    session_id    TEXT NOT NULL,
    session_kind  TEXT NOT NULL,
    parent_session_id TEXT,
    session_title TEXT,
    project_cwd   TEXT,
    project_name  TEXT,
    file_path     TEXT NOT NULL,
    first_ts      TEXT NOT NULL,
    last_ts       TEXT NOT NULL,
    file_mtime    INTEGER NOT NULL,
    pruned        INTEGER NOT NULL DEFAULT 0,
    file_id       INTEGER REFERENCES files(id),
    session_hash  BLOB
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
    cost_embedded REAL,
    origin_key    TEXT,
    record_hash   BLOB
);
CREATE INDEX IF NOT EXISTS idx_files_source_file ON files(source, file_path);
CREATE INDEX IF NOT EXISTS idx_sessions_source_file ON sessions(source, file_path);
CREATE INDEX IF NOT EXISTS idx_sessions_pruned ON sessions(pruned);
CREATE INDEX IF NOT EXISTS idx_sessions_file_id ON sessions(file_id);
CREATE INDEX IF NOT EXISTS idx_records_session ON records(session_rowid);
";

const V8_SESSIONS_COLUMNS: &[&str] = &[
  "id",
  "source",
  "session_id",
  "session_kind",
  "parent_session_id",
  "session_title",
  "project_cwd",
  "project_name",
  "file_path",
  "first_ts",
  "last_ts",
  "file_mtime",
  "pruned",
];

const V8_RECORDS_COLUMNS: &[&str] = &[
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

const EXPECTED_FILES_COLUMNS: &[&str] = &[
  "id",
  "source",
  "file_path",
  "file_mtime_ns",
  "file_size",
  "file_aux_mtime_ns",
  "file_aux_size",
];

const EXPECTED_SESSIONS_COLUMNS: &[&str] = &[
  "id",
  "source",
  "session_id",
  "session_kind",
  "parent_session_id",
  "session_title",
  "project_cwd",
  "project_name",
  "file_path",
  "first_ts",
  "last_ts",
  "file_mtime",
  "pruned",
  "file_id",
  "session_hash",
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
  "origin_key",
  "record_hash",
];

pub struct CacheDb {
  conn: Connection,
}

pub struct CacheStats {
  pub scanned: usize,
  pub cached: usize,
  pub added: usize,
  pub updated: usize,
}

pub struct CachePruneStats {
  pub sessions: i64,
  pub records: i64,
}

/// The portion of a source-file stat that determines a cache hit.
///
/// We use nanosecond precision and byte length: some JSONL writers update a
/// file more than once within one second, and an unchanged mtime alone is not
/// enough to safely reuse a completed cache parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileStamp {
  mtime_ns: i64,
  file_size: i64,
  aux_mtime_ns: i64,
  aux_size: i64,
}

impl FileStamp {
  pub fn from_path(path: &Path) -> Option<Self> {
    let (mtime_ns, file_size) = file_metadata_stamp(path)?;
    Some(Self {
      mtime_ns,
      file_size,
      aux_mtime_ns: -1,
      aux_size: -1,
    })
  }

  /// Include SQLite's active WAL in the cache key. Recent OpenCode commits can
  /// live solely in `opencode.db-wal`, leaving the main database's metadata
  /// unchanged until a checkpoint.
  pub fn from_sqlite_database(path: &Path) -> Option<Self> {
    let (mtime_ns, file_size) = file_metadata_stamp(path)?;
    let mut wal_path = path.as_os_str().to_os_string();
    wal_path.push("-wal");
    let wal_path = PathBuf::from(wal_path);
    let (aux_mtime_ns, aux_size) = file_metadata_stamp(&wal_path).unwrap_or((-1, -1));
    Some(Self {
      mtime_ns,
      file_size,
      aux_mtime_ns,
      aux_size,
    })
  }
}

fn file_metadata_stamp(path: &Path) -> Option<(i64, i64)> {
  let metadata = std::fs::metadata(path).ok()?;
  let modified = metadata.modified().ok()?;
  let duration = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
  Some((
    i64::try_from(duration.as_nanos()).unwrap_or(i64::MAX),
    i64::try_from(metadata.len()).unwrap_or(i64::MAX),
  ))
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SessionKey {
  session_id: String,
  session_kind: SessionKind,
}

struct CachedSession {
  rowid: i64,
  session_hash: Option<Vec<u8>>,
}

#[derive(Serialize)]
struct SessionFingerprint<'a> {
  source: Source,
  session_id: &'a str,
  session_kind: SessionKind,
  parent_session_id: Option<&'a str>,
  session_title: Option<&'a str>,
  project_cwd: Option<&'a str>,
  project_name: Option<&'a str>,
  first_ts: &'a str,
  last_ts: &'a str,
}

#[derive(Serialize)]
struct RecordFingerprint<'a> {
  source: Source,
  provider: Option<&'a str>,
  model: Option<&'a str>,
  ts: &'a DateTime<Utc>,
  prompt: u64,
  completion: u64,
  input_bytes: u64,
  output_bytes: u64,
  input_estimated: bool,
  output_estimated: bool,
  input_bytes_estimated: bool,
  output_bytes_estimated: bool,
  reasoning: u64,
  cache_read: u64,
  cache_write: u64,
  total_direct: Option<u64>,
  mode: Option<&'a str>,
  agent: Option<&'a str>,
  is_compaction: bool,
  rounds: u64,
  calls: u64,
  cost_embedded: Option<f64>,
}

impl CacheStats {
  pub fn new() -> Self {
    Self {
      scanned: 0,
      cached: 0,
      added: 0,
      updated: 0,
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
    if Self::can_migrate_v8(&conn)? {
      Self::migrate_v8(&mut conn)?;
    } else if Self::needs_recreate(&conn)? {
      drop(conn);
      remove_cache_files(&path);
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
    conn.pragma_update(None, "foreign_keys", "ON")?;
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
    let has_files = table_exists(conn, "files")?;
    let has_sessions = table_exists(conn, "sessions")?;
    let has_records = table_exists(conn, "records")?;
    if !has_files && !has_sessions && !has_records {
      return Ok(false);
    }
    if !has_files || !has_sessions || !has_records {
      return Ok(true);
    }
    Ok(
      !columns_match(&table_columns(conn, "files")?, EXPECTED_FILES_COLUMNS)
        || !columns_match(&table_columns(conn, "sessions")?, EXPECTED_SESSIONS_COLUMNS)
        || !columns_match(&table_columns(conn, "records")?, EXPECTED_RECORDS_COLUMNS),
    )
  }

  fn can_migrate_v8(conn: &Connection) -> Result<bool> {
    let schema_version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if schema_version != 8 || table_exists(conn, "files")? {
      return Ok(false);
    }
    if !table_exists(conn, "sessions")? || !table_exists(conn, "records")? {
      return Ok(false);
    }
    Ok(
      columns_match(&table_columns(conn, "sessions")?, V8_SESSIONS_COLUMNS)
        && columns_match(&table_columns(conn, "records")?, V8_RECORDS_COLUMNS),
    )
  }

  /// Make a v8 cache readable without scanning every historical record.
  ///
  /// v8 did not retain source-event identities, so an active v8 file is
  /// replaced once on its next complete parse. The old `pruned` history stays
  /// available for `cache prune` to reclaim in a deliberate maintenance step.
  fn migrate_v8(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute_batch(FILES_SCHEMA)?;
    tx.execute_batch(
      "ALTER TABLE sessions ADD COLUMN file_id INTEGER REFERENCES files(id);\
       ALTER TABLE sessions ADD COLUMN session_hash BLOB;\
       ALTER TABLE records ADD COLUMN origin_key TEXT;\
       ALTER TABLE records ADD COLUMN record_hash BLOB;",
    )?;
    tx.execute_batch(SCHEMA)?;
    tx.execute(
      "INSERT INTO files (source, file_path, file_mtime_ns, file_size, file_aux_mtime_ns, file_aux_size) \
       SELECT source, file_path, MAX(file_mtime) * 1000000000, 0, -1, -1 \
       FROM sessions \
       WHERE pruned = 0 \
       GROUP BY source, file_path",
      [],
    )?;
    tx.execute(
      "UPDATE sessions \
       SET file_id = ( \
         SELECT id FROM files \
         WHERE files.source = sessions.source AND files.file_path = sessions.file_path \
       ) \
       WHERE pruned = 0",
      [],
    )?;
    tx.pragma_update(None, "user_version", CACHE_SCHEMA_VERSION)?;
    tx.commit()?;
    Ok(())
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
      "SELECT s.source, s.session_id, s.session_kind, s.parent_session_id, s.session_title, s.project_cwd, s.project_name, \
              r.provider, r.model, r.ts, r.prompt, r.completion, r.input_bytes, r.output_bytes, \
              r.input_estimated, r.output_estimated, r.input_bytes_estimated, r.output_bytes_estimated, \
              r.reasoning, r.cache_read, r.cache_write, r.total, r.mode, r.agent, r.is_compaction, r.rounds, \
              r.calls, r.cost_embedded \
       FROM records r \
       INNER JOIN sessions s ON s.id = r.session_rowid \
       WHERE s.pruned = 0 AND s.source = ?1 AND s.file_path = ?2 \
       ORDER BY s.id, r.id",
    )?;
    let rows = stmt.query_map(params![source, fp_str.as_ref()], |row| {
      let source_str: String = row.get(0)?;
      let ts_str: String = row.get(9)?;
      Ok(row_to_record(row, &source_str, &ts_str))
    })?;
    Ok(rows.filter_map(|row| row.ok()).collect())
  }

  pub fn file_stamps_for(&self, source: &str) -> Result<HashMap<PathBuf, FileStamp>> {
    let mut stmt = self.conn.prepare(
      "SELECT file_path, file_mtime_ns, file_size, file_aux_mtime_ns, file_aux_size \
         FROM files WHERE source = ?1",
    )?;
    let rows = stmt.query_map(params![source], |row| {
      let file_path: String = row.get(0)?;
      Ok((
        PathBuf::from(file_path),
        FileStamp {
          mtime_ns: row.get(1)?,
          file_size: row.get(2)?,
          aux_mtime_ns: row.get(3)?,
          aux_size: row.get(4)?,
        },
      ))
    })?;
    Ok(rows.filter_map(|row| row.ok()).collect())
  }

  /// Reconcile one complete file snapshot.
  ///
  /// Record identity (`origin_key`) and record content (`record_hash`) are
  /// intentionally separate: equal API calls may be distinct source events.
  /// This leaves unchanged records untouched, updates changed records in place,
  /// inserts newly observed events, and deletes only confirmed absences.
  pub fn upsert_file(
    &mut self,
    file_path: &Path,
    stamp: FileStamp,
    source: &str,
    parsed: &ParsedUsageFile,
  ) -> Result<()> {
    if !parsed.complete {
      anyhow::bail!("refusing to reconcile an incomplete cache parse");
    }
    let grouped = group_parsed_by_session(&parsed.records)?;
    let file_path = file_path.to_string_lossy();

    // A v8 row has no identities. Rebuild that one file once, atomically,
    // rather than making unsafe guesses about its previous records.
    if self.file_needs_rebuild(source, file_path.as_ref())? {
      let tx = self.conn.transaction()?;
      replace_file_snapshot(&tx, source, file_path.as_ref(), stamp, &grouped)?;
      tx.commit()?;
      return Ok(());
    }

    let tx = self.conn.transaction()?;
    delete_pruned_file_rows(&tx, source, file_path.as_ref())?;
    let file_id = upsert_file_row(&tx, source, file_path.as_ref(), stamp)?;
    let mut existing_sessions = load_active_sessions(&tx, file_id)?;

    for (session_key, session_records) in grouped {
      let first = &session_records[0].record;
      let (first_ts, last_ts) = parsed_ts_range(&session_records);
      let session_hash = session_fingerprint(first, &first_ts, &last_ts)?;
      let session_rowid = match existing_sessions.remove(&session_key) {
        Some(cached) => {
          if cached.session_hash.as_deref() != Some(session_hash.as_slice()) {
            update_session(
              &tx,
              cached.rowid,
              source,
              file_path.as_ref(),
              stamp,
              first,
              &first_ts,
              &last_ts,
              &session_hash,
            )?;
          }
          cached.rowid
        }
        None => insert_session(
          &tx,
          file_id,
          source,
          file_path.as_ref(),
          stamp,
          first,
          &first_ts,
          &last_ts,
          &session_hash,
        )?,
      };
      reconcile_session_records(&tx, session_rowid, &session_records)?;
    }

    for cached in existing_sessions.into_values() {
      delete_session_rows(&tx, cached.rowid)?;
    }
    tx.commit()?;
    Ok(())
  }

  fn file_needs_rebuild(&self, source: &str, file_path: &str) -> Result<bool> {
    let has_unkeyed: i64 = self.conn.query_row(
      "SELECT EXISTS( \
         SELECT 1 FROM sessions s \
         WHERE s.source = ?1 AND s.file_path = ?2 AND s.pruned = 0 \
           AND ( \
             s.file_id IS NULL OR s.session_hash IS NULL \
             OR NOT EXISTS (SELECT 1 FROM records r WHERE r.session_rowid = s.id) \
             OR EXISTS( \
               SELECT 1 FROM records r \
               WHERE r.session_rowid = s.id AND (r.origin_key IS NULL OR r.record_hash IS NULL) \
             ) \
           ) \
       )",
      params![source, file_path],
      |row| row.get(0),
    )?;
    if has_unkeyed != 0 {
      return Ok(true);
    }

    let has_duplicate_sessions: i64 = self.conn.query_row(
      "SELECT EXISTS( \
         SELECT 1 FROM sessions \
         WHERE source = ?1 AND file_path = ?2 AND pruned = 0 \
         GROUP BY session_id, session_kind \
         HAVING COUNT(*) > 1 \
       )",
      params![source, file_path],
      |row| row.get(0),
    )?;
    Ok(has_duplicate_sessions != 0)
  }

  /// Remove v8's historical `pruned` snapshots and compact any free pages.
  pub fn prune(&mut self) -> Result<CachePruneStats> {
    let tx = self.conn.transaction()?;
    let sessions = tx.query_row("SELECT COUNT(*) FROM sessions WHERE pruned != 0", [], |row| row.get(0))?;
    let records = tx.query_row(
      "SELECT COUNT(*) FROM records WHERE session_rowid IN (SELECT id FROM sessions WHERE pruned != 0)",
      [],
      |row| row.get(0),
    )?;
    tx.execute(
      "DELETE FROM records WHERE session_rowid IN (SELECT id FROM sessions WHERE pruned != 0)",
      [],
    )?;
    tx.execute("DELETE FROM sessions WHERE pruned != 0", [])?;
    tx.commit()?;

    let free_pages: i64 = self.conn.pragma_query_value(None, "freelist_count", |row| row.get(0))?;
    if sessions > 0 || records > 0 || free_pages > 0 {
      self.conn.execute_batch("VACUUM").with_context(|| {
        format!(
          "removed {sessions} obsolete cache sessions and {records} records, but could not compact the cache file"
        )
      })?;
    }
    Ok(CachePruneStats { sessions, records })
  }
}

fn group_parsed_by_session<'a>(
  records: &'a [ParsedUsageRecord],
) -> Result<HashMap<SessionKey, Vec<&'a ParsedUsageRecord>>> {
  let mut grouped = HashMap::new();
  let mut origin_keys = HashSet::new();
  for parsed in records {
    if parsed.origin_key.is_empty() {
      anyhow::bail!("cache record has an empty source origin key");
    }
    if !origin_keys.insert(parsed.origin_key.as_str()) {
      anyhow::bail!(
        "cache parse contains a duplicate source origin key: {}",
        parsed.origin_key
      );
    }
    let key = SessionKey {
      session_id: parsed.record.session_id.clone(),
      session_kind: parsed.record.session_kind,
    };
    grouped.entry(key).or_insert_with(Vec::new).push(parsed);
  }
  Ok(grouped)
}

fn parsed_ts_range(records: &[&ParsedUsageRecord]) -> (String, String) {
  let mut min_ts = records[0].record.ts;
  let mut max_ts = records[0].record.ts;
  for parsed in records.iter().skip(1) {
    if parsed.record.ts < min_ts {
      min_ts = parsed.record.ts;
    }
    if parsed.record.ts > max_ts {
      max_ts = parsed.record.ts;
    }
  }
  (min_ts.to_rfc3339(), max_ts.to_rfc3339())
}

fn fingerprint<T: Serialize>(value: &T) -> Result<Vec<u8>> {
  let encoded = serde_json::to_vec(value).context("serializing cache fingerprint")?;
  Ok(hash(&encoded).as_bytes().to_vec())
}

fn session_fingerprint(record: &UsageRecord, first_ts: &str, last_ts: &str) -> Result<Vec<u8>> {
  fingerprint(&SessionFingerprint {
    source: record.source,
    session_id: record.session_id.as_str(),
    session_kind: record.session_kind,
    parent_session_id: record.parent_session_id.as_deref(),
    session_title: record.session_title.as_deref(),
    project_cwd: record.project_cwd.as_deref(),
    project_name: record.project_name.as_deref(),
    first_ts,
    last_ts,
  })
}

fn record_fingerprint(record: &UsageRecord) -> Result<Vec<u8>> {
  fingerprint(&RecordFingerprint {
    source: record.source,
    provider: record.provider.as_deref(),
    model: record.model.as_deref(),
    ts: &record.ts,
    prompt: record.prompt,
    completion: record.completion,
    input_bytes: record.input_bytes,
    output_bytes: record.output_bytes,
    input_estimated: record.input_estimated,
    output_estimated: record.output_estimated,
    input_bytes_estimated: record.input_bytes_estimated,
    output_bytes_estimated: record.output_bytes_estimated,
    reasoning: record.reasoning,
    cache_read: record.cache_read,
    cache_write: record.cache_write,
    total_direct: record.total_direct,
    mode: record.mode.as_deref(),
    agent: record.agent.as_deref(),
    is_compaction: record.is_compaction,
    rounds: record.rounds,
    calls: record.calls,
    cost_embedded: record.cost_embedded,
  })
}

fn replace_file_snapshot(
  tx: &Transaction<'_>,
  source: &str,
  file_path: &str,
  stamp: FileStamp,
  grouped: &HashMap<SessionKey, Vec<&ParsedUsageRecord>>,
) -> Result<()> {
  delete_file_rows(tx, source, file_path)?;
  let file_id = upsert_file_row(tx, source, file_path, stamp)?;
  for session_records in grouped.values() {
    let first = &session_records[0].record;
    let (first_ts, last_ts) = parsed_ts_range(session_records);
    let session_hash = session_fingerprint(first, &first_ts, &last_ts)?;
    let session_rowid = insert_session(
      tx,
      file_id,
      source,
      file_path,
      stamp,
      first,
      &first_ts,
      &last_ts,
      &session_hash,
    )?;
    reconcile_session_records(tx, session_rowid, session_records)?;
  }
  Ok(())
}

fn upsert_file_row(tx: &Transaction<'_>, source: &str, file_path: &str, stamp: FileStamp) -> Result<i64> {
  tx.execute(
    "INSERT INTO files (source, file_path, file_mtime_ns, file_size, file_aux_mtime_ns, file_aux_size) \
     VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
     ON CONFLICT(source, file_path) DO UPDATE SET \
       file_mtime_ns = excluded.file_mtime_ns, \
       file_size = excluded.file_size, \
       file_aux_mtime_ns = excluded.file_aux_mtime_ns, \
       file_aux_size = excluded.file_aux_size",
    params![
      source,
      file_path,
      stamp.mtime_ns,
      stamp.file_size,
      stamp.aux_mtime_ns,
      stamp.aux_size,
    ],
  )?;
  Ok(tx.query_row(
    "SELECT id FROM files WHERE source = ?1 AND file_path = ?2",
    params![source, file_path],
    |row| row.get(0),
  )?)
}

fn load_active_sessions(tx: &Transaction<'_>, file_id: i64) -> Result<HashMap<SessionKey, CachedSession>> {
  let mut stmt = tx.prepare(
    "SELECT id, session_id, session_kind, session_hash \
     FROM sessions \
     WHERE file_id = ?1 AND pruned = 0",
  )?;
  let rows = stmt.query_map(params![file_id], |row| {
    Ok((
      row.get::<_, i64>(0)?,
      row.get::<_, String>(1)?,
      row.get::<_, String>(2)?,
      row.get::<_, Option<Vec<u8>>>(3)?,
    ))
  })?;
  let mut sessions = HashMap::new();
  for row in rows {
    let (rowid, session_id, session_kind, session_hash) = row?;
    let session_kind = match session_kind.as_str() {
      "root" => SessionKind::Root,
      "sub_agent" => SessionKind::SubAgent,
      _ => anyhow::bail!("cache contains an unknown session kind: {session_kind}"),
    };
    let key = SessionKey {
      session_id,
      session_kind,
    };
    if sessions.insert(key, CachedSession { rowid, session_hash }).is_some() {
      anyhow::bail!("cache contains duplicate active session rows");
    }
  }
  Ok(sessions)
}

fn insert_session(
  tx: &Transaction<'_>,
  file_id: i64,
  source: &str,
  file_path: &str,
  stamp: FileStamp,
  record: &UsageRecord,
  first_ts: &str,
  last_ts: &str,
  session_hash: &[u8],
) -> Result<i64> {
  tx.execute(
    "INSERT INTO sessions (source, session_id, session_kind, parent_session_id, session_title, project_cwd, project_name, \
                          file_path, first_ts, last_ts, file_mtime, pruned, file_id, session_hash) \
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 0, ?12, ?13)",
    params![
      source,
      record.session_id.as_str(),
      record.session_kind.as_str(),
      record.parent_session_id.as_deref(),
      record.session_title.as_deref(),
      record.project_cwd.as_deref(),
      record.project_name.as_deref(),
      file_path,
      first_ts,
      last_ts,
      stamp.mtime_ns.div_euclid(1_000_000_000),
      file_id,
      session_hash,
    ],
  )?;
  Ok(tx.last_insert_rowid())
}

fn update_session(
  tx: &Transaction<'_>,
  rowid: i64,
  source: &str,
  file_path: &str,
  stamp: FileStamp,
  record: &UsageRecord,
  first_ts: &str,
  last_ts: &str,
  session_hash: &[u8],
) -> Result<()> {
  tx.execute(
    "UPDATE sessions SET \
       source = ?1, session_id = ?2, session_kind = ?3, parent_session_id = ?4, session_title = ?5, \
       project_cwd = ?6, project_name = ?7, file_path = ?8, first_ts = ?9, last_ts = ?10, file_mtime = ?11, \
       session_hash = ?12 \
     WHERE id = ?13",
    params![
      source,
      record.session_id.as_str(),
      record.session_kind.as_str(),
      record.parent_session_id.as_deref(),
      record.session_title.as_deref(),
      record.project_cwd.as_deref(),
      record.project_name.as_deref(),
      file_path,
      first_ts,
      last_ts,
      stamp.mtime_ns.div_euclid(1_000_000_000),
      session_hash,
      rowid,
    ],
  )?;
  Ok(())
}

fn reconcile_session_records(
  tx: &Transaction<'_>,
  session_rowid: i64,
  parsed_records: &[&ParsedUsageRecord],
) -> Result<()> {
  let mut existing = HashMap::new();
  {
    let mut stmt = tx.prepare(
      "SELECT id, origin_key, record_hash \
       FROM records \
       WHERE session_rowid = ?1",
    )?;
    let rows = stmt.query_map(params![session_rowid], |row| {
      Ok((
        row.get::<_, i64>(0)?,
        row.get::<_, Option<String>>(1)?,
        row.get::<_, Option<Vec<u8>>>(2)?,
      ))
    })?;
    for row in rows {
      let (rowid, origin_key, record_hash) = row?;
      let Some(origin_key) = origin_key else {
        anyhow::bail!("cache contains a record without an origin key");
      };
      let Some(record_hash) = record_hash else {
        anyhow::bail!("cache contains a record without a fingerprint");
      };
      if existing.insert(origin_key, (rowid, record_hash)).is_some() {
        anyhow::bail!("cache contains duplicate record origin keys");
      }
    }
  }

  for parsed in parsed_records {
    let fingerprint = record_fingerprint(&parsed.record)?;
    match existing.remove(&parsed.origin_key) {
      Some((_, existing_hash)) if existing_hash == fingerprint => {}
      Some((rowid, _)) => update_record(tx, rowid, parsed, &fingerprint)?,
      None => insert_record(tx, session_rowid, parsed, &fingerprint)?,
    }
  }

  for (rowid, _) in existing.into_values() {
    tx.execute("DELETE FROM records WHERE id = ?1", params![rowid])?;
  }
  Ok(())
}

fn insert_record(
  tx: &Transaction<'_>,
  session_rowid: i64,
  parsed: &ParsedUsageRecord,
  fingerprint: &[u8],
) -> Result<()> {
  let record = &parsed.record;
  tx.execute(
    "INSERT INTO records (session_rowid, provider, model, ts, prompt, completion, input_bytes, output_bytes, \
                         input_estimated, output_estimated, input_bytes_estimated, output_bytes_estimated, \
                         reasoning, cache_read, cache_write, total, mode, agent, is_compaction, rounds, calls, \
                         cost_embedded, origin_key, record_hash) \
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24)",
    params![
      session_rowid,
      record.provider.as_deref(),
      record.model.as_deref(),
      record.ts.to_rfc3339(),
      to_sql_i64(record.prompt),
      to_sql_i64(record.completion),
      to_sql_i64(record.input_bytes),
      to_sql_i64(record.output_bytes),
      bool_to_sql(record.input_estimated),
      bool_to_sql(record.output_estimated),
      bool_to_sql(record.input_bytes_estimated),
      bool_to_sql(record.output_bytes_estimated),
      to_sql_i64(record.reasoning),
      to_sql_i64(record.cache_read),
      to_sql_i64(record.cache_write),
      record.total_direct.map(to_sql_i64),
      record.mode.as_deref(),
      record.agent.as_deref(),
      bool_to_sql(record.is_compaction),
      to_sql_i64(record.rounds),
      to_sql_i64(record.calls),
      record.cost_embedded,
      parsed.origin_key.as_str(),
      fingerprint,
    ],
  )?;
  Ok(())
}

fn update_record(tx: &Transaction<'_>, rowid: i64, parsed: &ParsedUsageRecord, fingerprint: &[u8]) -> Result<()> {
  let record = &parsed.record;
  tx.execute(
    "UPDATE records SET \
       provider = ?1, model = ?2, ts = ?3, prompt = ?4, completion = ?5, input_bytes = ?6, output_bytes = ?7, \
       input_estimated = ?8, output_estimated = ?9, input_bytes_estimated = ?10, output_bytes_estimated = ?11, \
       reasoning = ?12, cache_read = ?13, cache_write = ?14, total = ?15, mode = ?16, agent = ?17, \
       is_compaction = ?18, rounds = ?19, calls = ?20, cost_embedded = ?21, record_hash = ?22 \
     WHERE id = ?23",
    params![
      record.provider.as_deref(),
      record.model.as_deref(),
      record.ts.to_rfc3339(),
      to_sql_i64(record.prompt),
      to_sql_i64(record.completion),
      to_sql_i64(record.input_bytes),
      to_sql_i64(record.output_bytes),
      bool_to_sql(record.input_estimated),
      bool_to_sql(record.output_estimated),
      bool_to_sql(record.input_bytes_estimated),
      bool_to_sql(record.output_bytes_estimated),
      to_sql_i64(record.reasoning),
      to_sql_i64(record.cache_read),
      to_sql_i64(record.cache_write),
      record.total_direct.map(to_sql_i64),
      record.mode.as_deref(),
      record.agent.as_deref(),
      bool_to_sql(record.is_compaction),
      to_sql_i64(record.rounds),
      to_sql_i64(record.calls),
      record.cost_embedded,
      fingerprint,
      rowid,
    ],
  )?;
  Ok(())
}

fn delete_session_rows(tx: &Transaction<'_>, session_rowid: i64) -> Result<()> {
  tx.execute("DELETE FROM records WHERE session_rowid = ?1", params![session_rowid])?;
  tx.execute("DELETE FROM sessions WHERE id = ?1", params![session_rowid])?;
  Ok(())
}

fn delete_pruned_file_rows(tx: &Transaction<'_>, source: &str, file_path: &str) -> Result<()> {
  tx.execute(
    "DELETE FROM records WHERE session_rowid IN ( \
       SELECT id FROM sessions WHERE source = ?1 AND file_path = ?2 AND pruned != 0 \
     )",
    params![source, file_path],
  )?;
  tx.execute(
    "DELETE FROM sessions WHERE source = ?1 AND file_path = ?2 AND pruned != 0",
    params![source, file_path],
  )?;
  Ok(())
}

fn delete_file_rows(tx: &Transaction<'_>, source: &str, file_path: &str) -> Result<usize> {
  tx.execute(
    "DELETE FROM records WHERE session_rowid IN (SELECT id FROM sessions WHERE source = ?1 AND file_path = ?2)",
    params![source, file_path],
  )?;
  let sessions = tx.execute(
    "DELETE FROM sessions WHERE source = ?1 AND file_path = ?2",
    params![source, file_path],
  )?;
  tx.execute(
    "DELETE FROM files WHERE source = ?1 AND file_path = ?2",
    params![source, file_path],
  )?;
  Ok(sessions)
}

fn remove_cache_files(path: &Path) {
  let _ = std::fs::remove_file(path);
  let _ = std::fs::remove_file(format!("{}-wal", path.display()));
  let _ = std::fs::remove_file(format!("{}-shm", path.display()));
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
  let sql = format!("PRAGMA table_info({table})");
  let mut stmt = conn.prepare(&sql)?;
  let columns = stmt
    .query_map([], |row| row.get::<_, String>(1))?
    .filter_map(|row| row.ok())
    .collect();
  Ok(columns)
}

fn columns_match(actual: &[String], expected: &[&str]) -> bool {
  actual.len() == expected.len() && actual.iter().zip(expected).all(|(actual, expected)| actual == expected)
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
    .map(|timestamp| timestamp.with_timezone(&Utc))
    .unwrap_or_else(|_| Utc.timestamp_opt(0, 0).single().unwrap_or_else(Utc::now));
  UsageRecord {
    source,
    session_id: row.get(1).unwrap_or_default(),
    session_kind: match row.get::<_, String>(2).as_deref() {
      Ok("sub_agent") => SessionKind::SubAgent,
      _ => SessionKind::Root,
    },
    parent_session_id: row.get(3).unwrap_or(None),
    session_title: row.get(4).unwrap_or(None),
    project_cwd: row.get(5).unwrap_or(None),
    project_name: row.get(6).unwrap_or(None),
    provider: row.get(7).unwrap_or(None),
    model: row.get(8).unwrap_or(None),
    ts,
    prompt: row.get::<_, i64>(10).ok().map(from_sql_i64).unwrap_or(0),
    completion: row.get::<_, i64>(11).ok().map(from_sql_i64).unwrap_or(0),
    input_bytes: row.get::<_, i64>(12).ok().map(from_sql_i64).unwrap_or(0),
    output_bytes: row.get::<_, i64>(13).ok().map(from_sql_i64).unwrap_or(0),
    input_estimated: row.get::<_, i64>(14).unwrap_or(0) != 0,
    output_estimated: row.get::<_, i64>(15).unwrap_or(0) != 0,
    input_bytes_estimated: row.get::<_, i64>(16).unwrap_or(0) != 0,
    output_bytes_estimated: row.get::<_, i64>(17).unwrap_or(0) != 0,
    reasoning: row.get::<_, i64>(18).ok().map(from_sql_i64).unwrap_or(0),
    cache_read: row.get::<_, i64>(19).ok().map(from_sql_i64).unwrap_or(0),
    cache_write: row.get::<_, i64>(20).ok().map(from_sql_i64).unwrap_or(0),
    total_direct: row.get::<_, Option<i64>>(21).unwrap_or(None).map(from_sql_i64),
    mode: row.get(22).unwrap_or(None),
    agent: row.get(23).unwrap_or(None),
    is_compaction: row.get::<_, i64>(24).unwrap_or(0) != 0,
    rounds: row.get::<_, i64>(25).ok().map(from_sql_i64).unwrap_or(0),
    calls: row.get::<_, i64>(26).ok().map(from_sql_i64).unwrap_or(0),
    cost_embedded: row.get(27).unwrap_or(None),
  }
}

fn bool_to_sql(value: bool) -> i64 {
  if value {
    1
  } else {
    0
  }
}

fn to_sql_i64(value: u64) -> i64 {
  i64::try_from(value).unwrap_or(i64::MAX)
}

fn from_sql_i64(value: i64) -> u64 {
  u64::try_from(value).unwrap_or(0)
}

#[cfg(test)]
mod tests {
  use super::*;

  const V8_SCHEMA: &str = "\
CREATE TABLE sessions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source TEXT NOT NULL,
    session_id TEXT NOT NULL,
    session_kind TEXT NOT NULL,
    parent_session_id TEXT,
    session_title TEXT,
    project_cwd TEXT,
    project_name TEXT,
    file_path TEXT NOT NULL,
    first_ts TEXT NOT NULL,
    last_ts TEXT NOT NULL,
    file_mtime INTEGER NOT NULL,
    pruned INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE records (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_rowid INTEGER NOT NULL REFERENCES sessions(id),
    provider TEXT,
    model TEXT,
    ts TEXT NOT NULL,
    prompt INTEGER NOT NULL,
    completion INTEGER NOT NULL,
    input_bytes INTEGER NOT NULL,
    output_bytes INTEGER NOT NULL,
    input_estimated INTEGER NOT NULL,
    output_estimated INTEGER NOT NULL,
    input_bytes_estimated INTEGER NOT NULL,
    output_bytes_estimated INTEGER NOT NULL,
    reasoning INTEGER NOT NULL,
    cache_read INTEGER NOT NULL,
    cache_write INTEGER NOT NULL,
    total INTEGER,
    mode TEXT,
    agent TEXT,
    is_compaction INTEGER NOT NULL,
    rounds INTEGER NOT NULL,
    calls INTEGER NOT NULL,
    cost_embedded REAL
);\
";

  fn cache() -> CacheDb {
    let conn = Connection::open_in_memory().expect("open in-memory cache");
    conn
      .pragma_update(None, "foreign_keys", "ON")
      .expect("enable foreign keys");
    conn.execute_batch(SCHEMA).expect("create cache schema");
    CacheDb { conn }
  }

  fn stamp(sequence: i64) -> FileStamp {
    FileStamp {
      mtime_ns: sequence * 1_000_000,
      file_size: 100 + sequence,
      aux_mtime_ns: -1,
      aux_size: -1,
    }
  }

  #[test]
  fn sqlite_stamp_changes_when_a_wal_appears() {
    let unique = std::time::SystemTime::now()
      .duration_since(std::time::UNIX_EPOCH)
      .expect("system clock after epoch")
      .as_nanos();
    let directory = std::env::temp_dir().join(format!("llm-tokei-cache-stamp-{}-{unique}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("create temporary cache stamp directory");
    let database = directory.join("opencode.db");
    std::fs::write(&database, "database").expect("write database");

    let without_wal = FileStamp::from_sqlite_database(&database).expect("stamp database without wal");
    std::fs::write(format!("{}-wal", database.display()), "wal").expect("write wal");
    let with_wal = FileStamp::from_sqlite_database(&database).expect("stamp database with wal");
    let _ = std::fs::remove_dir_all(directory);

    assert_ne!(without_wal, with_wal);
  }

  fn record(session_id: &str, second: i64, prompt: u64) -> UsageRecord {
    UsageRecord {
      source: Source::Codex,
      session_id: session_id.to_string(),
      session_kind: SessionKind::Root,
      parent_session_id: None,
      session_title: None,
      project_cwd: None,
      project_name: None,
      provider: Some("openai".to_string()),
      model: Some("gpt-5.6-sol".to_string()),
      ts: Utc
        .timestamp_opt(1_700_000_000 + second, 0)
        .single()
        .expect("valid timestamp"),
      prompt,
      completion: 0,
      input_bytes: 0,
      output_bytes: 0,
      input_estimated: false,
      output_estimated: false,
      input_bytes_estimated: false,
      output_bytes_estimated: false,
      reasoning: 0,
      cache_read: 0,
      cache_write: 0,
      total_direct: None,
      mode: None,
      agent: None,
      is_compaction: false,
      rounds: 1,
      calls: 1,
      cost_embedded: None,
    }
  }

  fn parsed(records: impl IntoIterator<Item = (&'static str, UsageRecord)>) -> ParsedUsageFile {
    ParsedUsageFile::new(
      true,
      records
        .into_iter()
        .map(|(origin_key, record)| ParsedUsageRecord {
          origin_key: origin_key.to_string(),
          record,
        })
        .collect(),
    )
  }

  fn count(conn: &Connection, table: &str) -> i64 {
    conn
      .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row.get(0))
      .expect("count table rows")
  }

  fn record_id(conn: &Connection, origin_key: &str) -> i64 {
    conn
      .query_row(
        "SELECT id FROM records WHERE origin_key = ?1",
        params![origin_key],
        |row| row.get(0),
      )
      .expect("load record id")
  }

  fn audit_record_writes(conn: &Connection) {
    conn
      .execute_batch(
        "CREATE TABLE record_audit (operation TEXT NOT NULL, origin_key TEXT NOT NULL);
         CREATE TRIGGER record_insert_audit AFTER INSERT ON records
         BEGIN INSERT INTO record_audit VALUES ('insert', NEW.origin_key); END;
         CREATE TRIGGER record_update_audit AFTER UPDATE ON records
         BEGIN INSERT INTO record_audit VALUES ('update', NEW.origin_key); END;
         CREATE TRIGGER record_delete_audit AFTER DELETE ON records
         BEGIN INSERT INTO record_audit VALUES ('delete', OLD.origin_key); END;",
      )
      .expect("install record audit triggers");
  }

  fn record_audit(conn: &Connection) -> Vec<(String, String)> {
    let mut stmt = conn
      .prepare("SELECT operation, origin_key FROM record_audit ORDER BY rowid")
      .expect("prepare record audit query");
    stmt
      .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
      .expect("query record audit")
      .map(|row| row.expect("read record audit"))
      .collect()
  }

  #[test]
  fn unchanged_records_are_not_rewritten_when_a_file_stamp_changes() {
    let mut db = cache();
    let file = Path::new("/cache/target.jsonl");
    let snapshot = parsed([
      ("event-a", record("session", 1, 10)),
      ("event-b", record("session", 2, 20)),
    ]);
    db.upsert_file(file, stamp(1), "codex", &snapshot).expect("seed cache");
    audit_record_writes(&db.conn);

    db.upsert_file(file, stamp(2), "codex", &snapshot)
      .expect("reconcile identical records");

    assert!(record_audit(&db.conn).is_empty());
    assert_eq!(count(&db.conn, "sessions"), 1);
    assert_eq!(count(&db.conn, "records"), 2);
    assert_eq!(
      db.file_stamps_for("codex").expect("load file stamps").get(file),
      Some(&stamp(2))
    );
  }

  #[test]
  fn reconciliation_updates_only_changed_records_and_inserts_new_events() {
    let mut db = cache();
    let file = Path::new("/cache/target.jsonl");
    db.upsert_file(
      file,
      stamp(1),
      "codex",
      &parsed([
        ("event-a", record("session", 1, 10)),
        ("event-b", record("session", 2, 20)),
      ]),
    )
    .expect("seed cache");
    let event_a_id = record_id(&db.conn, "event-a");
    let event_b_id = record_id(&db.conn, "event-b");
    audit_record_writes(&db.conn);

    db.upsert_file(
      file,
      stamp(2),
      "codex",
      &parsed([
        ("event-a", record("session", 1, 10)),
        ("event-b", record("session", 2, 25)),
        ("event-c", record("session", 3, 30)),
      ]),
    )
    .expect("reconcile changed cache");

    assert_eq!(record_id(&db.conn, "event-a"), event_a_id);
    assert_eq!(record_id(&db.conn, "event-b"), event_b_id);
    assert_eq!(
      record_audit(&db.conn),
      vec![
        ("update".to_string(), "event-b".to_string()),
        ("insert".to_string(), "event-c".to_string()),
      ]
    );
    assert_eq!(count(&db.conn, "sessions"), 1);
    assert_eq!(count(&db.conn, "records"), 3);
  }

  #[test]
  fn reconciliation_deletes_events_and_sessions_absent_from_a_complete_snapshot() {
    let mut db = cache();
    let file = Path::new("/cache/target.jsonl");
    db.upsert_file(
      file,
      stamp(1),
      "codex",
      &parsed([
        ("event-a", record("session-a", 1, 10)),
        ("event-b", record("session-b", 2, 20)),
      ]),
    )
    .expect("seed cache");
    audit_record_writes(&db.conn);

    db.upsert_file(
      file,
      stamp(2),
      "codex",
      &parsed([("event-a", record("session-a", 1, 10))]),
    )
    .expect("reconcile removal");

    assert_eq!(
      record_audit(&db.conn),
      vec![("delete".to_string(), "event-b".to_string())]
    );
    assert_eq!(count(&db.conn, "sessions"), 1);
    assert_eq!(count(&db.conn, "records"), 1);
  }

  #[test]
  fn distinct_origin_keys_preserve_equal_usage_records() {
    let mut db = cache();
    let file = Path::new("/cache/target.jsonl");
    let same = record("session", 1, 10);
    db.upsert_file(
      file,
      stamp(1),
      "codex",
      &parsed([("event-a", same.clone()), ("event-b", same)]),
    )
    .expect("cache equal events");

    assert_eq!(count(&db.conn, "records"), 2);
    assert_eq!(db.load_active_for_file("codex", file).expect("load cache").len(), 2);
  }

  #[test]
  fn incomplete_snapshots_do_not_change_cached_records() {
    let mut db = cache();
    let file = Path::new("/cache/target.jsonl");
    db.upsert_file(
      file,
      stamp(1),
      "codex",
      &parsed([("event-a", record("session", 1, 10))]),
    )
    .expect("seed cache");
    let incomplete = ParsedUsageFile::new(
      false,
      vec![ParsedUsageRecord {
        origin_key: "event-a".to_string(),
        record: record("session", 1, 20),
      }],
    );

    assert!(db.upsert_file(file, stamp(2), "codex", &incomplete).is_err());
    assert_eq!(
      db.load_active_for_file("codex", file).expect("load cache")[0].prompt,
      10
    );
    assert_eq!(
      db.file_stamps_for("codex").expect("load file stamps").get(file),
      Some(&stamp(1))
    );
  }

  #[test]
  fn failed_reconciliation_rolls_back_all_cache_changes() {
    let mut db = cache();
    let file = Path::new("/cache/target.jsonl");
    db.upsert_file(
      file,
      stamp(1),
      "codex",
      &parsed([("event-a", record("session", 1, 10))]),
    )
    .expect("seed cache");
    db.conn
      .execute_batch(
        "CREATE TRIGGER fail_record_update BEFORE UPDATE ON records
         BEGIN SELECT RAISE(ABORT, 'injected failure'); END;",
      )
      .expect("install failure trigger");

    assert!(db
      .upsert_file(
        file,
        stamp(2),
        "codex",
        &parsed([("event-a", record("session", 1, 20))]),
      )
      .is_err());

    assert_eq!(
      db.load_active_for_file("codex", file).expect("load cache")[0].prompt,
      10
    );
    assert_eq!(
      db.file_stamps_for("codex").expect("load file stamps").get(file),
      Some(&stamp(1))
    );
  }

  #[test]
  fn an_unkeyed_legacy_file_is_rebuilt_once() {
    let mut db = cache();
    let file = Path::new("/cache/target.jsonl");
    db.upsert_file(
      file,
      stamp(1),
      "codex",
      &parsed([("event-a", record("session", 1, 10))]),
    )
    .expect("seed cache");
    let legacy_record_id = record_id(&db.conn, "event-a");
    db.conn
      .execute_batch(
        "UPDATE sessions SET session_hash = NULL; UPDATE records SET origin_key = NULL, record_hash = NULL",
      )
      .expect("simulate v8 rows");

    db.upsert_file(
      file,
      stamp(2),
      "codex",
      &parsed([("event-b", record("session", 2, 20))]),
    )
    .expect("replace legacy file");

    assert_eq!(count(&db.conn, "sessions"), 1);
    assert_eq!(count(&db.conn, "records"), 1);
    assert_ne!(record_id(&db.conn, "event-b"), legacy_record_id);
    assert_eq!(
      db.load_active_for_file("codex", file).expect("load cache")[0].prompt,
      20
    );
  }

  #[test]
  fn prune_removes_legacy_history_and_preserves_active_rows() {
    let mut db = cache();
    let legacy = Path::new("/cache/legacy.jsonl");
    let active = Path::new("/cache/active.jsonl");
    db.upsert_file(
      legacy,
      stamp(1),
      "codex",
      &parsed([("event-legacy", record("legacy", 1, 10))]),
    )
    .expect("seed legacy cache");
    db.upsert_file(
      active,
      stamp(1),
      "codex",
      &parsed([("event-active", record("active", 2, 20))]),
    )
    .expect("seed active cache");
    db.conn
      .execute(
        "UPDATE sessions SET pruned = 1 WHERE source = 'codex' AND file_path = ?1",
        params![legacy.to_string_lossy()],
      )
      .expect("simulate legacy history");

    let stats = db.prune().expect("prune legacy cache");

    assert_eq!(stats.sessions, 1);
    assert_eq!(stats.records, 1);
    assert_eq!(count(&db.conn, "sessions"), 1);
    assert_eq!(count(&db.conn, "records"), 1);
    assert_eq!(
      db.load_active_for_file("codex", active).expect("load active cache")[0].prompt,
      20
    );
  }

  #[test]
  fn migrates_a_v8_cache_without_scanning_or_dropping_its_records() {
    let mut conn = Connection::open_in_memory().expect("open v8 cache");
    conn
      .pragma_update(None, "foreign_keys", "ON")
      .expect("enable foreign keys");
    conn.execute_batch(V8_SCHEMA).expect("create v8 schema");
    conn.pragma_update(None, "user_version", 8).expect("mark v8 schema");
    conn.execute(
      "INSERT INTO sessions (source, session_id, session_kind, file_path, first_ts, last_ts, file_mtime, pruned)
       VALUES ('codex', 'session', 'root', '/cache/legacy.jsonl', '2024-01-01T00:00:00+00:00', '2024-01-01T00:00:00+00:00', 100, 0)",
      [],
    )
    .expect("insert v8 session");
    let session_rowid = conn.last_insert_rowid();
    conn.execute(
      "INSERT INTO records (session_rowid, ts, prompt, completion, input_bytes, output_bytes, input_estimated, output_estimated, input_bytes_estimated, output_bytes_estimated, reasoning, cache_read, cache_write, is_compaction, rounds, calls)
       VALUES (?1, '2024-01-01T00:00:00+00:00', 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1)",
      params![session_rowid],
    )
    .expect("insert v8 record");

    assert!(CacheDb::can_migrate_v8(&conn).expect("recognize v8 cache"));
    CacheDb::migrate_v8(&mut conn).expect("migrate v8 cache");

    assert!(!CacheDb::needs_recreate(&conn).expect("validate v9 schema"));
    assert_eq!(count(&conn, "records"), 1);
    assert_eq!(count(&conn, "files"), 1);
    let file_id: Option<i64> = conn
      .query_row("SELECT file_id FROM sessions", [], |row| row.get(0))
      .expect("load migrated file id");
    assert!(file_id.is_some());
  }
}
