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

#[derive(Debug, Clone, Deserialize, Default)]
struct ModelMultiplier {
    #[serde(default)]
    multiplier: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct ProviderEntry {
    #[serde(default)]
    multiplier: Option<f64>,
    #[serde(default)]
    models: HashMap<String, ModelMultiplier>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct PricingFile {
    #[serde(default)]
    providers: HashMap<String, ProviderEntry>,
    /// Keys are either "model" or "provider/model" (latter only when it differs).
    #[serde(default)]
    models: HashMap<String, Price>,
}

#[derive(Debug, Default, Clone)]
pub struct PricingTable {
    /// Provider entries keyed by lowercased provider name.
    providers: HashMap<String, ProviderEntry>,
    /// Prices keyed by lowercased "model" or "provider/model".
    models: HashMap<String, Price>,
}

const BUNDLED: &str = include_str!("../data/prices.json");

impl PricingTable {
    pub fn load_bundled() -> Self {
        let mut t = Self::default();
        if let Ok(file) = serde_json::from_str::<PricingFile>(BUNDLED) {
            t.merge(file);
        }
        t
    }

    pub fn merge_file(&mut self, path: &Path) -> Result<()> {
        let s = std::fs::read_to_string(path)
            .with_context(|| format!("reading pricing file {}", path.display()))?;
        let file: PricingFile = serde_json::from_str(&s)
            .with_context(|| format!("parsing pricing file {}", path.display()))?;
        self.merge(file);
        Ok(())
    }

    fn merge(&mut self, file: PricingFile) {
        for (k, v) in file.providers {
            let key = k.to_lowercase();
            let entry = self
                .providers
                .entry(key)
                .or_insert_with(ProviderEntry::default);
            if v.multiplier.is_some() {
                entry.multiplier = v.multiplier;
            }
            for (mk, mv) in v.models {
                entry.models.insert(mk.to_lowercase(), mv);
            }
        }
        for (k, v) in file.models {
            self.models.insert(k.to_lowercase(), v);
        }
    }

    /// Look up base USD/1M price for a (provider, model) pair.
    /// Tries `provider/model` first, then plain `model`.
    pub fn lookup_base(&self, provider: Option<&str>, model: Option<&str>) -> Option<&Price> {
        let model = model?;
        let m = model.to_lowercase();
        if let Some(p) = provider {
            let combined = format!("{}/{}", p.to_lowercase(), m);
            if let Some(price) = self.models.get(&combined) {
                return Some(price);
            }
        }
        self.models.get(&m)
    }

    /// Resolve the multiplier: model-specific override → provider default → 1.0.
    pub fn lookup_multiplier(&self, provider: Option<&str>, model: Option<&str>) -> f64 {
        let provider = match provider {
            Some(p) => p.to_lowercase(),
            None => return 1.0,
        };
        let entry = match self.providers.get(&provider) {
            Some(e) => e,
            None => return 1.0,
        };
        if let Some(model) = model {
            if let Some(m) = entry.models.get(&model.to_lowercase()) {
                if let Some(mult) = m.multiplier {
                    return mult;
                }
            }
        }
        entry.multiplier.unwrap_or(1.0)
    }

    /// Returns (cost_base, cost_multiplied) in USD; `None` when no base price.
    pub fn cost_for(&self, r: &UsageRecord) -> Option<(f64, f64)> {
        let p = self.lookup_base(r.provider.as_deref(), r.model.as_deref())?;
        let m = 1_000_000.0_f64;
        let reasoning_rate = p.reasoning.unwrap_or(p.output);
        let base = (r.input as f64 * p.input
            + r.output as f64 * p.output
            + r.cache_read as f64 * p.cache_read
            + r.cache_write as f64 * p.cache_write
            + r.reasoning as f64 * reasoning_rate)
            / m;
        let mult = self.lookup_multiplier(r.provider.as_deref(), r.model.as_deref());
        Some((base, base * mult))
    }
}
