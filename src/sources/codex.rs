use crate::model::{Source, UsageRecord};
use crate::sources::UsageSource;
use anyhow::Result;
use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use walkdir::WalkDir;

pub struct CodexSource {
    pub root: PathBuf,
}

impl CodexSource {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn default_path() -> Option<PathBuf> {
        let base = std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .map(|p| p.join(".codex"))
            })?;
        Some(base.join("sessions"))
    }
}

#[derive(Debug, Deserialize)]
struct RolloutLine {
    #[serde(default, rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    payload: Option<serde_json::Value>,
    // session_meta inline fields (some versions emit at top level)
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    originator: Option<String>,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct TokenUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    cached_input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    reasoning_output_tokens: u64,
    #[serde(default)]
    total_tokens: u64,
}

impl UsageSource for CodexSource {
    fn name(&self) -> &'static str {
        "codex"
    }

    fn collect(&self) -> Result<Vec<UsageRecord>> {
        if !self.root.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for entry in WalkDir::new(&self.root)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n,
                None => continue,
            };
            if !name.ends_with(".jsonl") {
                continue;
            }
            if let Ok(rec) = parse_rollout(path) {
                if let Some(r) = rec {
                    out.push(r);
                }
            }
        }
        Ok(out)
    }
}

fn parse_rollout(path: &std::path::Path) -> Result<Option<UsageRecord>> {
    let f = File::open(path)?;
    let reader = BufReader::new(f);

    let mut session_id: Option<String> = None;
    let mut cwd: Option<String> = None;
    let mut model: Option<String> = None;
    let mut provider: Option<String> = None;
    let mut session_ts: Option<DateTime<Utc>> = None;
    let mut last_total: Option<TokenUsage> = None;
    let mut summed: Option<TokenUsage> = None; // fallback if only deltas exist
    let mut last_ts: Option<DateTime<Utc>> = None;

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        if line.trim().is_empty() {
            continue;
        }
        let parsed: RolloutLine = match serde_json::from_str(&line) {
            Ok(p) => p,
            Err(_) => continue,
        };

        if let Some(ts_str) = &parsed.timestamp {
            if let Ok(dt) = DateTime::parse_from_rfc3339(ts_str) {
                let utc = dt.with_timezone(&Utc);
                last_ts = Some(utc);
                if session_ts.is_none() {
                    session_ts = Some(utc);
                }
            }
        }

        match parsed.kind.as_deref() {
            Some("session_meta") => {
                // payload may carry the meta or it may be at top-level.
                if let Some(payload) = &parsed.payload {
                    let meta_holder = payload.get("meta").unwrap_or(payload);
                    if session_id.is_none() {
                        session_id = meta_holder
                            .get("id")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                    }
                    if cwd.is_none() {
                        cwd = meta_holder
                            .get("cwd")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                    }
                    if model.is_none() {
                        model = meta_holder
                            .get("model")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                    }
                    if provider.is_none() {
                        provider = meta_holder
                            .get("model_provider")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                            .or_else(|| {
                                meta_holder
                                    .get("originator")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string())
                            });
                    }
                }
                if session_id.is_none() {
                    session_id = parsed.id.clone();
                }
                if cwd.is_none() {
                    cwd = parsed.cwd.clone();
                }
                if model.is_none() {
                    model = parsed.model.clone();
                }
                if provider.is_none() {
                    provider = parsed.originator.clone();
                }
            }
            Some("event_msg") => {
                if let Some(payload) = &parsed.payload {
                    let inner_kind = payload.get("type").and_then(|v| v.as_str());
                    if inner_kind == Some("token_count") {
                        let info = payload.get("info").unwrap_or(payload);
                        if let Some(total) = info.get("total_token_usage") {
                            if let Ok(t) =
                                serde_json::from_value::<TokenUsage>(total.clone())
                            {
                                last_total = Some(t);
                            }
                        } else if let Some(last) = info.get("last_token_usage") {
                            if let Ok(t) =
                                serde_json::from_value::<TokenUsage>(last.clone())
                            {
                                let acc = summed.get_or_insert_with(TokenUsage::default);
                                acc.input_tokens += t.input_tokens;
                                acc.cached_input_tokens += t.cached_input_tokens;
                                acc.output_tokens += t.output_tokens;
                                acc.reasoning_output_tokens += t.reasoning_output_tokens;
                                acc.total_tokens += t.total_tokens;
                            }
                        }
                    }
                }
            }
            Some("turn_context") => {
                if let Some(payload) = &parsed.payload {
                    if let Some(m) = payload.get("model").and_then(|v| v.as_str()) {
                        // prefer the latest turn_context model
                        model = Some(m.to_string());
                    }
                }
            }
            Some("response_item") => {
                if model.is_none() {
                    if let Some(payload) = &parsed.payload {
                        if let Some(m) = payload.get("model").and_then(|v| v.as_str()) {
                            model = Some(m.to_string());
                        }
                    }
                }
            }
            _ => {}
        }
    }

    let usage = match last_total.or(summed) {
        Some(u) => u,
        None => return Ok(None), // no token data → skip
    };

    // session_id fallback: file stem
    let sid = session_id.unwrap_or_else(|| {
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string()
    });

    let ts = last_ts
        .or(session_ts)
        .unwrap_or_else(|| Utc.timestamp_opt(0, 0).single().unwrap_or_else(Utc::now));

    // input_tokens in Codex usage is the *non-cached* input;
    // cached_input_tokens is the cache-read portion. Codex CLI does not
    // surface cache writes, so cache_write stays 0.
    Ok(Some(UsageRecord {
        source: Source::Codex,
        session_id: sid,
        session_title: None,
        project_cwd: cwd,
        project_name: None,
        provider,
        model,
        ts,
        input: usage.input_tokens,
        output: usage.output_tokens,
        reasoning: usage.reasoning_output_tokens,
        cache_read: usage.cached_input_tokens,
        cache_write: 0,
        cost_embedded: None,
    }))
}

#[allow(dead_code)]
pub fn _phantom(_m: HashMap<String, String>) {}
