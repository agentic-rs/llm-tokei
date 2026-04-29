use std::path::PathBuf;

use anyhow::Result;

use crate::model::{Summary, UsageRecord};

pub mod opencode;

pub trait SessionParser {
    fn provider(&self) -> &'static str;
    fn default_paths(&self) -> Vec<PathBuf>;
    fn parse_paths(&self, paths: Vec<PathBuf>) -> Result<Vec<UsageRecord>>;

    fn summarize(&self, paths: Vec<PathBuf>) -> Result<Summary> {
        let paths = if paths.is_empty() {
            self.default_paths()
        } else {
            paths
        };

        Ok(Summary::from_records(
            self.provider(),
            self.parse_paths(paths)?,
        ))
    }
}
