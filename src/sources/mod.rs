use crate::model::UsageRecord;
use anyhow::Result;

pub mod claude;
pub mod codex;
pub mod copilot;
pub mod copilot_cli;
pub mod copilot_shutdown;
pub mod opencode;

#[allow(dead_code)]
pub trait UsageSource {
  fn name(&self) -> &'static str;
  fn collect(&self) -> Result<Vec<UsageRecord>>;
}
