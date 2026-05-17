//! Dump-record types and the [`DumpSink`] accumulator.
//!
//! A "dump" is the human-readable replay of a session: an ordered list of
//! [`DumpRecord`] values, each tagged with a logical role. Sources turn their
//! per-file traversals into a [`DumpedSession`] by feeding [`TextSpan`]s
//! through [`DumpSink`].

use crate::text_count::{SpanSink, TextSpan};
use std::borrow::Cow;

#[derive(Debug, Clone, serde::Serialize)]
pub struct DumpRecord {
  pub role: &'static str,
  pub text: String,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub encrypted_text: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub display: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub call_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DumpedSession {
  pub session_id: String,
  pub records: Vec<DumpRecord>,
}

/// Accumulates [`TextSpan`]s into [`DumpRecord`]s.
#[derive(Default)]
pub struct DumpSink {
  pub records: Vec<DumpRecord>,
}

impl DumpSink {
  /// Convert a span into a [`DumpRecord`] without going through the trait,
  /// returning `None` when both text payloads are empty.
  pub fn record_from(span: TextSpan<'_>) -> Option<DumpRecord> {
    if span.text.is_empty() && span.encrypted_text.as_deref().unwrap_or_default().is_empty() {
      return None;
    }
    Some(DumpRecord {
      role: span.role,
      text: span.text.into_owned(),
      encrypted_text: span.encrypted_text.map(Cow::into_owned),
      display: span.display.map(Cow::into_owned),
      call_id: span.call_id.map(Cow::into_owned),
    })
  }
}

impl SpanSink for DumpSink {
  fn text(&mut self, span: TextSpan<'_>) {
    if let Some(record) = Self::record_from(span) {
      self.records.push(record);
    }
  }
}
