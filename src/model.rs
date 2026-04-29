use std::collections::BTreeMap;

use serde::Serialize;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
}

impl TokenUsage {
    pub fn total(self) -> u64 {
        self.input_tokens + self.output_tokens + self.cache_creation_tokens + self.cache_read_tokens
    }

    pub fn add(&mut self, other: Self) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_creation_tokens += other.cache_creation_tokens;
        self.cache_read_tokens += other.cache_read_tokens;
    }

    pub fn is_empty(self) -> bool {
        self.total() == 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageRecord {
    pub session_id: String,
    pub model: Option<String>,
    pub usage: TokenUsage,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub model: Option<String>,
    pub messages: u64,
    pub usage: TokenUsage,
    pub total_tokens: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub provider: String,
    pub sessions: Vec<SessionSummary>,
    pub totals: SummaryTotals,
}

#[derive(Debug, Clone, Serialize)]
pub struct SummaryTotals {
    pub sessions: u64,
    pub messages: u64,
    pub usage: TokenUsage,
    pub total_tokens: u64,
}

impl Summary {
    pub fn from_records(provider: impl Into<String>, records: Vec<UsageRecord>) -> Self {
        let mut grouped: BTreeMap<String, SessionSummary> = BTreeMap::new();

        for record in records {
            let entry = grouped
                .entry(record.session_id.clone())
                .or_insert(SessionSummary {
                    session_id: record.session_id,
                    model: record.model.clone(),
                    messages: 0,
                    usage: TokenUsage::default(),
                    total_tokens: 0,
                });

            if entry.model.is_none() {
                entry.model = record.model;
            }

            entry.messages += 1;
            entry.usage.add(record.usage);
            entry.total_tokens = entry.usage.total();
        }

        let sessions: Vec<SessionSummary> = grouped.into_values().collect();
        let mut usage = TokenUsage::default();
        let mut messages = 0;

        for session in &sessions {
            usage.add(session.usage);
            messages += session.messages;
        }

        Self {
            provider: provider.into(),
            totals: SummaryTotals {
                sessions: sessions.len() as u64,
                messages,
                usage,
                total_tokens: usage.total(),
            },
            sessions,
        }
    }
}
