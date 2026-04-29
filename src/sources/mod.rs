use crate::model::UsageRecord;
use anyhow::Result;

pub mod codex;
pub mod opencode;

#[allow(dead_code)]
pub trait UsageSource {
    fn name(&self) -> &'static str;
    fn collect(&self) -> Result<Vec<UsageRecord>>;
}
