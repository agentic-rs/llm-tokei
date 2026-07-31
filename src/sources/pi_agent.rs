use crate::model::{ParsedUsageFile, ParsedUsageRecord, Source, UsageRecord};
use crate::sources::{read_jsonl_with_status, summarize_records, JsonlPosition, UsageSource};
use crate::text_count::{all_strings, BytesSink, SpanSink, StatsSink, TextSpan};
use anyhow::Result;
use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use tokn_pi_protocol::{PiSessionItem, PiSessionLine};
use tracing::debug;
use walkdir::WalkDir;

pub struct PiAgentSource {
  pub root: PathBuf,
}

impl PiAgentSource {
  pub fn new(root: PathBuf) -> Self {
    Self { root }
  }

  pub fn default_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    Some(home.join(".pi/agent/sessions"))
  }

  pub fn discover_files(&self) -> Vec<PathBuf> {
    if !self.root.exists() {
      return Vec::new();
    }
    WalkDir::new(&self.root)
      .follow_links(false)
      .into_iter()
      .filter_map(|e| e.ok())
      .filter(|e| e.file_type().is_file())
      .filter_map(|entry| {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str())?;
        if name.ends_with(".jsonl") {
          Some(path.to_path_buf())
        } else {
          None
        }
      })
      .collect()
  }

  pub fn parse_file(path: &Path) -> Result<Option<Vec<UsageRecord>>> {
    let records = Self::parse_cache_file(path)?.into_usage_records();
    Ok(if records.is_empty() { None } else { Some(records) })
  }

  pub fn parse_cache_file(path: &Path) -> Result<ParsedUsageFile> {
    parse_session(path)
  }
}

impl UsageSource for PiAgentSource {
  fn name(&self) -> &'static str {
    "pi-agent"
  }

  fn collect(&self) -> Result<Vec<UsageRecord>> {
    let mut out = Vec::new();
    for path in self.discover_files() {
      debug!(source = "pi-agent", file = %path.display(), "processing file");
      if let Ok(Some(recs)) = Self::parse_file(&path) {
        debug!(
          source = "pi-agent",
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

#[derive(Debug, Deserialize)]
struct UsageMessage {
  #[serde(default)]
  role: Option<String>,
  #[serde(default)]
  content: Option<Value>,
  #[serde(default)]
  details: Option<Value>,
  #[serde(default)]
  provider: Option<String>,
  #[serde(default)]
  model: Option<String>,
  #[serde(default)]
  usage: Option<Usage>,
  #[serde(default)]
  api: Option<String>,
  #[serde(default, rename = "responseId")]
  response_id: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct Details {
  #[serde(default, rename = "curatedQueries")]
  curated_queries: Vec<CuratedQuery>,
  #[serde(default)]
  summary: Option<Summary>,
}

#[derive(Debug, Deserialize, Default)]
struct CuratedQuery {
  #[serde(default)]
  query: String,
  #[serde(default)]
  provider: Option<String>,
  #[serde(default)]
  answer: String,
  #[serde(default)]
  sources: Vec<SummarySource>,
  #[serde(default)]
  results: Vec<SummarySource>,
  #[serde(default)]
  error: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct SummarySource {
  #[serde(default)]
  title: String,
  #[serde(default)]
  url: String,
}

#[derive(Debug, Deserialize, Default)]
struct Summary {
  #[serde(default)]
  text: String,
  #[serde(default)]
  workflow: Option<String>,
  #[serde(default)]
  model: Option<String>,
  #[serde(default, rename = "tokenEstimate")]
  token_estimate: u64,
  #[serde(default, rename = "fallbackUsed")]
  fallback_used: bool,
}

#[derive(Debug, Deserialize, Default)]
struct Usage {
  #[serde(default)]
  input: u64,
  #[serde(default)]
  output: u64,
  #[serde(default, rename = "cacheRead")]
  cache_read: u64,
  #[serde(default, rename = "cacheWrite")]
  cache_write: u64,
  #[serde(default, rename = "totalTokens")]
  total_tokens: Option<u64>,
  #[serde(default)]
  cost: Option<Cost>,
}

#[derive(Debug, Deserialize, Default)]
struct Cost {
  #[serde(default)]
  total: Option<f64>,
}

struct PendingTurn {
  origin_key: String,
  ts: DateTime<Utc>,
  provider: Option<String>,
  model: Option<String>,
  usage: Usage,
  bytes: BytesSink,
  rounds: u64,
  mode: Option<String>,
  agent: Option<String>,
}

struct PluginTurn {
  origin_key: String,
  ts: DateTime<Utc>,
  provider: Option<String>,
  model: Option<String>,
  prompt: u64,
  completion: u64,
  input_bytes: u64,
  output_bytes: u64,
  mode: Option<String>,
  agent: Option<String>,
}

fn parse_session(path: &Path) -> Result<ParsedUsageFile> {
  let mut session_id = file_stem_or(path, "unknown");
  let mut cwd: Option<String> = None;
  let mut current_provider: Option<String> = None;
  let mut current_model: Option<String> = None;
  let mut pending_bytes = BytesSink::default();
  let mut pending_rounds = 0u64;
  let mut pending: Vec<PendingTurn> = Vec::new();
  let mut plugin_turns: Vec<PluginTurn> = Vec::new();

  let complete = read_jsonl_with_status::<PiSessionLine, _>(path, |line, position| {
    let ts = parse_ts(line.timestamp());
    match line.item() {
      PiSessionItem::Session(header) => {
        if let Some(id) = &header.id {
          session_id = id.clone();
        }
        if cwd.is_none() {
          cwd = header.cwd.clone();
        }
      }
      PiSessionItem::ModelChange(change) => {
        current_provider = change.provider.clone();
        current_model = change.model_id.clone();
      }
      PiSessionItem::Message(item) => {
        if item.message.is_none() {
          return;
        }
        let Some(message) = extract_usage_message(line.native().get("message")) else {
          return;
        };
        let event_id = item.id.as_deref().or(message.response_id.as_deref());
        let role = message.role.as_deref();
        if role == Some("user") {
          pending_rounds = pending_rounds.saturating_add(1);
        }
        visit_message(role, message.content.as_ref(), &mut pending_bytes);

        if let Some(plugin_turn) = plugin_summary_turn(
          &message,
          ts,
          item.parent_id.as_deref(),
          origin_key(event_id, position, 1),
        ) {
          plugin_turns.push(plugin_turn);
        }

        let Some(usage) = message.usage else {
          return;
        };
        pending.push(PendingTurn {
          origin_key: origin_key(event_id, position, 0),
          ts,
          provider: message.provider.or_else(|| current_provider.clone()),
          model: message.model.or_else(|| current_model.clone()),
          usage,
          bytes: pending_bytes.take(),
          rounds: std::mem::take(&mut pending_rounds),
          mode: message.api,
          agent: message.response_id.or_else(|| item.parent_id.clone()),
        });
      }
      _ => {}
    }
  })?;

  if pending.is_empty() && plugin_turns.is_empty() {
    return Ok(ParsedUsageFile::new(complete, Vec::new()));
  }
  if !pending.is_empty() && pending.iter().all(|t| t.rounds == 0) {
    pending[0].rounds = 1;
  }

  let mut records: Vec<ParsedUsageRecord> = pending
    .into_iter()
    .map(|turn| ParsedUsageRecord {
      origin_key: turn.origin_key,
      record: UsageRecord {
        source: Source::PiAgent,
        session_id: session_id.clone(),
        session_kind: crate::model::SessionKind::Root,
        parent_session_id: None,
        session_title: None,
        project_cwd: cwd.clone(),
        project_name: None,
        provider: turn.provider,
        model: turn.model,
        ts: turn.ts,
        prompt: turn.usage.input,
        completion: turn.usage.output,
        input_bytes: turn.bytes.input,
        output_bytes: turn.bytes.output.saturating_add(turn.bytes.reasoning),
        input_estimated: false,
        output_estimated: false,
        input_bytes_estimated: true,
        output_bytes_estimated: true,
        reasoning: 0,
        cache_read: turn.usage.cache_read,
        cache_write: turn.usage.cache_write,
        total_direct: turn.usage.total_tokens,
        mode: turn.mode,
        agent: turn.agent,
        is_compaction: false,
        rounds: turn.rounds,
        calls: 1,
        cost_embedded: turn.usage.cost.and_then(|c| c.total),
      },
    })
    .collect();

  records.extend(plugin_turns.into_iter().map(|turn| ParsedUsageRecord {
    origin_key: turn.origin_key,
    record: UsageRecord {
      source: Source::PiAgent,
      session_id: session_id.clone(),
      session_kind: crate::model::SessionKind::Root,
      parent_session_id: None,
      session_title: None,
      project_cwd: cwd.clone(),
      project_name: None,
      provider: turn.provider,
      model: turn.model,
      ts: turn.ts,
      prompt: turn.prompt,
      completion: turn.completion,
      input_bytes: turn.input_bytes,
      output_bytes: turn.output_bytes,
      input_estimated: true,
      output_estimated: true,
      input_bytes_estimated: true,
      output_bytes_estimated: true,
      reasoning: 0,
      cache_read: 0,
      cache_write: 0,
      total_direct: None,
      mode: turn.mode,
      agent: turn.agent,
      is_compaction: false,
      rounds: 0,
      calls: 1,
      cost_embedded: None,
    },
  }));

  Ok(ParsedUsageFile::new(complete, records))
}

fn origin_key(event_id: Option<&str>, position: JsonlPosition, emitted_slot: usize) -> String {
  match event_id.filter(|id| !id.is_empty()) {
    Some(id) => format!("event:{id}:slot:{emitted_slot}"),
    None => format!("offset:{}:slot:{emitted_slot}", position.byte_offset),
  }
}

fn extract_usage_message(value: Option<&Value>) -> Option<UsageMessage> {
  serde_json::from_value(value?.clone()).ok()
}

fn plugin_summary_turn(
  message: &UsageMessage,
  ts: DateTime<Utc>,
  parent_id: Option<&str>,
  origin_key: String,
) -> Option<PluginTurn> {
  let details = serde_json::from_value::<Details>(message.details.clone()?).ok()?;
  let summary = details.summary.as_ref()?;
  if summary.fallback_used {
    return None;
  }
  if summary.workflow.as_deref() != Some("summary-review") {
    return None;
  }
  let model_name = summary.model.as_deref()?;
  let (provider, model) = split_provider_model(model_name);
  let prompt = build_summary_prompt(&details.curated_queries, None);
  let prompt = prompt.trim();
  let completion_text = summary.text.trim();
  Some(PluginTurn {
    origin_key,
    ts,
    provider,
    model,
    prompt: estimate_text_tokens(prompt),
    completion: summary.token_estimate,
    input_bytes: prompt.len() as u64,
    output_bytes: completion_text.len() as u64,
    mode: summary.workflow.clone(),
    agent: parent_id.map(str::to_string),
  })
}

fn split_provider_model(model: &str) -> (Option<String>, Option<String>) {
  if let Some((provider, model)) = model.split_once('/') {
    (Some(provider.to_string()), Some(model.to_string()))
  } else {
    (None, Some(model.to_string()))
  }
}

fn estimate_text_tokens(text: &str) -> u64 {
  let len = text.trim().chars().count() as u64;
  if len == 0 {
    0
  } else {
    len.div_ceil(4).max(1)
  }
}

fn build_summary_prompt(results: &[CuratedQuery], feedback: Option<&str>) -> String {
  let mut sections = vec![
    "You are writing the final web search summary for a coding assistant.".to_string(),
    "Write a concise, factual summary using only the provided search results.".to_string(),
    "Requirements:".to_string(),
    "- Keep it readable and skimmable.".to_string(),
    "- Include key findings and caveats.".to_string(),
    "- Do not invent sources or claims.".to_string(),
    "- If evidence is weak or conflicting, say so explicitly.".to_string(),
    "- End with a short \"Sources\" section listing the most relevant URLs.".to_string(),
  ];

  if feedback.is_some() {
    sections.push("- Incorporate the user feedback provided below into the summary.".to_string());
  }

  sections.push(String::new());
  sections.push("<search_results>".to_string());

  for (i, result) in results.iter().enumerate() {
    sections.push(format!("\n[Result {}]", i + 1));
    sections.push(summarize_query_result(result));
  }

  sections.push("\n</search_results>".to_string());

  if let Some(feedback) = feedback {
    sections.push(String::new());
    sections.push("<user_feedback>".to_string());
    sections.push(feedback.to_string());
    sections.push("</user_feedback>".to_string());
  }

  sections.join("\n")
}

fn summarize_query_result(result: &CuratedQuery) -> String {
  if let Some(error) = result.error.as_deref() {
    return format!("Query: {}\nStatus: Error\nError: {error}", result.query);
  }

  let mut lines = vec![
    format!("Query: {}", result.query),
    format!("Provider: {}", result.provider.as_deref().unwrap_or("unknown")),
    format!(
      "Answer: {}",
      if result.answer.is_empty() {
        "(no answer text returned)"
      } else {
        &result.answer
      }
    ),
  ];

  let sources = if result.results.is_empty() {
    &result.sources
  } else {
    &result.results
  };

  if sources.is_empty() {
    lines.push("Sources: none".to_string());
  } else {
    lines.push("Sources:".to_string());
    for (i, source) in sources.iter().enumerate() {
      lines.push(format!("{}. {} — {}", i + 1, source.title, source.url));
    }
  }

  lines.join("\n")
}

fn visit_message(role: Option<&str>, content: Option<&Value>, sink: &mut BytesSink) {
  let Some(role) = role.and_then(normalize_role) else {
    return;
  };
  let stats = all_strings::<StatsSink>(content);
  sink.text(TextSpan::new(role, "").with_stats(stats));
}

fn normalize_role(role: &str) -> Option<&'static str> {
  match role {
    "user" => Some("user"),
    "assistant" => Some("assistant"),
    "system" => Some("system"),
    "toolResult" => Some("tool_call_result"),
    _ => None,
  }
}

fn parse_ts(s: Option<&str>) -> DateTime<Utc> {
  s.and_then(|s| DateTime::parse_from_rfc3339(s).ok())
    .map(|dt| dt.with_timezone(&Utc))
    .unwrap_or_else(|| Utc.timestamp_opt(0, 0).single().unwrap_or_else(Utc::now))
}

fn file_stem_or(path: &Path, fallback: &str) -> String {
  path
    .file_stem()
    .and_then(|s| s.to_str())
    .unwrap_or(fallback)
    .to_string()
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn cache_parser_uses_pi_event_ids_and_distinct_emitted_slots() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
      .join("tests/fixtures/pi_agent/sessions/--tmp-proj--/2026-05-27T01-02-03-004Z_abc123.jsonl");

    let parsed = PiAgentSource::parse_cache_file(&path).expect("parse fixture");
    let keys = parsed
      .records
      .iter()
      .map(|record| record.origin_key.clone())
      .collect::<Vec<_>>();

    assert!(parsed.complete);
    assert_eq!(
      keys,
      vec![
        "event:a1:slot:0".to_string(),
        "event:a2:slot:0".to_string(),
        "event:t2:slot:1".to_string(),
      ]
    );
    assert_eq!(
      keys,
      PiAgentSource::parse_cache_file(&path)
        .expect("reparse fixture")
        .records
        .iter()
        .map(|record| record.origin_key.clone())
        .collect::<Vec<_>>()
    );
  }

  #[test]
  fn protocol_parser_skips_malformed_records_and_keeps_later_usage() {
    let path = std::env::temp_dir().join(format!("llm-tokei-pi-protocol-{}.jsonl", std::process::id()));
    std::fs::write(
      &path,
      concat!(
        r#"{"type":"session","id":"pi-session","timestamp":"2026-08-01T00:00:00Z","cwd":"/tmp/project"}"#,
        "\n",
        r#"{"type":"model_change","id":42,"provider":"openai","modelId":"gpt-5"}"#,
        "\n",
        r#"{"type":"message","id":"assistant-1","timestamp":"2026-08-01T00:01:00Z","message":{"role":"assistant","provider":"openai","model":"gpt-5","content":[{"type":"text","text":"hello"}],"details":"future-extension","usage":{"input":5,"output":3,"cacheRead":1,"cacheWrite":2,"totalTokens":8}}}"#,
        "\n"
      ),
    )
    .expect("write temporary Pi session");

    let parsed = parse_session(&path).expect("parse tolerant protocol session");
    std::fs::remove_file(&path).expect("remove temporary Pi session");

    assert_eq!(parsed.records.len(), 1);
    let record = &parsed.records[0].record;
    assert_eq!(record.session_id, "pi-session");
    assert_eq!(record.provider.as_deref(), Some("openai"));
    assert_eq!(record.model.as_deref(), Some("gpt-5"));
    assert_eq!(record.prompt, 5);
    assert_eq!(record.completion, 3);
  }
}
