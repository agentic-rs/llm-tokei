use crate::aggregate::{Aggregate, GroupDim};
use serde::Serialize;

#[derive(Serialize)]
struct JsonRow<'a> {
    keys: serde_json::Map<String, serde_json::Value>,
    input: u64,
    output: u64,
    reasoning: u64,
    cache_read: u64,
    cache_write: u64,
    total: u64,
    turns: u64,
    cost_embedded: f64,
    cost_estimated: f64,
    first_ts: Option<&'a chrono::DateTime<chrono::Utc>>,
    last_ts: Option<&'a chrono::DateTime<chrono::Utc>>,
}

pub fn render_json(aggs: &[Aggregate], dims: &[GroupDim]) -> String {
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
                input: a.input,
                output: a.output,
                reasoning: a.reasoning,
                cache_read: a.cache_read,
                cache_write: a.cache_write,
                total: a.total,
                turns: a.turns,
                cost_embedded: a.cost_embedded,
                cost_estimated: a.cost_estimated,
                first_ts: a.first_ts.as_ref(),
                last_ts: a.last_ts.as_ref(),
            }
        })
        .collect();
    serde_json::to_string_pretty(&rows).unwrap_or_else(|_| "[]".into())
}
