use crate::model::UsageRecord;
use crate::pricing::{CostMode, PricingTable};
use crate::time::date_bucket;
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

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
  pub input_bytes: u64,
  pub output_bytes: u64,
  pub input_estimated: bool,
  pub output_estimated: bool,
  pub input_bytes_estimated: bool,
  pub output_bytes_estimated: bool,
  pub reasoning: u64,
  pub cache_read: u64,
  pub cache_write: u64,
  pub total: u64,
  pub calls: u64,
  pub rounds: u64,
  pub sessions: u64,
  pub cost_embedded: f64,
  pub cost: f64,
  pub cost_per: BTreeMap<String, f64>,
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

pub fn key_for(r: &UsageRecord, dims: &[GroupDim], date_bucket_unit: &str, pricing: &PricingTable) -> Vec<String> {
  dims
    .iter()
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
  cost_per: Option<GroupDim>,
  cost_mode: CostMode,
) -> Vec<Aggregate> {
  let mut map: BTreeMap<Vec<String>, Aggregate> = BTreeMap::new();
  let mut session_sets: BTreeMap<Vec<String>, BTreeSet<String>> = BTreeMap::new();
  for r in records.iter().filter(|r| filters.matches(r, pricing)) {
    let key = key_for(r, dims, date_bucket_unit, pricing);
    let agg = map.entry(key.clone()).or_insert_with(|| Aggregate {
      keys: key.clone(),
      input: 0,
      output: 0,
      input_bytes: 0,
      output_bytes: 0,
      input_estimated: false,
      output_estimated: false,
      input_bytes_estimated: false,
      output_bytes_estimated: false,
      reasoning: 0,
      cache_read: 0,
      cache_write: 0,
      total: 0,
      calls: 0,
      rounds: 0,
      sessions: 0,
      cost_embedded: 0.0,
      cost: 0.0,
      cost_per: BTreeMap::new(),
      first_ts: None,
      last_ts: None,
    });
    let sess_set = session_sets.entry(key).or_default();
    sess_set.insert(r.session_id.clone());

    agg.input += r.display_input();
    agg.output += r.display_output();
    agg.input_bytes += r.input_bytes;
    agg.output_bytes += r.output_bytes;
    agg.input_estimated |= r.input_estimated;
    agg.output_estimated |= r.output_estimated;
    agg.input_bytes_estimated |= r.input_bytes_estimated;
    agg.output_bytes_estimated |= r.output_bytes_estimated;
    agg.reasoning += r.reasoning;
    agg.cache_read += r.cache_read;
    agg.cache_write += r.cache_write;
    agg.total += r.total();
    agg.calls += r.calls;
    agg.rounds += r.rounds;
    if let Some(c) = r.cost_embedded {
      agg.cost_embedded += c;
    }
    if let Some(cost) = pricing.cost_for(r, cost_mode) {
      agg.cost += cost;
      if let Some(dim) = cost_per {
        let split_key = key_for(r, &[dim], date_bucket_unit, pricing)
          .into_iter()
          .next()
          .unwrap_or_else(|| "-".to_string());
        *agg.cost_per.entry(split_key).or_default() += cost;
      }
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
  for (key, agg) in map.iter_mut() {
    if let Some(set) = session_sets.get(key) {
      agg.sessions = set.len() as u64;
    }
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
  Calls,
}

impl SortKey {
  pub fn parse(s: &str) -> Option<Self> {
    Some(match s.to_lowercase().as_str() {
      "total" => SortKey::Total,
      "input" => SortKey::Input,
      "output" => SortKey::Output,
      "cost" => SortKey::Cost,
      "cost-base" | "cost_base" | "base" => SortKey::CostBase,
      "date" | "time" => SortKey::Date,
      "calls" => SortKey::Calls,
      _ => return None,
    })
  }
}

pub fn sort_aggs(aggs: &mut [Aggregate], key: SortKey, descending: bool, use_bytes: bool) {
  aggs.sort_by(|a, b| {
    let ord = match key {
      SortKey::Total => a.total.cmp(&b.total),
      SortKey::Input => {
        if use_bytes {
          a.input_bytes.cmp(&b.input_bytes)
        } else {
          a.input.cmp(&b.input)
        }
      }
      SortKey::Output => {
        if use_bytes {
          a.output_bytes.cmp(&b.output_bytes)
        } else {
          a.output.cmp(&b.output)
        }
      }
      SortKey::Cost | SortKey::CostBase => a.cost.partial_cmp(&b.cost).unwrap_or(std::cmp::Ordering::Equal),
      SortKey::Date => a.last_ts.cmp(&b.last_ts),
      SortKey::Calls => a.calls.cmp(&b.calls),
    };
    if descending {
      ord.reverse()
    } else {
      ord
    }
  });
}
