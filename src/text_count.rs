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

#[derive(Debug, Default, Clone, Copy)]
pub struct TextStats {
  pub chars: u64,
  pub bytes: u64,
}

impl TextStats {
  pub fn add(&mut self, other: Self) {
    self.chars = self.chars.saturating_add(other.chars);
    self.bytes = self.bytes.saturating_add(other.bytes);
  }
}

pub trait TextSink: Default {
  type Output;

  fn text(&mut self, s: &str);
  fn finish(self) -> Self::Output;
}

#[derive(Default)]
pub struct StatsSink(TextStats);

impl TextSink for StatsSink {
  type Output = TextStats;

  fn text(&mut self, s: &str) {
    self.0.chars = self.0.chars.saturating_add(Chars.count(s));
    self.0.bytes = self.0.bytes.saturating_add(Bytes.count(s));
  }

  fn finish(self) -> Self::Output {
    self.0
  }
}

#[derive(Default)]
pub struct StringSink(Vec<String>);

impl TextSink for StringSink {
  type Output = String;

  fn text(&mut self, s: &str) {
    if !s.is_empty() {
      self.0.push(s.to_string());
    }
  }

  fn finish(self) -> Self::Output {
    self.0.join("\n")
  }
}

pub fn all_strings<S: TextSink>(value: Option<&Value>) -> S::Output {
  extract(value, walk_all_strings::<S>)
}

pub fn text_value<S: TextSink>(value: Option<&Value>) -> S::Output {
  extract(value, walk_text_value::<S>)
}

pub fn rich_text<S: TextSink>(value: Option<&Value>) -> S::Output {
  extract(value, walk_rich_text::<S>)
}

pub fn nested_fields<S: TextSink>(value: Option<&Value>) -> S::Output {
  extract(value, walk_nested_fields::<S>)
}

pub fn message_content<S: TextSink>(value: Option<&Value>) -> S::Output {
  extract(value, walk_message_content::<S>)
}

pub fn json_serialized_or_string<S: TextSink>(value: Option<&Value>) -> S::Output {
  extract(value, walk_json_serialized_or_string::<S>)
}

fn extract<S: TextSink>(value: Option<&Value>, walk: fn(Option<&Value>, &mut S)) -> S::Output {
  let mut sink = S::default();
  walk(value, &mut sink);
  sink.finish()
}

fn walk_all_strings<S: TextSink>(value: Option<&Value>, sink: &mut S) {
  match value {
    Some(Value::String(s)) => sink.text(s),
    Some(Value::Array(items)) => {
      for item in items {
        walk_all_strings(Some(item), sink);
      }
    }
    Some(Value::Object(map)) => {
      for item in map.values() {
        walk_all_strings(Some(item), sink);
      }
    }
    _ => {}
  }
}

fn walk_text_value<S: TextSink>(value: Option<&Value>, sink: &mut S) {
  match value {
    Some(Value::String(s)) => sink.text(s),
    Some(Value::Object(map)) => {
      if let Some(s) = map.get("text").or_else(|| map.get("value")).and_then(|v| v.as_str()) {
        sink.text(s);
      }
    }
    Some(Value::Array(items)) => {
      for item in items {
        walk_text_value(Some(item), sink);
      }
    }
    _ => {}
  }
}

fn walk_rich_text<S: TextSink>(value: Option<&Value>, sink: &mut S) {
  match value {
    Some(Value::String(s)) => sink.text(s),
    Some(Value::Array(items)) => {
      for item in items {
        walk_rich_text(Some(item), sink);
      }
    }
    Some(Value::Object(map)) => {
      if let Some(s) = map.get("text").and_then(|v| v.as_str()) {
        sink.text(s);
      }
      if let Some(children) = map.get("children").and_then(|v| v.as_array()) {
        for child in children {
          walk_rich_text(Some(child), sink);
        }
      }
      walk_rich_text(map.get("node"), sink);
    }
    _ => {}
  }
}

fn walk_nested_fields<S: TextSink>(value: Option<&Value>, sink: &mut S) {
  match value {
    Some(Value::String(s)) => sink.text(s),
    Some(Value::Array(items)) => {
      for item in items {
        walk_nested_fields(Some(item), sink);
      }
    }
    Some(Value::Object(map)) => {
      for key in ["text", "value", "output", "content"] {
        walk_nested_fields(map.get(key), sink);
      }
    }
    _ => {}
  }
}

fn walk_message_content<S: TextSink>(value: Option<&Value>, sink: &mut S) {
  match value {
    Some(Value::String(s)) => sink.text(s),
    Some(Value::Array(items)) => {
      for item in items {
        if let Some(s) = item.get("text").and_then(|v| v.as_str()) {
          sink.text(s);
        } else {
          walk_nested_fields(Some(item), sink);
        }
      }
    }
    Some(value) => walk_nested_fields(Some(value), sink),
    None => {}
  }
}

fn walk_json_serialized_or_string<S: TextSink>(value: Option<&Value>, sink: &mut S) {
  match value {
    Some(Value::String(s)) => sink.text(s),
    Some(value) => {
      if let Ok(serialized) = serde_json::to_string(value) {
        sink.text(&serialized);
      }
    }
    None => {}
  }
}
