use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Source {
    Codex,
    OpenCode,
}

impl Source {
    pub fn as_str(&self) -> &'static str {
        match self {
            Source::Codex => "codex",
            Source::OpenCode => "opencode",
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
    pub reasoning: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    /// Cost reported by the source (e.g. OpenCode); USD.
    pub cost_embedded: Option<f64>,
}

impl UsageRecord {
    pub fn total(&self) -> u64 {
        self.input
            .saturating_add(self.output)
            .saturating_add(self.reasoning)
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write)
    }
}
