use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde_json::Value;
use walkdir::WalkDir;

use crate::model::{TokenUsage, UsageRecord};

use super::SessionParser;

pub struct OpenCodeParser;

impl OpenCodeParser {
    pub fn new() -> Self {
        Self
    }

    fn parse_file(&self, path: &Path) -> Result<Vec<UsageRecord>> {
        let file =
            File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
        let reader = BufReader::new(file);
        let session_id = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("unknown")
            .to_string();
        let mut records = Vec::new();

        for line in reader.lines() {
            let line = line.with_context(|| format!("failed to read {}", path.display()))?;
            if line.trim().is_empty() {
                continue;
            }

            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };

            if let Some(usage) = extract_usage(&value) {
                records.push(UsageRecord {
                    session_id: extract_session_id(&value).unwrap_or_else(|| session_id.clone()),
                    model: extract_model(&value),
                    usage,
                });
            }
        }

        Ok(records)
    }
}

impl Default for OpenCodeParser {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionParser for OpenCodeParser {
    fn provider(&self) -> &'static str {
        "opencode"
    }

    fn default_paths(&self) -> Vec<PathBuf> {
        let mut paths = Vec::new();

        if let Some(data_dir) = dirs::data_dir() {
            paths.push(data_dir.join("opencode"));
        }

        if let Some(home_dir) = dirs::home_dir() {
            paths.push(home_dir.join(".opencode"));
        }

        paths
    }

    fn parse_paths(&self, paths: Vec<PathBuf>) -> Result<Vec<UsageRecord>> {
        let mut records = Vec::new();

        for path in paths {
            if path.is_file() {
                records.extend(self.parse_file(&path)?);
                continue;
            }

            if !path.is_dir() {
                continue;
            }

            for entry in WalkDir::new(&path).into_iter().filter_map(Result::ok) {
                let path = entry.path();
                if path.is_file() && is_jsonish(path) {
                    records.extend(self.parse_file(path)?);
                }
            }
        }

        Ok(records)
    }
}

fn is_jsonish(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("json") | Some("jsonl")
    )
}

fn extract_session_id(value: &Value) -> Option<String> {
    first_string(
        value,
        &["sessionID", "sessionId", "session_id", "session.id", "id"],
    )
}

fn extract_model(value: &Value) -> Option<String> {
    first_string(value, &["model", "message.model", "response.model"])
}

fn extract_usage(value: &Value) -> Option<TokenUsage> {
    let usage = value
        .pointer("/usage")
        .or_else(|| value.pointer("/message/usage"))?;

    let result = TokenUsage {
        input_tokens: first_u64(
            usage,
            &[
                "inputTokens",
                "input_tokens",
                "promptTokens",
                "prompt_tokens",
                "tokens.input",
            ],
        ),
        output_tokens: first_u64(
            usage,
            &[
                "outputTokens",
                "output_tokens",
                "completionTokens",
                "completion_tokens",
                "tokens.output",
            ],
        ),
        cache_creation_tokens: first_u64(
            usage,
            &[
                "cacheCreationTokens",
                "cache_creation_tokens",
                "cacheCreationInputTokens",
                "cache_creation_input_tokens",
            ],
        ),
        cache_read_tokens: first_u64(
            usage,
            &[
                "cacheReadTokens",
                "cache_read_tokens",
                "cacheReadInputTokens",
                "cache_read_input_tokens",
            ],
        ),
    };

    (!result.is_empty()).then_some(result)
}

fn first_string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .filter_map(|key| get_path(value, key))
        .find_map(|value| value.as_str().map(ToString::to_string))
}

fn first_u64(value: &Value, keys: &[&str]) -> u64 {
    keys.iter()
        .filter_map(|key| get_path(value, key))
        .find_map(|value| value.as_u64())
        .unwrap_or(0)
}

fn get_path<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    key.split('.')
        .try_fold(value, |current, segment| current.get(segment))
}

#[cfg(test)]
mod tests {
    use std::{fs, io::Write};

    use tempfile::NamedTempFile;

    use super::*;

    #[test]
    fn parses_opencode_jsonl_usage() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"{{"sessionID":"abc","model":"anthropic/claude-sonnet-4","usage":{{"inputTokens":10,"outputTokens":20,"cacheReadTokens":3}}}}"#
        )
        .unwrap();

        let records = OpenCodeParser::new()
            .parse_paths(vec![file.path().to_path_buf()])
            .unwrap();

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].session_id, "abc");
        assert_eq!(records[0].usage.input_tokens, 10);
        assert_eq!(records[0].usage.output_tokens, 20);
        assert_eq!(records[0].usage.cache_read_tokens, 3);
    }

    #[test]
    fn scans_json_files_in_directories() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        fs::write(&path, r#"{"usage":{"input_tokens":1,"output_tokens":2}}"#).unwrap();

        let records = OpenCodeParser::new()
            .parse_paths(vec![dir.path().to_path_buf()])
            .unwrap();

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].usage.total(), 3);
    }
}
