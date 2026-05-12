use crate::aggregate::{Aggregate, GroupDim};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Serialize)]
struct JsonRow<'a> {
  keys: serde_json::Map<String, serde_json::Value>,
  input: u64,
  output: u64,
  input_estimated: bool,
  output_estimated: bool,
  reasoning: u64,
  cache_read: u64,
  cache_write: u64,
  total: u64,
  turns: u64,
  rounds: u64,
  sessions: u64,
  cost_embedded: f64,
  cost: f64,
  #[serde(skip_serializing_if = "BTreeMap::is_empty")]
  cost_per: &'a BTreeMap<String, f64>,
  first_ts: Option<&'a chrono::DateTime<chrono::Utc>>,
  last_ts: Option<&'a chrono::DateTime<chrono::Utc>>,
}

pub fn render_json(aggs: &[Aggregate], dims: &[GroupDim], use_bytes: bool) -> String {
  let rows: Vec<JsonRow> = aggs
    .iter()
    .map(|a| {
      let mut keys = serde_json::Map::new();
      for (i, d) in dims.iter().enumerate() {
        keys.insert(
          d.label().to_string(),
          serde_json::Value::String(a.keys.get(i).cloned().unwrap_or_default()),
        );
      }
      JsonRow {
        keys,
        input: if use_bytes { a.input_bytes } else { a.input },
        output: if use_bytes { a.output_bytes } else { a.output },
        input_estimated: if use_bytes {
          a.input_bytes_estimated
        } else {
          a.input_estimated
        },
        output_estimated: if use_bytes {
          a.output_bytes_estimated
        } else {
          a.output_estimated
        },
        reasoning: a.reasoning,
        cache_read: a.cache_read,
        cache_write: a.cache_write,
        total: a.total,
        turns: a.turns,
        rounds: a.rounds,
        sessions: a.sessions,
        cost_embedded: a.cost_embedded,
        cost: a.cost,
        cost_per: &a.cost_per,
        first_ts: a.first_ts.as_ref(),
        last_ts: a.last_ts.as_ref(),
      }
    })
    .collect();
  serde_json::to_string_pretty(&rows).unwrap_or_else(|_| "[]".into())
}
