use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

use crate::model::UsageRecord;

/// USD per 1M tokens for each category.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Price {
    #[serde(default)]
    pub input: f64,
    #[serde(default)]
    pub output: f64,
    #[serde(default)]
    pub cache_read: f64,
    #[serde(default)]
    pub cache_write: f64,
    #[serde(default)]
    pub reasoning: Option<f64>,
}

#[derive(Debug, Default, Clone)]
pub struct PricingTable {
    /// Keyed by lowercased "provider/model" or just "model".
    map: HashMap<String, Price>,
}

const BUNDLED: &str = include_str!("../data/prices.json");

impl PricingTable {
    pub fn load_bundled() -> Self {
        let mut t = Self::default();
        if let Ok(map) = serde_json::from_str::<HashMap<String, Price>>(BUNDLED) {
            t.map = map.into_iter().map(|(k, v)| (k.to_lowercase(), v)).collect();
        }
        t
    }

    pub fn merge_file(&mut self, path: &Path) -> Result<()> {
        let s = std::fs::read_to_string(path)
            .with_context(|| format!("reading pricing file {}", path.display()))?;
        let map: HashMap<String, Price> = serde_json::from_str(&s)
            .with_context(|| format!("parsing pricing file {}", path.display()))?;
        for (k, v) in map {
            self.map.insert(k.to_lowercase(), v);
        }
        Ok(())
    }

    pub fn lookup(&self, provider: Option<&str>, model: Option<&str>) -> Option<&Price> {
        let model = model?;
        if let Some(p) = provider {
            let k = format!("{}/{}", p, model).to_lowercase();
            if let Some(v) = self.map.get(&k) {
                return Some(v);
            }
        }
        self.map.get(&model.to_lowercase())
    }

    pub fn cost_for(&self, r: &UsageRecord) -> Option<f64> {
        let p = self.lookup(r.provider.as_deref(), r.model.as_deref())?;
        let m = 1_000_000.0_f64;
        let reasoning_rate = p.reasoning.unwrap_or(p.output);
        Some(
            (r.input as f64 * p.input
                + r.output as f64 * p.output
                + r.cache_read as f64 * p.cache_read
                + r.cache_write as f64 * p.cache_write
                + r.reasoning as f64 * reasoning_rate)
                / m,
        )
    }
}
