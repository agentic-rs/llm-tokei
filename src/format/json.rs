use crate::aggregate::{Aggregate, GroupDim};
use crate::cli::Unit;
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Serialize)]
struct JsonRow<'a> {
  keys: serde_json::Map<String, serde_json::Value>,
  input: serde_json::Value,
  output: serde_json::Value,
  input_estimated: bool,
  output_estimated: bool,
  reasoning: serde_json::Value,
  cache_read: serde_json::Value,
  cache_write: serde_json::Value,
  total: serde_json::Value,
  calls: u64,
  rounds: u64,
  sessions: u64,
  root_sessions: u64,
  sub_agent_sessions: u64,
  cost_embedded: f64,
  cost: f64,
  #[serde(skip_serializing_if = "BTreeMap::is_empty")]
  cost_per: &'a BTreeMap<String, f64>,
  first_ts: Option<&'a chrono::DateTime<chrono::Utc>>,
  last_ts: Option<&'a chrono::DateTime<chrono::Utc>>,
}

pub fn render_json(aggs: &[Aggregate], dims: &[GroupDim], unit: Unit) -> String {
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
        input: json_input(a, unit),
        output: json_output(a, unit),
        input_estimated: if unit == Unit::Bytes {
          a.input_bytes_estimated
        } else {
          a.input_estimated
        },
        output_estimated: if unit == Unit::Bytes {
          a.output_bytes_estimated
        } else {
          a.output_estimated
        },
        reasoning: json_reasoning(a, unit),
        cache_read: json_cache_read(a, unit),
        cache_write: json_cache_write(a, unit),
        total: json_total(a, unit),
        calls: a.calls,
        rounds: a.rounds,
        sessions: a.sessions,
        root_sessions: a.root_sessions,
        sub_agent_sessions: a.sub_agent_sessions,
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

fn json_input(a: &Aggregate, unit: Unit) -> serde_json::Value {
  match unit {
    Unit::Tokens => serde_json::Value::from(a.input),
    Unit::Bytes => serde_json::Value::from(a.input_bytes),
    Unit::Cost => serde_json::Value::from(a.prompt_cost + a.cache_read_cost + a.cache_write_cost),
  }
}

fn json_output(a: &Aggregate, unit: Unit) -> serde_json::Value {
  match unit {
    Unit::Tokens => serde_json::Value::from(a.output),
    Unit::Bytes => serde_json::Value::from(a.output_bytes),
    Unit::Cost => serde_json::Value::from(a.completion_cost + a.reasoning_cost),
  }
}

fn json_reasoning(a: &Aggregate, unit: Unit) -> serde_json::Value {
  match unit {
    Unit::Cost => serde_json::Value::from(a.reasoning_cost),
    _ => serde_json::Value::from(a.reasoning),
  }
}

fn json_cache_read(a: &Aggregate, unit: Unit) -> serde_json::Value {
  match unit {
    Unit::Cost => serde_json::Value::from(a.cache_read_cost),
    _ => serde_json::Value::from(a.cache_read),
  }
}

fn json_cache_write(a: &Aggregate, unit: Unit) -> serde_json::Value {
  match unit {
    Unit::Cost => serde_json::Value::from(a.cache_write_cost),
    _ => serde_json::Value::from(a.cache_write),
  }
}

fn json_total(a: &Aggregate, unit: Unit) -> serde_json::Value {
  match unit {
    Unit::Cost => serde_json::Value::from(a.cost),
    _ => serde_json::Value::from(a.total),
  }
}
