//! Text and token counting primitives shared across sources.
//!
//! Two sink families live here:
//!
//! * [`TextSink`] - drives the JSON walkers (`all_strings`, `text_value`, etc.).
//!   Implementations decide whether to accumulate stats ([`StatsSink`]) or
//!   stitch the strings back together ([`StringSink`]).
//! * [`SpanSink`] - receives semantically labelled [`TextSpan`]/[`TokenSpan`]
//!   values from source visitors. Used to compute byte/char stats per role
//!   ([`SpanStatsSink`]), token totals ([`TokenStatsSink`]), per-role byte
//!   breakdowns ([`BytesSink`]), or build dump transcripts (see
//!   `crate::sources::dump::DumpSink`).

use serde_json::Value;
use std::borrow::Cow;

#[derive(Debug, Default, Clone, Copy)]
pub struct TextStats {
  pub chars: u64,
  pub bytes: u64,
}

impl TextStats {
  pub fn from_str(s: &str) -> Self {
    Self {
      chars: s.chars().count() as u64,
      bytes: s.len() as u64,
    }
  }

  pub fn add(&mut self, other: Self) {
    self.chars = self.chars.saturating_add(other.chars);
    self.bytes = self.bytes.saturating_add(other.bytes);
  }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct TokenSpan {
  pub input: Option<u64>,
  pub output: Option<u64>,
  pub reasoning: Option<u64>,
  pub cache_read: Option<u64>,
  pub cache_write: Option<u64>,
}

impl TokenSpan {
  pub fn usage(input: u64, output: u64, reasoning: u64, cache_read: u64, cache_write: u64) -> Self {
    Self {
      input: Some(input),
      output: Some(output),
      reasoning: Some(reasoning),
      cache_read: Some(cache_read),
      cache_write: Some(cache_write),
    }
  }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct TokenUsageStats {
  pub input: u64,
  pub output: u64,
  pub reasoning: u64,
  pub cache_read: u64,
  pub cache_write: u64,
}

impl TokenUsageStats {
  pub fn add_span(&mut self, span: TokenSpan) {
    self.input = self.input.saturating_add(span.input.unwrap_or(0));
    self.output = self.output.saturating_add(span.output.unwrap_or(0));
    self.reasoning = self.reasoning.saturating_add(span.reasoning.unwrap_or(0));
    self.cache_read = self.cache_read.saturating_add(span.cache_read.unwrap_or(0));
    self.cache_write = self.cache_write.saturating_add(span.cache_write.unwrap_or(0));
  }
}

#[derive(Debug, Clone)]
pub struct TextSpan<'a> {
  pub role: &'static str,
  pub text: Cow<'a, str>,
  pub stats: Option<TextStats>,
  pub encrypted_text: Option<Cow<'a, str>>,
  pub display: Option<Cow<'a, str>>,
  pub call_id: Option<Cow<'a, str>>,
}

impl<'a> TextSpan<'a> {
  pub fn new(role: &'static str, text: impl Into<Cow<'a, str>>) -> Self {
    Self {
      role,
      text: text.into(),
      stats: None,
      encrypted_text: None,
      display: None,
      call_id: None,
    }
  }

  pub fn with_stats(mut self, stats: TextStats) -> Self {
    self.stats = Some(stats);
    self
  }

  pub fn with_call_id(mut self, call_id: Option<impl Into<Cow<'a, str>>>) -> Self {
    self.call_id = call_id.map(Into::into);
    self
  }

  pub fn encrypted(role: &'static str, encrypted_text: impl Into<Cow<'a, str>>, stats: TextStats) -> Self {
    Self {
      role,
      text: Cow::Borrowed(""),
      stats: Some(stats),
      encrypted_text: Some(encrypted_text.into()),
      display: None,
      call_id: None,
    }
  }

  /// Resolve the span's stats, computing from `text` when not pre-attached.
  pub fn resolved_stats(&self) -> TextStats {
    self.stats.unwrap_or_else(|| TextStats::from_str(&self.text))
  }
}

pub trait SpanSink {
  fn text(&mut self, span: TextSpan<'_>);

  fn token(&mut self, _span: TokenSpan) {}
}

#[derive(Default)]
pub struct TokenStatsSink {
  pub usage: TokenUsageStats,
}

impl SpanSink for TokenStatsSink {
  fn text(&mut self, _span: TextSpan<'_>) {}

  fn token(&mut self, span: TokenSpan) {
    self.usage.add_span(span);
  }
}

#[derive(Default)]
pub struct SpanStatsSink {
  pub stats: TextStats,
}

impl SpanSink for SpanStatsSink {
  fn text(&mut self, span: TextSpan<'_>) {
    self.stats.add(span.resolved_stats());
  }
}

/// Per-role byte accumulator used by source visitors to attribute bytes
/// between input, output, and reasoning channels based on a span's `role`.
#[derive(Debug, Default, Clone, Copy)]
pub struct BytesSink {
  pub input: u64,
  pub output: u64,
  pub reasoning: u64,
}

impl BytesSink {
  pub fn total(&self) -> u64 {
    self.input.saturating_add(self.output).saturating_add(self.reasoning)
  }

  pub fn take(&mut self) -> Self {
    std::mem::take(self)
  }

  pub fn add(&mut self, other: Self) {
    self.input = self.input.saturating_add(other.input);
    self.output = self.output.saturating_add(other.output);
    self.reasoning = self.reasoning.saturating_add(other.reasoning);
  }
}

impl SpanSink for BytesSink {
  fn text(&mut self, span: TextSpan<'_>) {
    let bytes = span.resolved_stats().bytes;
    match span.role {
      "user" | "system" | "developer" | "tool_call_result" => {
        self.input = self.input.saturating_add(bytes);
      }
      "assistant" | "tool_call" => {
        self.output = self.output.saturating_add(bytes);
      }
      "reasoning" => {
        self.reasoning = self.reasoning.saturating_add(bytes);
      }
      _ => {}
    }
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
    self.0.add(TextStats::from_str(s));
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
