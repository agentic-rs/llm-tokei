use crate::model::{Source, UsageRecord};
use crate::text_count::{SpanSink, TokenSpan, TokenStatsSink};
use chrono::{TimeZone, Utc};
use serde_json::Value;
use std::path::Path;

pub struct ShutdownRecordArgs<'a> {
  pub source: Source,
  pub source_path: &'a Path,
  pub session_id: Option<String>,
  pub project_cwd: Option<String>,
  pub project_name: Option<String>,
  pub event: &'a Value,
}

pub fn records_from_shutdown_model_metrics(args: ShutdownRecordArgs<'_>) -> Vec<UsageRecord> {
  if args.event.get("type").and_then(|v| v.as_str()) != Some("session.shutdown") {
    return Vec::new();
  }
  let Some(metrics) = args.event.pointer("/data/modelMetrics").and_then(|v| v.as_object()) else {
    return Vec::new();
  };

  let session_id = args
    .session_id
    .or_else(|| {
      args
        .event
        .pointer("/data/sessionId")
        .and_then(|v| v.as_str())
        .map(str::to_string)
    })
    .unwrap_or_else(|| {
      args
        .source_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string()
    });
  let ts = timestamp_from_event(args.event);

  metrics
    .iter()
    .map(|(model, metric)| {
      let usage = metric.get("usage").unwrap_or(&Value::Null);
      let tokens = token_stats_from_shutdown_usage(usage);
      let (provider, normalized_model) = normalize_copilot_model(model.clone());
      UsageRecord {
        source: args.source,
        session_id: session_id.clone(),
        session_title: None,
        project_cwd: args.project_cwd.clone(),
        project_name: args.project_name.clone(),
        provider: Some(provider),
        model: Some(normalized_model),
        ts,
        prompt: tokens.prompt,
        completion: tokens.completion,
        input_bytes: 0,
        output_bytes: 0,
        input_estimated: false,
        output_estimated: false,
        input_bytes_estimated: true,
        output_bytes_estimated: true,
        reasoning: tokens.reasoning,
        cache_read: tokens.cache_read,
        cache_write: tokens.cache_write,
        total_direct: None,
        mode: Some("session.shutdown".to_string()),
        agent: None,
        is_compaction: false,
        rounds: metric.pointer("/requests/count").and_then(|v| v.as_u64()).unwrap_or(1),
        calls: metric.pointer("/requests/count").and_then(|v| v.as_u64()).unwrap_or(1),
        cost_embedded: None,
      }
    })
    .collect()
}

fn token_stats_from_shutdown_usage(usage: &Value) -> crate::text_count::TokenUsageStats {
  let mut sink = TokenStatsSink::default();
  let reasoning = token(usage, "reasoningTokens");
  sink.token(TokenSpan::usage(
    token(usage, "inputTokens"),
    token(usage, "outputTokens").saturating_sub(reasoning),
    reasoning,
    token(usage, "cacheReadTokens"),
    token(usage, "cacheWriteTokens"),
    usage.get("totalTokens").and_then(|v| v.as_u64()),
  ));
  sink.usage
}

pub fn normalize_copilot_model(model: String) -> (String, String) {
  match model.split_once('/') {
    Some((provider, rest)) if !rest.is_empty() => {
      let normalized_provider = if provider == "copilot" {
        "github-copilot".to_string()
      } else {
        provider.to_string()
      };
      (normalized_provider, rest.to_string())
    }
    _ => ("github-copilot".to_string(), model),
  }
}

pub fn timestamp_from_event(event: &Value) -> chrono::DateTime<Utc> {
  event
    .get("timestamp")
    .and_then(|v| v.as_str())
    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
    .map(|dt| dt.with_timezone(&Utc))
    .unwrap_or_else(|| Utc.timestamp_opt(0, 0).single().unwrap_or_else(Utc::now))
}

fn token(usage: &Value, key: &str) -> u64 {
  usage.get(key).and_then(|v| v.as_u64()).unwrap_or(0)
}
