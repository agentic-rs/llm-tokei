use serde_json::Value;

pub trait Counter {
  fn count(&self, s: &str) -> u64;
}

pub struct Chars;

impl Counter for Chars {
  fn count(&self, s: &str) -> u64 {
    s.chars().count() as u64
  }
}

pub struct Bytes;

impl Counter for Bytes {
  fn count(&self, s: &str) -> u64 {
    s.len() as u64
  }
}

pub fn count_value<C: Counter>(counter: &C, value: &Value) -> u64 {
  match value {
    Value::String(s) => counter.count(s),
    Value::Array(items) => items.iter().map(|item| count_value(counter, item)).sum(),
    Value::Object(map) => map.values().map(|item| count_value(counter, item)).sum(),
    _ => 0,
  }
}
