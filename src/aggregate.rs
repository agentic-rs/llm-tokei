use crate::model::UsageRecord;
use crate::pricing::PricingTable;
use crate::time::date_bucket;
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupDim {
    Source,
    Model,
    Provider,
    Project,
    Date,
    Session,
}

impl GroupDim {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_lowercase().as_str() {
            "source" | "tool" => GroupDim::Source,
            "model" => GroupDim::Model,
            "provider" => GroupDim::Provider,
            "project" | "cwd" => GroupDim::Project,
            "date" | "day" => GroupDim::Date,
            "session" => GroupDim::Session,
            _ => return None,
        })
    }

    pub fn label(&self) -> &'static str {
        match self {
            GroupDim::Source => "source",
            GroupDim::Model => "model",
            GroupDim::Provider => "provider",
            GroupDim::Project => "project",
            GroupDim::Date => "date",
            GroupDim::Session => "session",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Aggregate {
    pub keys: Vec<String>,
    pub input: u64,
    pub output: u64,
    pub reasoning: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub total: u64,
    pub turns: u64,
    pub cost_embedded: f64,
    pub cost_base: f64,
    pub cost_multiplied: f64,
    pub first_ts: Option<DateTime<Utc>>,
    pub last_ts: Option<DateTime<Utc>>,
}

pub struct Filters {
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    pub model_glob: Option<glob::Pattern>,
    pub provider_glob: Option<glob::Pattern>,
    pub cwd_glob: Option<glob::Pattern>,
}

impl Filters {
    pub fn matches(&self, r: &UsageRecord, pricing: &PricingTable) -> bool {
        if let Some(s) = self.since {
            if r.ts < s {
                return false;
            }
        }
        if let Some(u) = self.until {
            if r.ts > u {
                return false;
            }
        }
        if let Some(g) = &self.model_glob {
            let canonical = pricing.canonical_model(r.provider.as_deref(), r.model.as_deref());
            if !r.model.as_deref().is_some_and(|m| g.matches(m)) && !g.matches(&canonical) {
                return false;
            }
        }
        if let Some(g) = &self.provider_glob {
            if !r.provider.as_deref().is_some_and(|p| g.matches(p)) {
                return false;
            }
        }
        if let Some(g) = &self.cwd_glob {
            if !r.project_cwd.as_deref().is_some_and(|c| g.matches(c)) {
                return false;
            }
        }
        true
    }
}

pub fn key_for(
    r: &UsageRecord,
    dims: &[GroupDim],
    date_bucket_unit: &str,
    pricing: &PricingTable,
) -> Vec<String> {
    dims.iter()
        .map(|d| match d {
            GroupDim::Source => r.source.as_str().to_string(),
            GroupDim::Model => pricing.canonical_model(r.provider.as_deref(), r.model.as_deref()),
            GroupDim::Provider => r.provider.clone().unwrap_or_else(|| "-".into()),
            GroupDim::Project => r
                .project_name
                .clone()
                .or_else(|| {
                    r.project_cwd.as_ref().map(|c| {
                        std::path::Path::new(c)
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or(c)
                            .to_string()
                    })
                })
                .unwrap_or_else(|| "-".into()),
            GroupDim::Date => date_bucket(r.ts, date_bucket_unit),
            GroupDim::Session => {
                let sid = &r.session_id;
                // shorten long ids for readability
                if sid.len() > 14 {
                    format!("{}…", &sid[..14])
                } else {
                    sid.clone()
                }
            }
        })
        .collect()
}

pub fn aggregate(
    records: &[UsageRecord],
    dims: &[GroupDim],
    date_bucket_unit: &str,
    filters: &Filters,
    pricing: &PricingTable,
) -> Vec<Aggregate> {
    let mut map: BTreeMap<Vec<String>, Aggregate> = BTreeMap::new();
    for r in records.iter().filter(|r| filters.matches(r, pricing)) {
        let key = key_for(r, dims, date_bucket_unit, pricing);
        let agg = map.entry(key.clone()).or_insert_with(|| Aggregate {
            keys: key,
            input: 0,
            output: 0,
            reasoning: 0,
            cache_read: 0,
            cache_write: 0,
            total: 0,
            turns: 0,
            cost_embedded: 0.0,
            cost_base: 0.0,
            cost_multiplied: 0.0,
            first_ts: None,
            last_ts: None,
        });
        agg.input += r.input;
        agg.output += r.output;
        agg.reasoning += r.reasoning;
        agg.cache_read += r.cache_read;
        agg.cache_write += r.cache_write;
        agg.total += r.total();
        agg.turns += 1;
        if let Some(c) = r.cost_embedded {
            agg.cost_embedded += c;
        }
        if let Some((base, mult)) = pricing.cost_for(r) {
            agg.cost_base += base;
            agg.cost_multiplied += mult;
        }
        agg.first_ts = Some(match agg.first_ts {
            Some(t) if t < r.ts => t,
            _ => r.ts,
        });
        agg.last_ts = Some(match agg.last_ts {
            Some(t) if t > r.ts => t,
            _ => r.ts,
        });
    }
    map.into_values().collect()
}

#[derive(Debug, Clone, Copy)]
pub enum SortKey {
    Total,
    Input,
    Output,
    Cost,
    CostBase,
    Date,
    Turns,
}

impl SortKey {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.to_lowercase().as_str() {
            "total" => SortKey::Total,
            "input" => SortKey::Input,
            "output" => SortKey::Output,
            "cost" | "cost-multiplied" | "cost_multiplied" => SortKey::Cost,
            "cost-base" | "cost_base" | "base" => SortKey::CostBase,
            "date" | "time" => SortKey::Date,
            "turns" => SortKey::Turns,
            _ => return None,
        })
    }
}

pub fn sort_aggs(aggs: &mut [Aggregate], key: SortKey, descending: bool) {
    aggs.sort_by(|a, b| {
        let ord = match key {
            SortKey::Total => a.total.cmp(&b.total),
            SortKey::Input => a.input.cmp(&b.input),
            SortKey::Output => a.output.cmp(&b.output),
            SortKey::Cost => a
                .cost_multiplied
                .partial_cmp(&b.cost_multiplied)
                .unwrap_or(std::cmp::Ordering::Equal),
            SortKey::CostBase => a
                .cost_base
                .partial_cmp(&b.cost_base)
                .unwrap_or(std::cmp::Ordering::Equal),
            SortKey::Date => a.last_ts.cmp(&b.last_ts),
            SortKey::Turns => a.turns.cmp(&b.turns),
        };
        if descending { ord.reverse() } else { ord }
    });
}
