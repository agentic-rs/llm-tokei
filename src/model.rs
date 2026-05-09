use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Source {
  Codex,
  OpenCode,
  Claude,
  Copilot,
  CopilotCli,
}

impl Source {
  pub fn as_str(&self) -> &'static str {
    match self {
      Source::Codex => "codex",
      Source::OpenCode => "opencode",
      Source::Claude => "claude",
      Source::Copilot => "copilot",
      Source::CopilotCli => "copilot-cli",
    }
  }
}

impl std::fmt::Display for Source {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.write_str(self.as_str())
  }
}

#[derive(Debug, Clone, Serialize)]
pub struct UsageRecord {
  pub source: Source,
  pub session_id: String,
  pub session_title: Option<String>,
  pub project_cwd: Option<String>,
  pub project_name: Option<String>,
  pub provider: Option<String>,
  pub model: Option<String>,
  pub ts: DateTime<Utc>,
  pub input: u64,
  pub output: u64,
  pub input_bytes: u64,
  pub output_bytes: u64,
  pub input_estimated: bool,
  pub output_estimated: bool,
  pub input_bytes_estimated: bool,
  pub output_bytes_estimated: bool,
  pub reasoning: u64,
  pub cache_read: u64,
  pub cache_write: u64,
  pub mode: Option<String>,
  pub agent: Option<String>,
  pub is_compaction: bool,
  /// Number of user-initiated rounds (prompts) in this record.
  pub rounds: u64,
  /// Number of total API turns (including tool-call continuations) in this record.
  pub turns: u64,
  /// Cost reported by the source (e.g. OpenCode); USD.
  pub cost_embedded: Option<f64>,
}

impl UsageRecord {
  /// Displayed input includes cached reads and writes.
  pub fn display_input(&self) -> u64 {
    self
      .input
      .saturating_add(self.cache_read)
      .saturating_add(self.cache_write)
  }

  /// Display total uses the displayed input column as-is.
  pub fn total(&self) -> u64 {
    self
      .display_input()
      .saturating_add(self.output)
      .saturating_add(self.reasoning)
  }
}
