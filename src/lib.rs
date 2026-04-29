pub mod model;
pub mod output;
pub mod providers;

use std::path::PathBuf;

use anyhow::Result;
use model::Summary;
use providers::{SessionParser, opencode::OpenCodeParser};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    OpenCode,
}

impl ProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenCode => "opencode",
        }
    }
}

pub fn summarize(provider: ProviderKind, paths: Vec<PathBuf>) -> Result<Summary> {
    match provider {
        ProviderKind::OpenCode => OpenCodeParser::new().summarize(paths),
    }
}
