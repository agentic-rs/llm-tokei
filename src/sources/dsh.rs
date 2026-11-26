use crate::model::{ParsedUsageFile, ParsedUsageRecord, SessionKind, Source, UsageRecord};
use crate::sources::{read_jsonl_reader_with_status, summarize_records, UsageSource};
use anyhow::{Context, Result};
use chrono::{TimeZone, Utc};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use tracing::debug;
use walkdir::WalkDir;

pub struct DshSource {
  pub root: PathBuf,
}

impl DshSource {
  pub fn new(root: PathBuf) -> Self {
    Self { root }
  }

  pub fn default_path() -> Option<PathBuf> {
    let home = std::env::var_os("DSH_HOME").map(PathBuf::from).or_else(|| {
      std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|path| path.join(".dsh"))
    })?;
    Some(home.join("sessions"))
  }

  pub fn discover_files(&self) -> Vec<PathBuf> {
    if !self.root.exists() {
      return Vec::new();
    }
    WalkDir::new(&self.root)
      .follow_links(false)
      .into_iter()
      .filter_map(|entry| entry.ok())
      .filter(|entry| entry.file_type().is_file())
      .filter_map(|entry| {
        let path = entry.path();
        matches!(
          path.file_name().and_then(|name| name.to_str()),
          Some("session.jsonl" | "session.jsonl.zstd")
        )
        .then(|| path.to_path_buf())
      })
      .collect()
  }

  pub fn parse_file(path: &Path) -> Result<Option<Vec<UsageRecord>>> {
    let records = Self::parse_cache_file(path)?.into_usage_records();
    Ok((!records.is_empty()).then_some(records))
  }

  pub fn parse_cache_file(path: &Path) -> Result<ParsedUsageFile> {
    let file = File::open(path).with_context(|| format!("opening DSH session {}", path.display()))?;
    if path.file_name().and_then(|name| name.to_str()) == Some("session.jsonl.zstd") {
      let decoder = zstd::stream::read::Decoder::new(file)
        .with_context(|| format!("opening Zstandard DSH session {}", path.display()))?;
      parse_session(BufReader::new(decoder), path)
    } else {
      parse_session(BufReader::new(file), path)
    }
  }
}

impl UsageSource for DshSource {
  fn name(&self) -> &'static str {
    "dsh"
  }

  fn collect(&self) -> Result<Vec<UsageRecord>> {
    let mut out = Vec::new();
    for path in self.discover_files() {
      debug!(source = "dsh", file = %path.display(), "processing file");
      if let Ok(Some(records)) = Self::parse_file(&path) {
        debug!(
          source = "dsh",
          file = %path.display(),
          summary = %summarize_records(&records),
          "file summary"
        );
        out.extend(records);
      }
    }
    Ok(out)
  }
}

#[derive(Debug, Deserialize)]
struct Line {
  #[serde(rename = "type")]
  kind: String,
  #[serde(default)]
  time: Option<i64>,
  #[serde(default)]
  version: Option<u64>,
  #[serde(default)]
  id: Option<String>,
  #[serde(default, rename = "createdAt")]
  created_at: Option<i64>,
  #[serde(default)]
  cwd: Option<String>,
  #[serde(default, rename = "parentSession")]
  parent_session: Option<String>,
  #[serde(default)]
  origin: Option<String>,
  #[serde(default, rename = "delegationDepth")]
  delegation_depth: Option<u64>,
  #[serde(default, rename = "agentPreset")]
  agent_preset: Option<String>,
  #[serde(default)]
  data: Option<Value>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
struct TokenUsage {
  #[serde(default, rename = "inputTokens")]
  input_tokens: u64,
  #[serde(default, rename = "outputTokens")]
  output_tokens: u64,
  #[serde(default, rename = "cacheReadTokens")]
  cache_read_tokens: u64,
  #[serde(default, rename = "cacheWriteTokens")]
  cache_write_tokens: u64,
  #[serde(default, rename = "reasoningTokens")]
  reasoning_tokens: u64,
}

#[derive(Debug, Clone)]
struct UsageSample {
  time: i64,
  turn: u64,
  step: u64,
  provider: Option<String>,
  model: Option<String>,
  mode: Option<String>,
  usage: TokenUsage,
}

#[derive(Debug, Default)]
struct SessionMeta {
  id: Option<String>,
  created_at: Option<i64>,
  cwd: Option<String>,
  parent_session: Option<String>,
  is_subagent: bool,
  agent_preset: Option<String>,
  title: Option<String>,
}

fn parse_session<R: Read>(reader: BufReader<R>, path: &Path) -> Result<ParsedUsageFile> {
  let mut meta = SessionMeta::default();
  let mut samples = BTreeMap::<(u64, u64), UsageSample>::new();
  let mut current_provider: Option<String> = None;
  let mut current_model: Option<String> = None;
  let mut current_mode: Option<String> = None;
  let mut format_version = None;

  let complete = read_jsonl_reader_with_status::<Line, _, _>(reader, |line, _position| {
    if line.kind == "session" {
      meta.id = line.id;
      meta.created_at = line.created_at;
      meta.cwd = line.cwd;
      meta.parent_session = line.parent_session;
      meta.is_subagent = line.origin.as_deref() == Some("subagent") || line.delegation_depth.unwrap_or(0) > 0;
      meta.agent_preset = line.agent_preset;
      format_version = Some(line.version);
      return;
    }

    let Some(data) = line.data else {
      return;
    };
    if line.kind == "session/title" {
      meta.title = string_at(&data, &["title"]);
      return;
    }
    if line.kind == "request/header" {
      current_provider = string_at(&data, &["header", "config", "provider"]);
      current_model = string_at(&data, &["header", "config", "model"]);
      current_mode = string_at(&data, &["header", "config", "reasoningEffort"]);
      return;
    }

    let Some((turn, step)) = turn_step(&data) else {
      return;
    };
    let usage = match line.kind.as_str() {
      "assistant/chunk" if string_at(&data, &["chunk", "type"]).as_deref() == Some("usage") => {
        value_at(&data, &["chunk", "usage"])
      }
      "assistant/message" => value_at(&data, &["usage"]),
      _ => None,
    };
    let Some(usage) = usage.and_then(|value| serde_json::from_value::<TokenUsage>(value.clone()).ok()) else {
      return;
    };

    let provider = if line.kind == "assistant/message" {
      string_at(&data, &["message", "source", "provider"])
        .or_else(|| string_at(&data, &["provenance", "provider"]))
        .or_else(|| current_provider.clone())
    } else {
      current_provider.clone()
    };
    let model = if line.kind == "assistant/message" {
      string_at(&data, &["message", "source", "model"])
        .or_else(|| string_at(&data, &["provenance", "model"]))
        .or_else(|| current_model.clone())
    } else {
      current_model.clone()
    };
    samples.insert(
      (turn, step),
      UsageSample {
        time: line.time.or(meta.created_at).unwrap_or(0),
        turn,
        step,
        provider,
        model,
        mode: current_mode.clone(),
        usage,
      },
    );
  })?;

  match format_version {
    Some(Some(0)) => {}
    Some(Some(version)) => {
      anyhow::bail!("unsupported DSH session format version {version}; this build reads version 0");
    }
    Some(None) => anyhow::bail!("DSH session header is missing its format version"),
    None => anyhow::bail!("DSH session log is missing its session header"),
  }

  let session_id = meta.id.unwrap_or_else(|| session_id_from_path(path));
  let session_kind = if meta.is_subagent {
    SessionKind::SubAgent
  } else {
    SessionKind::Root
  };
  let mut previous_turn = None;
  let records = samples
    .into_values()
    .map(|sample| {
      let rounds = u64::from(previous_turn != Some(sample.turn));
      previous_turn = Some(sample.turn);
      let reasoning = sample.usage.reasoning_tokens.min(sample.usage.output_tokens);
      ParsedUsageRecord {
        origin_key: format!("turn:{}:step:{}", sample.turn, sample.step),
        record: UsageRecord {
          source: Source::Dsh,
          session_id: session_id.clone(),
          session_kind,
          parent_session_id: meta.parent_session.clone(),
          session_title: meta.title.clone(),
          project_cwd: meta.cwd.clone(),
          project_name: None,
          provider: sample.provider,
          model: sample.model,
          service_tier: None,
          ts: Utc
            .timestamp_millis_opt(sample.time)
            .single()
            .unwrap_or_else(|| Utc.timestamp_opt(0, 0).single().unwrap_or_else(Utc::now)),
          prompt: sample.usage.input_tokens,
          completion: sample.usage.output_tokens.saturating_sub(reasoning),
          input_bytes: 0,
          output_bytes: 0,
          input_estimated: false,
          output_estimated: false,
          input_bytes_estimated: true,
          output_bytes_estimated: true,
          reasoning,
          cache_read: sample.usage.cache_read_tokens,
          cache_write: sample.usage.cache_write_tokens,
          total_direct: None,
          mode: sample.mode,
          agent: meta.agent_preset.clone(),
          is_compaction: false,
          rounds,
          calls: 1,
          cost_embedded: None,
        },
      }
    })
    .collect();

  Ok(ParsedUsageFile::new(complete, records))
}

fn turn_step(data: &Value) -> Option<(u64, u64)> {
  Some((
    value_at(data, &["turn"])?.as_u64()?,
    value_at(data, &["step"])?.as_u64()?,
  ))
}

fn value_at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
  path.iter().try_fold(value, |current, key| current.get(key))
}

fn string_at(value: &Value, path: &[&str]) -> Option<String> {
  value_at(value, path)?
    .as_str()
    .filter(|value| !value.is_empty())
    .map(str::to_string)
}

fn session_id_from_path(path: &Path) -> String {
  path
    .parent()
    .and_then(Path::file_name)
    .and_then(|name| name.to_str())
    .unwrap_or("unknown")
    .to_string()
}

#[cfg(test)]
mod tests {
  use super::*;

  fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
      .join("tests/fixtures/dsh")
      .join(name)
  }

  #[test]
  fn parses_plaintext_and_replaces_usage_chunk_with_committed_message() {
    let parsed = DshSource::parse_cache_file(&fixture("plain/session.jsonl")).expect("parse DSH fixture");
    assert!(parsed.complete);
    assert_eq!(parsed.records.len(), 2);

    let first = &parsed.records[0].record;
    assert_eq!(first.source, Source::Dsh);
    assert_eq!(first.session_id, "session-fixture");
    assert_eq!(first.session_title.as_deref(), Some("Fixture session"));
    assert_eq!(first.project_cwd.as_deref(), Some("/workspace/project"));
    assert_eq!(first.provider.as_deref(), Some("deepseek-official"));
    assert_eq!(first.model.as_deref(), Some("deepseek-v4-flash"));
    assert_eq!(first.prompt, 100);
    assert_eq!(first.completion, 20);
    assert_eq!(first.reasoning, 5);
    assert_eq!(first.cache_read, 30);
    assert_eq!(first.rounds, 1);
    assert_eq!(first.calls, 1);

    let second = &parsed.records[1].record;
    assert_eq!(second.prompt, 40);
    assert_eq!(second.completion, 10);
    assert_eq!(second.reasoning, 0);
    assert_eq!(second.rounds, 0);
  }

  #[test]
  fn parses_default_zstandard_session_logs() {
    let parsed =
      DshSource::parse_cache_file(&fixture("compressed/session.jsonl.zstd")).expect("parse compressed fixture");
    assert!(parsed.complete);
    assert_eq!(parsed.records.len(), 2);
    assert_eq!(parsed.records[0].record.prompt, 100);
  }
}
