use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

use crate::model::UsageRecord;

/// USD per 1M tokens for each category.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct Price {
    #[serde(default)]
    pub input: f64,
    #[serde(default)]
    pub output: f64,
    #[serde(default)]
    pub cache_read: f64,
    #[serde(default)]
    pub cache_write: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct PriceRow {
    pub provider: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub input: f64,
    #[serde(default)]
    pub output: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<f64>,
    #[serde(default)]
    pub cache_read: f64,
    #[serde(default)]
    pub cache_write: f64,
}

impl From<PriceRow> for Price {
    fn from(row: PriceRow) -> Self {
        Self {
            input: row.input,
            output: row.output,
            cache_read: row.cache_read,
            cache_write: row.cache_write,
            reasoning: row.reasoning,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ModelInfo {
    pub provider: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ModelOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multiplier: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub included: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ProviderEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multiplier: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub included: Option<bool>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub models: BTreeMap<String, ModelOverride>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct PricingFile {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub providers: BTreeMap<String, ProviderEntry>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub models: BTreeMap<String, ModelInfo>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prices: Vec<PriceRow>,
}

#[derive(Debug, Default, Clone)]
pub struct PricingTable {
    providers: BTreeMap<String, ProviderEntry>,
    models: BTreeMap<String, ModelInfo>,
    aliases: BTreeMap<String, String>,
    prices: BTreeMap<(String, String), Price>,
}

const BUNDLED: &str = include_str!(concat!(env!("OUT_DIR"), "/prices.json"));

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
        for (model, mut info) in file.models {
            let model = norm(&model);
            info.provider = norm(&info.provider);
            info.aliases = info.aliases.into_iter().map(|a| norm(&a)).collect();
            self.models.insert(model, info);
        }
        self.rebuild_aliases();

        for (k, v) in file.providers {
            let provider = norm(&k);
            let models = v
                .models
                .into_iter()
                .map(|(mk, mv)| (self.canonical_model(Some(&provider), Some(&mk)), mv))
                .collect::<Vec<_>>();
            let entry = self.providers.entry(provider.clone()).or_default();
            if v.multiplier.is_some() {
                entry.multiplier = v.multiplier;
            }
            if v.included.is_some() {
                entry.included = v.included;
            }
            for (model, mv) in models {
                let slot = entry.models.entry(model).or_default();
                if mv.multiplier.is_some() {
                    slot.multiplier = mv.multiplier;
                }
                if mv.included.is_some() {
                    slot.included = mv.included;
                }
            }
        }

        for row in file.prices {
            let provider = norm(&row.provider);
            let model = self.canonical_model(Some(&provider), Some(&row.model));
            self.prices.insert((provider, model), row.into());
        }
    }

    fn rebuild_aliases(&mut self) {
        self.aliases.clear();
        for (model, info) in &self.models {
            self.aliases.insert(model.clone(), model.clone());
            for alias in &info.aliases {
                self.aliases.insert(norm(alias), model.clone());
            }
        }
    }

    pub fn canonical_model(&self, provider: Option<&str>, model: Option<&str>) -> String {
        let Some(model) = model else {
            return "-".into();
        };
        let model = norm(model);
        if let Some(provider) = provider {
            if let Some(canonical) = self.aliases.get(&format!("{}/{}", norm(provider), model)) {
                return canonical.clone();
            }
        }
        self.aliases.get(&model).cloned().unwrap_or(model)
    }

    pub fn lookup_base(&self, provider: Option<&str>, model: Option<&str>) -> Option<&Price> {
        let canonical = self.canonical_model(provider, model);
        if canonical == "-" {
            return None;
        }
        if let Some(provider) = provider {
            let key = (norm(provider), canonical.clone());
            if let Some(price) = self.prices.get(&key) {
                return Some(price);
            }
        }
        if let Some(info) = self.models.get(&canonical) {
            let key = (norm(&info.provider), canonical.clone());
            if let Some(price) = self.prices.get(&key) {
                return Some(price);
            }
        }
        None
    }

    pub fn lookup_multiplier(&self, provider: Option<&str>, model: Option<&str>) -> f64 {
        let provider = match provider {
            Some(p) => norm(p),
            None => return 1.0,
        };
        let entry = match self.providers.get(&provider) {
            Some(e) => e,
            None => return 1.0,
        };
        let model = self.canonical_model(Some(&provider), model);
        if let Some(m) = entry.models.get(&model) {
            if let Some(mult) = m.multiplier {
                return mult;
            }
        }
        entry.multiplier.unwrap_or(1.0)
    }

    pub fn lookup_included(&self, provider: Option<&str>, model: Option<&str>) -> bool {
        let provider = match provider {
            Some(p) => norm(p),
            None => return false,
        };
        let entry = match self.providers.get(&provider) {
            Some(e) => e,
            None => return false,
        };
        let model = self.canonical_model(Some(&provider), model);
        if let Some(m) = entry.models.get(&model) {
            if let Some(inc) = m.included {
                return inc;
            }
        }
        entry.included.unwrap_or(false)
    }

    /// Returns (cost_base, cost_multiplied) in USD.
    /// `cost_multiplied` is forced to 0.0 when the (provider, model) is `included`.
    pub fn cost_for(&self, r: &UsageRecord) -> Option<(f64, f64)> {
        let p = self.lookup_base(r.provider.as_deref(), r.model.as_deref())?;
        let m = 1_000_000.0_f64;
        let reasoning_rate = p.reasoning.unwrap_or(p.output);
        let uncached_input = r.input.saturating_sub(r.cache_read);
        let base = (uncached_input as f64 * p.input
            + r.output as f64 * p.output
            + r.cache_read as f64 * p.cache_read
            + r.cache_write as f64 * p.cache_write
            + r.reasoning as f64 * reasoning_rate)
            / m;
        let multiplied = if self.lookup_included(r.provider.as_deref(), r.model.as_deref()) {
            0.0
        } else {
            base * self.lookup_multiplier(r.provider.as_deref(), r.model.as_deref())
        };
        Some((base, multiplied))
    }
}

fn norm(s: &str) -> String {
    s.trim().to_lowercase()
}
