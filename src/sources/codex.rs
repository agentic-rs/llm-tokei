use crate::model::{ParsedUsageFile, ParsedUsageRecord, ServiceTier, SessionKind, Source, UsageRecord};
use crate::sources::dump::{DumpSink, DumpedSession};
use crate::sources::{read_jsonl_reader_with_status, summarize_records, JsonlPosition, UsageSource};
use crate::text_count::{
  all_strings, json_serialized_or_string, message_content, nested_fields, BytesSink, SpanSink, StatsSink, StringSink,
  TextSpan, TextStats, TokenSpan, TokenStatsSink, TokenUsageStats,
};
use anyhow::Result;
use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{hash_map::Entry, HashMap, HashSet};
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use tokn_codex_protocol::{ResponseItem, RolloutItem, RolloutLine, SessionMetaItem, TurnContextItem};
use tracing::debug;
use walkdir::WalkDir;

pub struct CodexSource {
  roots: Vec<PathBuf>,
}

impl CodexSource {
  pub fn from_override(root: Option<PathBuf>) -> Option<Self> {
    if let Some(root) = root {
      return Some(Self { roots: vec![root] });
    }
    let base = std::env::var_os("CODEX_HOME")
      .map(PathBuf::from)
      .or_else(|| std::env::var_os("HOME").map(PathBuf::from).map(|p| p.join(".codex")))?;
    Some(Self {
      roots: vec![base.join("sessions"), base.join("archived_sessions")],
    })
  }

  pub fn discover_files(&self) -> Vec<PathBuf> {
    let mut rollouts = HashMap::new();
    for (root_index, root) in self.roots.iter().enumerate() {
      for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
      {
        if !entry.file_type().is_file() {
          continue;
        }
        let Some((stem, compressed)) = rollout_name(entry.path()) else {
          continue;
        };

        // Codex keeps the unique rollout basename when it moves a thread.
        // Custom filenames can repeat in different directories.
        let directory = if self.roots.len() > 1 && stem.starts_with("rollout-") {
          None
        } else {
          entry.path().parent().map(Path::to_path_buf)
        };
        let key = (directory, stem.to_owned());
        let candidate = ((root_index, compressed), entry.into_path());
        match rollouts.entry(key) {
          Entry::Vacant(slot) => {
            slot.insert(candidate);
          }
          Entry::Occupied(mut slot) if &candidate < slot.get() => {
            slot.insert(candidate);
          }
          _ => {}
        }
      }
    }
    let mut files = rollouts.into_values().map(|(_, path)| path).collect::<Vec<_>>();
    files.sort_unstable();
    files
  }

  pub fn parse_file(path: &Path) -> Result<Option<Vec<UsageRecord>>> {
    let records = Self::parse_cache_file(path)?.into_usage_records();
    Ok(if records.is_empty() { None } else { Some(records) })
  }

  pub fn parse_cache_file(path: &Path) -> Result<ParsedUsageFile> {
    let mut builder = RecordBuilder::new(path);
    let complete = walk_rollout(open_rollout(path)?, &mut builder)?;
    Ok(builder.into_parsed_file(complete))
  }

  pub fn dump_session_messages(path: &Path) -> Result<Option<DumpedSession>> {
    let mut builder = DumpBuilder::new(path);
    let _ = walk_rollout(open_rollout(path)?, &mut builder)?;
    Ok(builder.into_session())
  }
}

impl UsageSource for CodexSource {
  fn name(&self) -> &'static str {
    "codex"
  }

  fn collect(&self) -> Result<Vec<UsageRecord>> {
    let mut out = Vec::new();
    for path in self.discover_files() {
      debug!(source = "codex", file = %path.display(), "processing file");
      if let Ok(Some(recs)) = Self::parse_file(&path) {
        debug!(
          source = "codex",
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

/// Visitor over a Codex rollout JSONL file.
///
/// The walker decodes each JSONL line with `tokn-codex-protocol` and passes
/// the retained native payload to the extractor callbacks that need volatile
/// provider data. High-level callbacks fire before low-level response-item
/// callbacks so consumers can choose their granularity.
trait RolloutVisitor {
  /// Called for every line with a top-level RFC3339 `timestamp` field.
  fn timestamp(&mut self, _ts: DateTime<Utc>) {}

  /// Called for `line.type == "session_meta"`.
  fn session_meta(&mut self, _item: Option<&SessionMetaItem>, _native: &Value) {}

  /// Called for `line.type == "turn_context"`.
  fn turn_context(&mut self, _item: Option<&TurnContextItem>, _native: &Value) {}

  fn thread_settings_applied(&mut self, _payload: &Value) {}

  fn task_started(&mut self, _turn_id: &str) {}

  fn task_finished(&mut self, _turn_id: Option<&str>) {}

  /// Called for `event_msg` records with a `token_count` payload.
  /// Receives the parsed line timestamp and the full event payload.
  fn turn_end(&mut self, _ts: DateTime<Utc>, _payload: &Value, _position: JsonlPosition, _event_id: Option<&str>) {}

  /// Called for every `response_item`, before low-level dispatch.
  fn response_item(&mut self, _payload: &Value) {}

  /// `response_item.payload.type == "message"`; full response-item payload.
  fn message(&mut self, _payload: &Value) {}

  /// `response_item.payload.type == "function_call"`; full response-item payload.
  fn function_call(&mut self, _payload: &Value) {}

  /// `response_item.payload.type == "function_call_output"`; full payload.
  fn function_call_output(&mut self, _payload: &Value) {}

  /// `response_item.payload.type == "custom_tool_call"`; full payload.
  fn custom_tool_call(&mut self, _payload: &Value) {}

  /// `response_item.payload.type == "custom_tool_call_output"`; full payload.
  fn custom_tool_call_output(&mut self, _payload: &Value) {}

  /// `response_item.payload.type == "reasoning"`; full payload.
  fn reasoning(&mut self, _payload: &Value) {}
}

fn walk_rollout<R: BufRead, V: RolloutVisitor>(reader: R, visitor: &mut V) -> Result<bool> {
  read_jsonl_reader_with_status(reader, |line: RolloutLine, position| {
    let ts = parse_rfc3339(line.timestamp());
    if let Some(ts) = ts {
      visitor.timestamp(ts);
    }
    match line.item() {
      RolloutItem::SessionMeta(item) => visitor.session_meta(Some(item), line.native()),
      RolloutItem::EventMessage(item) => {
        let payload = &item.native;
        let turn_id = payload.get("turn_id").and_then(Value::as_str);
        match item.event_type.as_deref() {
          Some("thread_settings_applied") => visitor.thread_settings_applied(payload),
          Some("task_started" | "turn_started") => {
            if let Some(turn_id) = turn_id {
              visitor.task_started(turn_id);
            }
          }
          Some("task_complete" | "turn_complete" | "turn_aborted") => visitor.task_finished(turn_id),
          Some("token_count") => visitor.turn_end(
            ts.unwrap_or_else(epoch_utc),
            payload,
            position,
            source_event_id(line.native()),
          ),
          _ => {}
        }
      }
      RolloutItem::TurnContext(item) => visitor.turn_context(Some(item), line.native()),
      RolloutItem::ResponseItem(item) => {
        let Some(payload) = line.native().get("payload") else {
          return;
        };
        visitor.response_item(payload);
        dispatch_response_item(item, payload, visitor);
      }
      RolloutItem::Unknown(item) => match item.native_type.as_deref() {
        Some("session_meta") => visitor.session_meta(None, line.native()),
        Some("turn_context") => visitor.turn_context(None, line.native()),
        _ => {}
      },
      _ => {}
    }
  })
}

fn open_rollout(path: &Path) -> io::Result<Box<dyn BufRead>> {
  let file = File::open(path)?;
  if rollout_name(path).is_some_and(|(_, compressed)| compressed) {
    Ok(Box::new(BufReader::new(zstd::stream::read::Decoder::new(file)?)))
  } else {
    Ok(Box::new(BufReader::new(file)))
  }
}

fn rollout_name(path: &Path) -> Option<(&str, bool)> {
  let name = path.file_name()?.to_str()?;
  if let Some(stem) = name.strip_suffix(".jsonl.zst") {
    Some((stem, true))
  } else {
    name.strip_suffix(".jsonl").map(|stem| (stem, false))
  }
}

fn dispatch_response_item<V: RolloutVisitor>(item: &ResponseItem, payload: &Value, visitor: &mut V) {
  match item.native_type() {
    Some("message") => visitor.message(payload),
    Some("function_call") => visitor.function_call(payload),
    Some("function_call_output") => visitor.function_call_output(payload),
    Some("custom_tool_call") => visitor.custom_tool_call(payload),
    Some("custom_tool_call_output") => visitor.custom_tool_call_output(payload),
    Some("reasoning") => visitor.reasoning(payload),
    _ => {}
  }
}

fn source_event_id(line: &Value) -> Option<&str> {
  let payload = line.get("payload");
  line
    .get("id")
    .and_then(Value::as_str)
    .or_else(|| line.get("uuid").and_then(Value::as_str))
    .or_else(|| payload.and_then(|value| value.get("id")).and_then(Value::as_str))
    .or_else(|| payload.and_then(|value| value.get("uuid")).and_then(Value::as_str))
}

fn origin_key(event_id: Option<&str>, position: JsonlPosition, emitted_slot: usize) -> String {
  match event_id.filter(|id| !id.is_empty()) {
    Some(id) => format!("event:{id}:slot:{emitted_slot}"),
    None => format!("offset:{}:slot:{emitted_slot}", position.byte_offset),
  }
}

fn parse_rfc3339(s: Option<&str>) -> Option<DateTime<Utc>> {
  DateTime::parse_from_rfc3339(s?).ok().map(|dt| dt.with_timezone(&Utc))
}

fn epoch_utc() -> DateTime<Utc> {
  Utc.timestamp_opt(0, 0).single().unwrap_or_else(Utc::now)
}

// ---------------------------------------------------------------------------
// Session metadata (shared by every visitor)
// ---------------------------------------------------------------------------

#[derive(Default)]
struct SessionMeta {
  session_id: Option<String>,
  forked_from_id: Option<String>,
  cwd: Option<String>,
  model: Option<String>,
  provider: Option<String>,
}

impl SessionMeta {
  fn apply(&mut self, item: Option<&SessionMetaItem>, native: &Value) {
    if let Some(payload) = native.get("payload") {
      let meta = payload.get("meta").unwrap_or(payload);
      let str_field = |key: &str| meta.get(key).and_then(|v| v.as_str()).map(str::to_string);
      if self.session_id.is_none() {
        self.session_id = str_field("id");
      }
      if self.forked_from_id.is_none() {
        self.forked_from_id = str_field("forked_from_id");
      }
      if self.cwd.is_none() {
        self.cwd = str_field("cwd");
      }
      if self.model.is_none() {
        self.model = str_field("model");
      }
      if self.provider.is_none() {
        self.provider = str_field("model_provider").or_else(|| str_field("originator"));
      }
    }

    if let Some(item) = item {
      if self.session_id.is_none() {
        self.session_id = item.id.clone();
      }
      if self.cwd.is_none() {
        self.cwd = item.cwd.clone();
      }
      if self.provider.is_none() {
        self.provider = item.model_provider.clone();
      }
    }

    if self.session_id.is_none() {
      self.session_id = native.get("id").and_then(|v| v.as_str()).map(str::to_string);
    }
    if self.cwd.is_none() {
      self.cwd = native.get("cwd").and_then(|v| v.as_str()).map(str::to_string);
    }
    if self.model.is_none() {
      self.model = native.get("model").and_then(|v| v.as_str()).map(str::to_string);
    }
    if self.provider.is_none() {
      self.provider = native.get("originator").and_then(|v| v.as_str()).map(str::to_string);
    }
  }

  fn resolved_session_id(&self, path: &Path) -> String {
    self.session_id.clone().unwrap_or_else(|| {
      rollout_name(path)
        .map(|(stem, _)| stem)
        .or_else(|| path.file_stem().and_then(|stem| stem.to_str()))
        .unwrap_or("unknown")
        .to_owned()
    })
  }
}

// ---------------------------------------------------------------------------
// Parse visitor: builds UsageRecord per token_count event
// ---------------------------------------------------------------------------

struct RecordBuilder<'a> {
  path: &'a Path,
  meta: SessionMeta,
  session_ts: Option<DateTime<Utc>>,
  last_ts: Option<DateTime<Utc>>,
  prev_total: Option<TokenUsageStats>,
  pending_bytes: BytesSink,
  pending_rounds: u64,
  inherited_turn: bool,
  seen_round_ids: HashSet<String>,
  selected_tier: Option<SelectedTier>,
  tier_turn: Option<TierTurn>,
  calls: Vec<Turn>,
}

#[derive(Clone, PartialEq, Eq)]
struct SelectedTier {
  tier: ServiceTier,
  model: String,
  provider: String,
}

struct TierTurn {
  id: String,
  selection: Option<SelectedTier>,
}

struct Turn {
  ts: DateTime<Utc>,
  model: Option<String>,
  provider: Option<String>,
  service_tier: Option<ServiceTier>,
  tokens: TokenUsageStats,
  bytes: BytesSink,
  rounds: u64,
  origin_key: String,
}

impl<'a> RecordBuilder<'a> {
  fn new(path: &'a Path) -> Self {
    Self {
      path,
      meta: SessionMeta::default(),
      session_ts: None,
      last_ts: None,
      prev_total: None,
      pending_bytes: BytesSink::default(),
      pending_rounds: 0,
      inherited_turn: false,
      seen_round_ids: HashSet::new(),
      selected_tier: None,
      tier_turn: None,
      calls: Vec::new(),
    }
  }

  fn into_parsed_file(mut self, complete: bool) -> ParsedUsageFile {
    if self.calls.is_empty() {
      return ParsedUsageFile::new(complete, Vec::new());
    }
    // If no turn_context events were observed, attribute one round to the
    // first turn so totals match historical behavior.
    if self.calls.iter().all(|t| t.rounds == 0) {
      self.calls[0].rounds = 1;
    }
    let sid = self.meta.resolved_session_id(self.path);
    let records = self
      .calls
      .into_iter()
      .map(|t| ParsedUsageRecord {
        origin_key: t.origin_key,
        record: UsageRecord {
          source: Source::Codex,
          session_id: sid.clone(),
          session_kind: if self.meta.forked_from_id.is_some() {
            SessionKind::SubAgent
          } else {
            SessionKind::Root
          },
          parent_session_id: self.meta.forked_from_id.clone(),
          session_title: None,
          project_cwd: self.meta.cwd.clone(),
          project_name: None,
          provider: t.provider,
          model: t.model,
          service_tier: t.service_tier,
          ts: t.ts,
          prompt: t.tokens.prompt,
          completion: t.tokens.completion,
          input_bytes: t.bytes.input,
          output_bytes: t.bytes.output,
          input_estimated: false,
          output_estimated: false,
          input_bytes_estimated: true,
          output_bytes_estimated: true,
          reasoning: t.tokens.reasoning,
          cache_read: t.tokens.cache_read,
          cache_write: t.tokens.cache_write,
          total_direct: t.tokens.total,
          mode: None,
          agent: None,
          is_compaction: false,
          rounds: t.rounds,
          calls: 1,
          cost_embedded: None,
        },
      })
      .collect();
    ParsedUsageFile::new(complete, records)
  }

  #[cfg(test)]
  fn into_records(self) -> Option<Vec<UsageRecord>> {
    let records = self.into_parsed_file(true).into_usage_records();
    if records.is_empty() {
      None
    } else {
      Some(records)
    }
  }

  fn push_turn(&mut self, ts: DateTime<Utc>, payload: &Value, position: JsonlPosition, event_id: Option<&str>) {
    let Some(tokens) = extract_turn_usage(payload, &mut self.prev_total) else {
      return;
    };
    if self.inherited_turn {
      self.pending_bytes.take();
      self.pending_rounds = 0;
      return;
    }
    let ts = self.last_ts.or(self.session_ts).unwrap_or(ts);
    self.calls.push(Turn {
      ts,
      model: self.meta.model.clone(),
      provider: self.meta.provider.clone(),
      service_tier: self.service_tier(),
      tokens,
      bytes: self.pending_bytes.take(),
      rounds: std::mem::take(&mut self.pending_rounds),
      origin_key: origin_key(event_id, position, 0),
    });
  }

  fn begin_tier_turn(&mut self, turn_id: &str) {
    self.tier_turn = Some(TierTurn {
      id: turn_id.to_owned(),
      selection: self.selected_tier.clone(),
    });
  }

  fn tier_turn_id(&self) -> Option<&str> {
    self.tier_turn.as_ref().map(|turn| turn.id.as_str())
  }

  fn service_tier(&self) -> Option<ServiceTier> {
    let turn = self.tier_turn.as_ref()?;
    // Pre-turn compaction can report usage before Codex persists the new
    // turn context. At that point `meta.model` can still refer to the old turn.
    if !self.seen_round_ids.contains(&turn.id) {
      return None;
    }
    let selection = turn.selection.as_ref()?;
    (self.meta.model.as_deref() == Some(&selection.model) && self.meta.provider.as_deref() == Some(&selection.provider))
      .then_some(selection.tier)
  }
}

impl RolloutVisitor for RecordBuilder<'_> {
  fn timestamp(&mut self, ts: DateTime<Utc>) {
    self.last_ts = Some(ts);
    self.session_ts.get_or_insert(ts);
  }

  fn session_meta(&mut self, item: Option<&SessionMetaItem>, native: &Value) {
    self.meta.apply(item, native);
    self.inherited_turn = self.meta.forked_from_id.is_some();
  }

  fn turn_end(&mut self, ts: DateTime<Utc>, payload: &Value, position: JsonlPosition, event_id: Option<&str>) {
    self.push_turn(ts, payload, position, event_id);
  }

  fn thread_settings_applied(&mut self, payload: &Value) {
    let owned = match payload.get("thread_id") {
      None | Some(Value::Null) => !self.inherited_turn,
      Some(owner) => self
        .meta
        .session_id
        .as_deref()
        .is_some_and(|id| owner.as_str() == Some(id)),
    };
    if !owned {
      return;
    }

    // Each event is a full thread snapshot. Omitted or unsupported tiers end the
    // previous selection for future turns.
    let selection = payload.get("thread_settings").and_then(|settings| {
      Some(SelectedTier {
        tier: ServiceTier::from_codex(settings.get("service_tier")?.as_str()?)?,
        model: settings.get("model")?.as_str()?.to_owned(),
        provider: settings.get("model_provider_id")?.as_str()?.to_owned(),
      })
    });
    if selection != self.selected_tier {
      // Newer Codex versions can apply settings to later steps in an active
      // turn. A settings event does not identify the first affected request.
      if let Some(turn) = &mut self.tier_turn {
        turn.selection = None;
      }
      self.selected_tier = selection;
    }
  }

  fn task_started(&mut self, turn_id: &str) {
    if self.tier_turn_id() != Some(turn_id) {
      self.begin_tier_turn(turn_id);
    }
  }

  fn task_finished(&mut self, turn_id: Option<&str>) {
    if turn_id.is_none() || self.tier_turn_id() == turn_id {
      self.tier_turn = None;
    }
  }

  fn turn_context(&mut self, item: Option<&TurnContextItem>, native: &Value) {
    let payload = native.get("payload");
    let turn_id = item.and_then(|item| item.turn_id.as_deref()).or_else(|| {
      payload
        .and_then(|payload| payload.get("turn_id"))
        .and_then(Value::as_str)
    });
    if let Some(inherited) = is_inherited_fork_turn(&self.meta, turn_id) {
      if self.inherited_turn && !inherited {
        self.pending_bytes.take();
        self.pending_rounds = 0;
      }
      self.inherited_turn = inherited;
    }
    if let Some(model) = item
      .and_then(|item| item.model.as_deref())
      .or_else(|| payload.and_then(|payload| payload.get("model")).and_then(Value::as_str))
    {
      self.meta.model = Some(model.to_string());
    }
    if let Some(p) = payload
      .and_then(|payload| payload.get("model_provider"))
      .and_then(Value::as_str)
    {
      self.meta.provider = Some(p.to_string());
    }
    if self.inherited_turn {
      return;
    }
    // Older histories can lack start events. Repeated context after compaction
    // does not establish a new turn or resolve an ambiguous settings change.
    match turn_id {
      Some(id) if !self.seen_round_ids.contains(id) && self.tier_turn_id() != Some(id) => self.begin_tier_turn(id),
      None => self.tier_turn = None,
      _ => {}
    }
    match turn_id {
      Some(turn_id) if self.seen_round_ids.insert(turn_id.to_string()) => self.pending_rounds += 1,
      Some(_) => {}
      None => self.pending_rounds += 1,
    }
  }

  fn response_item(&mut self, payload: &Value) {
    if let Some(m) = payload.get("model").and_then(|v| v.as_str()) {
      self.meta.model.get_or_insert_with(|| m.to_string());
    }
  }

  fn message(&mut self, payload: &Value) {
    visit_message(payload, &mut self.pending_bytes);
  }

  fn function_call(&mut self, payload: &Value) {
    visit_tool_call(payload, "name", "arguments", &mut self.pending_bytes);
  }

  fn function_call_output(&mut self, payload: &Value) {
    visit_tool_call_output(payload, &mut self.pending_bytes);
  }

  fn custom_tool_call(&mut self, payload: &Value) {
    visit_tool_call(payload, "name", "input", &mut self.pending_bytes);
  }

  fn custom_tool_call_output(&mut self, payload: &Value) {
    visit_tool_call_output(payload, &mut self.pending_bytes);
  }

  fn reasoning(&mut self, payload: &Value) {
    visit_reasoning(payload, &mut self.pending_bytes);
  }
}

/// Forked Codex rollouts begin with a replay of the parent thread. UUIDv7 turn
/// IDs preserve creation order, so turns older than the fork's own session ID
/// belong to that inherited replay and must not be counted again.
fn is_inherited_fork_turn(meta: &SessionMeta, turn_id: Option<&str>) -> Option<bool> {
  let session_id = meta.session_id.as_deref().filter(|_| meta.forked_from_id.is_some())?;
  let turn_id = turn_id?;
  (is_uuid_v7(session_id) && is_uuid_v7(turn_id)).then(|| turn_id < session_id)
}

fn is_uuid_v7(value: &str) -> bool {
  value.len() == 36
    && value.as_bytes().get(14) == Some(&b'7')
    && value.bytes().enumerate().all(|(idx, byte)| match idx {
      8 | 13 | 18 | 23 => byte == b'-',
      _ => byte.is_ascii_hexdigit(),
    })
}

// ---------------------------------------------------------------------------
// Dump visitor: builds DumpedSession
// ---------------------------------------------------------------------------

struct DumpBuilder<'a> {
  path: &'a Path,
  meta: SessionMeta,
  sink: DumpSink,
}

impl<'a> DumpBuilder<'a> {
  fn new(path: &'a Path) -> Self {
    Self {
      path,
      meta: SessionMeta::default(),
      sink: DumpSink::default(),
    }
  }

  fn into_session(self) -> Option<DumpedSession> {
    if self.sink.records.is_empty() {
      return None;
    }
    Some(DumpedSession {
      session_id: self.meta.resolved_session_id(self.path),
      records: self.sink.records,
    })
  }
}

impl RolloutVisitor for DumpBuilder<'_> {
  fn session_meta(&mut self, item: Option<&SessionMetaItem>, native: &Value) {
    self.meta.apply(item, native);
  }

  fn message(&mut self, payload: &Value) {
    let mut once = OnceDump::new(&mut self.sink);
    visit_message(payload, &mut once);
  }

  fn function_call(&mut self, payload: &Value) {
    let mut once = OnceDump::new(&mut self.sink);
    visit_tool_call(payload, "name", "arguments", &mut once);
  }

  fn function_call_output(&mut self, payload: &Value) {
    let mut once = OnceDump::new(&mut self.sink);
    visit_tool_call_output(payload, &mut once);
  }

  fn custom_tool_call(&mut self, payload: &Value) {
    let mut once = OnceDump::new(&mut self.sink);
    visit_tool_call(payload, "name", "input", &mut once);
  }

  fn custom_tool_call_output(&mut self, payload: &Value) {
    let mut once = OnceDump::new(&mut self.sink);
    visit_tool_call_output(payload, &mut once);
  }

  fn reasoning(&mut self, payload: &Value) {
    let mut once = OnceDump::new(&mut self.sink);
    visit_reasoning(payload, &mut once);
  }
}

// ---------------------------------------------------------------------------
// Per-kind response-item extractors. Each writes to an arbitrary SpanSink
// (BytesSink for parse, OnceDump for dump, SpanStatsSink in tests).
// ---------------------------------------------------------------------------

fn visit_message<S: SpanSink>(payload: &Value, sink: &mut S) {
  let Some(role) = message_role(payload) else {
    return;
  };
  let call_id = payload.get("call_id").and_then(|v| v.as_str()).map(str::to_string);
  let text = message_content::<StringSink>(payload.get("content"));
  let stats = message_content::<StatsSink>(payload.get("content"));
  sink.text(TextSpan::new(role, text).with_stats(stats).with_call_id(call_id));
}

/// Extracts a tool-call span from `function_call` or `custom_tool_call`.
/// `name_field` and `body_field` select which JSON keys carry the tool name
/// and arguments/input respectively.
fn visit_tool_call<S: SpanSink>(payload: &Value, name_field: &str, body_field: &str, sink: &mut S) {
  let call_id = payload.get("call_id").and_then(|v| v.as_str()).map(str::to_string);
  let mut stats = string_field_stats(payload, name_field);
  stats.add(all_strings::<StatsSink>(payload.get(body_field)));
  let name = payload.get(name_field).and_then(|v| v.as_str()).unwrap_or("tool");
  let args = json_serialized_or_string::<StringSink>(payload.get(body_field));
  let text = if args.is_empty() {
    name.to_string()
  } else {
    format!("{name}: {args}")
  };
  sink.text(TextSpan::new("tool_call", text).with_stats(stats).with_call_id(call_id));
}

/// Extracts a tool-call-result span from `function_call_output` or
/// `custom_tool_call_output`.
fn visit_tool_call_output<S: SpanSink>(payload: &Value, sink: &mut S) {
  let call_id = payload.get("call_id").and_then(|v| v.as_str()).map(str::to_string);
  let stats = all_strings::<StatsSink>(payload.get("output"));
  let text = nested_fields::<StringSink>(payload.get("output"));
  sink.text(
    TextSpan::new("tool_call_result", text)
      .with_stats(stats)
      .with_call_id(call_id),
  );
}

fn visit_reasoning<S: SpanSink>(payload: &Value, sink: &mut S) {
  if let Some(text) = payload.get("encrypted_content").and_then(|v| v.as_str()) {
    let stats = TextStats::from_str(text);
    sink.text(TextSpan::encrypted("reasoning", text.to_string(), stats));
  }
}

/// SpanSink wrapper that forwards only the first non-empty span to the inner
/// DumpSink, matching the legacy semantics where dump emits at most one record
/// per response_item.
struct OnceDump<'a> {
  inner: &'a mut DumpSink,
  done: bool,
}

impl<'a> OnceDump<'a> {
  fn new(inner: &'a mut DumpSink) -> Self {
    Self { inner, done: false }
  }
}

impl SpanSink for OnceDump<'_> {
  fn text(&mut self, span: TextSpan<'_>) {
    if self.done {
      return;
    }
    if let Some(record) = DumpSink::record_from(span) {
      self.inner.records.push(record);
      self.done = true;
    }
  }
}

fn message_role(payload: &Value) -> Option<&'static str> {
  match payload.get("role").and_then(|v| v.as_str())? {
    "user" => Some("user"),
    "assistant" => Some("assistant"),
    "developer" => Some("developer"),
    "system" => Some("system"),
    _ => None,
  }
}

fn string_field_stats(value: &Value, field: &str) -> TextStats {
  value
    .get(field)
    .and_then(|v| v.as_str())
    .map(TextStats::from_str)
    .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Token-usage delta extraction
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone, Copy, Deserialize)]
struct RawTokenUsage {
  #[serde(default)]
  input_tokens: u64,
  #[serde(default)]
  cached_input_tokens: u64,
  #[serde(default)]
  output_tokens: u64,
  #[serde(default)]
  reasoning_output_tokens: u64,
  #[serde(default)]
  total_tokens: Option<u64>,
}

impl RawTokenUsage {
  fn into_stats(self) -> TokenUsageStats {
    let mut sink = TokenStatsSink::default();
    sink.token(TokenSpan::usage(
      self.input_tokens.saturating_sub(self.cached_input_tokens),
      self.output_tokens.saturating_sub(self.reasoning_output_tokens),
      self.reasoning_output_tokens,
      self.cached_input_tokens,
      0,
      self.total_tokens,
    ));
    sink.usage.total = self.total_tokens;
    sink.usage
  }
}

/// Returns the per-turn delta as `TokenUsageStats`, updating the cumulative
/// snapshot when only `total_token_usage` is present. The `.total` field
/// carries the source-reported direct total when available.
fn extract_turn_usage(payload: &Value, prev_total: &mut Option<TokenUsageStats>) -> Option<TokenUsageStats> {
  let info = payload.get("info").unwrap_or(payload);
  let last = info
    .get("last_token_usage")
    .and_then(|v| serde_json::from_value::<RawTokenUsage>(v.clone()).ok());
  let total = info
    .get("total_token_usage")
    .and_then(|v| serde_json::from_value::<RawTokenUsage>(v.clone()).ok())
    .map(RawTokenUsage::into_stats);

  match (last, total) {
    (Some(usage), total) => {
      if let Some(t) = total {
        *prev_total = Some(t);
      }
      Some(usage.into_stats())
    }
    (None, Some(total)) => {
      let delta = match prev_total {
        Some(prev) => total.sub(*prev),
        None => total,
      };
      *prev_total = Some(total);
      Some(delta)
    }
    (None, None) => None,
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::text_count::SpanStatsSink;

  #[test]
  fn plain_and_compressed_rollouts_share_a_fallback_session_id() {
    let meta = SessionMeta::default();
    for name in [
      "rollout-example.jsonl",
      "rollout-example.jsonl.zst",
      "rollout-example.txt",
    ] {
      assert_eq!(meta.resolved_session_id(Path::new(name)), "rollout-example");
    }
  }

  fn test_position() -> JsonlPosition {
    JsonlPosition {
      byte_offset: 0,
      line_number: 0,
    }
  }

  fn parse_fixture() -> Vec<UsageRecord> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
      .join("tests/fixtures/codex/sessions/2025/01/02/rollout-2025-01-02T10-00-00-test.jsonl");
    CodexSource::parse_file(&path).expect("parse fixture").expect("records")
  }

  fn tier_rollout(lines: &[Value]) -> Vec<Option<ServiceTier>> {
    let jsonl = lines.iter().map(Value::to_string).collect::<Vec<_>>().join("\n");
    let mut builder = RecordBuilder::new(Path::new("rollout-example.jsonl"));
    assert!(walk_rollout(std::io::Cursor::new(jsonl), &mut builder).expect("parse rollout"));
    builder
      .into_parsed_file(true)
      .into_usage_records()
      .into_iter()
      .map(|record| record.service_tier)
      .collect()
  }

  fn rollout_line(kind: &str, payload: Value) -> Value {
    serde_json::json!({ "type": kind, "timestamp": "2026-11-18T01:02:03Z", "payload": payload })
  }

  fn turn_context(id: &str, model: &str) -> Value {
    rollout_line("turn_context", serde_json::json!({ "turn_id": id, "model": model }))
  }

  fn settings(owner: Option<&str>, tier: Option<&str>) -> Value {
    rollout_line(
      "event_msg",
      serde_json::json!({
        "type": "thread_settings_applied", "thread_id": owner,
        "thread_settings": { "model": "gpt-6.2-sol", "model_provider_id": "openai", "service_tier": tier }
      }),
    )
  }

  fn token_count() -> Value {
    rollout_line(
      "event_msg",
      serde_json::json!({
        "type": "token_count",
        "info": { "last_token_usage": { "input_tokens": 10, "output_tokens": 2, "total_tokens": 12 } }
      }),
    )
  }

  #[test]
  fn service_tier_tracks_turn_boundaries_and_full_settings_snapshots() {
    let session = rollout_line(
      "session_meta",
      serde_json::json!({ "id": "example", "model_provider": "openai" }),
    );
    let model = "gpt-6.2-sol";
    let task_event =
      |kind: &str, id: &str| rollout_line("event_msg", serde_json::json!({ "type": kind, "turn_id": id }));
    assert_eq!(
      tier_rollout(&[
        session,
        task_event("task_started", "first"),
        turn_context("first", model),
        token_count(),
        settings(None, Some("fast")),
        token_count(),
        task_event("task_complete", "first"),
        task_event("turn_started", "second"),
        token_count(),
        turn_context("second", model),
        token_count(),
        settings(None, Some("ultrafast")),
        turn_context("second", model),
        token_count(),
        task_event("turn_complete", "second"),
        token_count(),
        turn_context("third", model),
        token_count(),
        settings(None, None),
        turn_context("fourth", model),
        token_count(),
        settings(None, Some("default")),
        turn_context("fifth", "gpt-6.2-astra"),
        token_count(),
        turn_context("sixth", model),
        token_count(),
      ]),
      [
        None,
        None,
        None,
        Some(ServiceTier::Fast),
        None,
        None,
        Some(ServiceTier::Ultrafast),
        None,
        None,
        Some(ServiceTier::Standard),
      ]
    );
  }

  #[test]
  fn forked_rollouts_only_apply_owned_or_unambiguously_local_tier_settings() {
    let parent = "00000000-0001-7000-8000-000000000001";
    let child = "00000000-0002-7000-8000-000000000001";
    let inherited = "00000000-0001-7000-8000-000000000002";
    let first = "00000000-0002-7000-8000-000000000002";
    let second = "00000000-0002-7000-8000-000000000003";
    let third = "00000000-0002-7000-8000-000000000004";
    let model = "gpt-6.2-sol";
    let session = rollout_line(
      "session_meta",
      serde_json::json!({ "id": child, "forked_from_id": parent, "model_provider": "openai" }),
    );
    assert_eq!(
      tier_rollout(&[
        session,
        settings(None, Some("ultrafast")),
        turn_context(inherited, model),
        token_count(),
        settings(Some(child), Some("priority")),
        settings(Some(parent), Some("ultrafast")),
        turn_context(first, model),
        token_count(),
        settings(Some(parent), Some("ultrafast")),
        turn_context(second, model),
        token_count(),
        settings(None, Some("default")),
        turn_context(third, model),
        token_count(),
      ]),
      [
        Some(ServiceTier::Fast),
        Some(ServiceTier::Fast),
        Some(ServiceTier::Standard)
      ]
    );
  }

  #[test]
  fn cache_parser_uses_stable_offset_keys_when_events_have_no_id() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
      .join("tests/fixtures/codex/sessions/2025/01/02/rollout-2025-01-02T10-00-00-test.jsonl");

    let parsed = CodexSource::parse_cache_file(&path).expect("parse fixture");
    let keys = parsed
      .records
      .iter()
      .map(|record| record.origin_key.clone())
      .collect::<Vec<_>>();

    assert!(parsed.complete);
    assert_eq!(keys.len(), 4);
    assert!(keys.iter().all(|key| key.starts_with("offset:")));
    assert_eq!(
      keys,
      CodexSource::parse_cache_file(&path)
        .expect("reparse fixture")
        .records
        .iter()
        .map(|record| record.origin_key.clone())
        .collect::<Vec<_>>()
    );
  }

  #[test]
  fn response_item_bytes_are_attached_to_each_pushed_turn() {
    let records = parse_fixture();

    assert_eq!(records.len(), 4);
    assert_eq!(
      records.iter().map(|r| r.input_bytes).collect::<Vec<_>>(),
      vec![21, 4, 7, 5]
    );
    assert_eq!(
      records.iter().map(|r| r.output_bytes).collect::<Vec<_>>(),
      vec![20, 4, 5, 5]
    );
    // `prompt` stores the uncached portion (input_tokens - cached_input_tokens).
    assert_eq!(
      records.iter().map(|r| r.prompt).collect::<Vec<_>>(),
      vec![60, 60, 120, 60]
    );
    assert_eq!(
      records.iter().map(|r| r.completion).collect::<Vec<_>>(),
      vec![40, 30, 70, 30]
    );
    assert_eq!(
      records.iter().map(|r| r.cache_read).collect::<Vec<_>>(),
      vec![40, 40, 80, 40]
    );
  }

  #[test]
  fn reasoning_response_item_bytes_are_separate_from_output_bytes() {
    let payload = serde_json::json!({
      "type": "reasoning",
      "encrypted_content": "sealed",
      "summary": [{ "type": "summary_text", "text": "ignored" }]
    });
    let mut bytes = BytesSink::default();
    visit_reasoning(&payload, &mut bytes);

    assert_eq!(bytes.input, 0);
    assert_eq!(bytes.output, 0);
    assert_eq!(bytes.reasoning, 6);
    assert_eq!(bytes.total(), 6);
  }

  #[test]
  fn response_item_can_drive_stats_sink_for_aggregate_text() {
    let payload = serde_json::json!({
      "type": "message",
      "role": "assistant",
      "content": [{ "type": "output_text", "text": "hello world" }]
    });
    let mut stats = SpanStatsSink::default();
    visit_message(&payload, &mut stats);
    assert_eq!(stats.stats.bytes, "hello world".len() as u64);
  }

  #[test]
  fn unknown_response_items_keep_legacy_extractor_dispatch() {
    let payload = serde_json::json!({
      "type": "message",
      "role": "assistant",
      "content": "still inspectable"
    });
    let item: ResponseItem = serde_json::from_value(payload.clone()).expect("protocol response item should decode");
    assert!(matches!(item, ResponseItem::Unknown(_)));

    let mut visitor = MessageCounter::default();
    dispatch_response_item(&item, &payload, &mut visitor);
    assert_eq!(visitor.bytes.output, "still inspectable".len() as u64);
  }

  #[test]
  fn forked_rollout_ignores_parent_replay_but_keeps_new_turns() {
    let path = Path::new("forked.jsonl");
    let mut builder = RecordBuilder::new(path);
    let session_meta = serde_json::json!({
      "payload": {
        "id": "019f57cd-7555-7292-a6cc-540fc0df1778",
        "forked_from_id": "019f4a9e-3a88-7a00-9989-2d12dda99487"
      }
    });
    let session_meta_item = test_session_meta_item(&session_meta);
    builder.session_meta(Some(&session_meta_item), &session_meta);

    // Older replay formats can omit turn IDs. Stay in inherited mode until a
    // UUIDv7 turn boundary proves that the fork's own activity has begun.
    let missing_turn_id = serde_json::json!({ "payload": { "model": "gpt-5.6-sol" } });
    let missing_turn_context = test_turn_context_item(&missing_turn_id);
    builder.turn_context(Some(&missing_turn_context), &missing_turn_id);

    // Replayed token events may precede the first inherited turn context.
    builder.turn_end(
      epoch_utc(),
      &serde_json::json!({
        "info": {
          "last_token_usage": {
            "input_tokens": 50,
            "cached_input_tokens": 40,
            "output_tokens": 5,
            "reasoning_output_tokens": 1,
            "total_tokens": 55
          }
        }
      }),
      test_position(),
      None,
    );

    let inherited_turn = serde_json::json!({ "payload": {
      "turn_id": "019f4a9e-7b5c-71c1-b7a5-7b8c6805bc6e",
      "model": "gpt-5.6-sol"
    } });
    let inherited_turn_context = test_turn_context_item(&inherited_turn);
    builder.turn_context(Some(&inherited_turn_context), &inherited_turn);
    builder.message(&serde_json::json!({
      "type": "message",
      "role": "user",
      "content": [{ "type": "input_text", "text": "inherited" }]
    }));
    builder.turn_end(
      epoch_utc(),
      &serde_json::json!({
        "info": {
          "last_token_usage": {
            "input_tokens": 100,
            "cached_input_tokens": 80,
            "output_tokens": 10,
            "reasoning_output_tokens": 2,
            "total_tokens": 110
          }
        }
      }),
      test_position(),
      None,
    );

    let new_turn = serde_json::json!({ "payload": {
      "turn_id": "019f57cd-7c10-7aa1-b465-056defebbe28",
      "model": "gpt-5.6-sol"
    } });
    let new_turn_context = test_turn_context_item(&new_turn);
    builder.turn_context(Some(&new_turn_context), &new_turn);
    builder.turn_context(Some(&new_turn_context), &new_turn);
    builder.message(&serde_json::json!({
      "type": "message",
      "role": "user",
      "content": [{ "type": "input_text", "text": "new" }]
    }));
    builder.turn_end(
      epoch_utc(),
      &serde_json::json!({
        "info": {
          "last_token_usage": {
            "input_tokens": 40,
            "cached_input_tokens": 30,
            "output_tokens": 5,
            "reasoning_output_tokens": 1,
            "total_tokens": 45
          }
        }
      }),
      test_position(),
      None,
    );

    let records = builder.into_records().expect("new fork turn should produce a record");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].session_kind, SessionKind::SubAgent);
    assert_eq!(
      records[0].parent_session_id.as_deref(),
      Some("019f4a9e-3a88-7a00-9989-2d12dda99487")
    );
    assert_eq!(records[0].prompt, 10);
    assert_eq!(records[0].cache_read, 30);
    assert_eq!(records[0].completion, 4);
    assert_eq!(records[0].reasoning, 1);
    assert_eq!(records[0].input_bytes, 3);
    assert_eq!(records[0].rounds, 1);
  }

  fn test_session_meta_item(native: &Value) -> SessionMetaItem {
    serde_json::from_value(native["payload"].clone()).expect("session metadata should decode")
  }

  fn test_turn_context_item(native: &Value) -> TurnContextItem {
    serde_json::from_value(native["payload"].clone()).expect("turn context should decode")
  }

  #[derive(Default)]
  struct MessageCounter {
    bytes: BytesSink,
  }

  impl RolloutVisitor for MessageCounter {
    fn message(&mut self, payload: &Value) {
      visit_message(payload, &mut self.bytes);
    }
  }
}
