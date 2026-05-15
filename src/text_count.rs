use serde_json::Value;

pub trait Counter {
  fn count(&self, s: &str) -> u64;
}

pub trait Extractor: Default {
  type Output;

  fn push(&mut self, s: &str);
  fn finish(self) -> Self::Output;
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

#[derive(Default)]
pub struct CountChars(u64);

impl Extractor for CountChars {
  type Output = u64;

  fn push(&mut self, s: &str) {
    self.0 = self.0.saturating_add(Chars.count(s));
  }

  fn finish(self) -> Self::Output {
    self.0
  }
}

#[derive(Default)]
pub struct CountBytes(u64);

impl Extractor for CountBytes {
  type Output = u64;

  fn push(&mut self, s: &str) {
    self.0 = self.0.saturating_add(Bytes.count(s));
  }

  fn finish(self) -> Self::Output {
    self.0
  }
}

#[derive(Default)]
pub struct JoinString(Vec<String>);

impl Extractor for JoinString {
  type Output = String;

  fn push(&mut self, s: &str) {
    if !s.is_empty() {
      self.0.push(s.to_string());
    }
  }

  fn finish(self) -> Self::Output {
    self.0.join("\n")
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

pub fn extract_text_like<E: Extractor>(value: Option<&Value>) -> E::Output {
  let mut extractor = E::default();
  walk_text_like(value, &mut extractor);
  extractor.finish()
}

pub fn extract_rich_text<E: Extractor>(value: Option<&Value>) -> E::Output {
  let mut extractor = E::default();
  walk_rich_text(value, &mut extractor);
  extractor.finish()
}

pub fn extract_nested_text<E: Extractor>(value: Option<&Value>) -> E::Output {
  let mut extractor = E::default();
  walk_nested_text(value, &mut extractor);
  extractor.finish()
}

fn walk_text_like<E: Extractor>(value: Option<&Value>, extractor: &mut E) {
  match value {
    Some(Value::String(s)) => extractor.push(s),
    Some(Value::Object(map)) => {
      if let Some(s) = map.get("text").or_else(|| map.get("value")).and_then(|v| v.as_str()) {
        extractor.push(s);
      }
    }
    Some(Value::Array(items)) => {
      for item in items {
        walk_text_like(Some(item), extractor);
      }
    }
    _ => {}
  }
}

fn walk_rich_text<E: Extractor>(value: Option<&Value>, extractor: &mut E) {
  match value {
    Some(Value::String(s)) => extractor.push(s),
    Some(Value::Array(items)) => {
      for item in items {
        walk_rich_text(Some(item), extractor);
      }
    }
    Some(Value::Object(map)) => {
      if let Some(s) = map.get("text").and_then(|v| v.as_str()) {
        extractor.push(s);
      }
      if let Some(children) = map.get("children").and_then(|v| v.as_array()) {
        for child in children {
          walk_rich_text(Some(child), extractor);
        }
      }
      walk_rich_text(map.get("node"), extractor);
    }
    _ => {}
  }
}

fn walk_nested_text<E: Extractor>(value: Option<&Value>, extractor: &mut E) {
  match value {
    Some(Value::String(s)) => extractor.push(s),
    Some(Value::Array(items)) => {
      for item in items {
        walk_nested_text(Some(item), extractor);
      }
    }
    Some(Value::Object(map)) => {
      for key in ["text", "value", "output", "content"] {
        walk_nested_text(map.get(key), extractor);
      }
    }
    _ => {}
  }
}
