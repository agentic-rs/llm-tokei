use anyhow::{bail, Context, Result};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::model::UsageRecord;
use crate::model_name::{fuzzy_resolve, norm};

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum CostMode {
  /// Provider-specific cost; included providers are treated as $0.
  Actual,
  /// Provider-specific cost; included providers fall back to official model rates.
  Mixed,
  /// Official model-provider rates only.
  Official,
}

/// USD per 1M tokens for each category.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct Price {
  #[serde(default)]
  pub input: f64,
  #[serde(default)]
  pub output: f64,
  #[serde(default)]
  pub cache_read: f64,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub cache_write: Option<f64>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub reasoning: Option<f64>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CostBreakdown {
  pub prompt: f64,
  pub completion: f64,
  pub reasoning: f64,
  pub cache_read: f64,
  pub cache_write: f64,
}

impl CostBreakdown {
  pub fn total(self) -> f64 {
    self.prompt + self.completion + self.reasoning + self.cache_read + self.cache_write
  }
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
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub cache_write: Option<f64>,
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
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub source: Option<bool>,
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
const BUNDLED_MODELS: &str = include_str!("../data/models.json");
const BUNDLED_PROVIDERS: &str = include_str!("../data/providers.json");
const MODELS_DEV_URL: &str = "https://models.dev/api.json";
const CACHE_PRICE_FILE: &str = "llm-tokei.price.json";

impl PricingTable {
  pub fn load_bundled() -> Self {
    let mut t = Self::default();
    if let Ok(file) = serde_json::from_str::<PricingFile>(BUNDLED) {
      t.merge(file);
    }
    t
  }

  pub fn load_default() -> Result<Self> {
    if let Some(path) = cached_price_path() {
      if path.exists() {
        return Self::load_file(&path);
      }
    }
    Ok(Self::load_bundled())
  }

  pub fn load_file(path: &Path) -> Result<Self> {
    let mut t = Self::default();
    t.merge_file(path)?;
    Ok(t)
  }

  pub fn merge_file(&mut self, path: &Path) -> Result<()> {
    let s = std::fs::read_to_string(path).with_context(|| format!("reading pricing file {}", path.display()))?;
    let file: PricingFile =
      serde_json::from_str(&s).with_context(|| format!("parsing pricing file {}", path.display()))?;
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
        .map(|(mk, mv)| (self.canonical_model_strict(Some(&provider), Some(&mk)), mv))
        .collect::<Vec<_>>();
      let entry = self.providers.entry(provider.clone()).or_default();
      if v.multiplier.is_some() {
        entry.multiplier = v.multiplier;
      }
      if v.included.is_some() {
        entry.included = v.included;
      }
      if v.source.is_some() {
        entry.source = v.source;
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
      let model = self.canonical_model_strict(Some(&provider), Some(&row.model));
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

  fn canonical_model_strict(&self, provider: Option<&str>, model: Option<&str>) -> String {
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
    if let Some(canonical) = self.aliases.get(&model) {
      return canonical.clone();
    }
    if let Some(canonical) = fuzzy_resolve(&self.aliases, &model) {
      return canonical;
    }
    model
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

  pub fn lookup_official_base(&self, provider: Option<&str>, model: Option<&str>) -> Option<&Price> {
    let canonical = self.canonical_model(provider, model);
    if canonical == "-" {
      return None;
    }
    let info = self.models.get(&canonical)?;
    self.prices.get(&(norm(&info.provider), canonical))
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

  pub fn cost_breakdown_for(&self, r: &UsageRecord, mode: CostMode) -> Option<CostBreakdown> {
    let provider_base = self
      .lookup_base(r.provider.as_deref(), r.model.as_deref())
      .map(|p| token_cost_breakdown(r, p));
    let official_base = self
      .lookup_official_base(r.provider.as_deref(), r.model.as_deref())
      .map(|p| token_cost_breakdown(r, p));

    match mode {
      CostMode::Actual => {
        if self.lookup_included(r.provider.as_deref(), r.model.as_deref()) {
          Some(CostBreakdown::default())
        } else {
          provider_base
            .map(|base| scale_cost_breakdown(base, self.lookup_multiplier(r.provider.as_deref(), r.model.as_deref())))
        }
      }
      CostMode::Mixed => {
        if self.lookup_included(r.provider.as_deref(), r.model.as_deref()) {
          official_base
        } else {
          provider_base
        }
      }
      CostMode::Official => official_base,
    }
  }
}

pub fn cached_price_path() -> Option<PathBuf> {
  std::env::var_os("XDG_CACHE_HOME")
    .map(PathBuf::from)
    .or_else(|| std::env::var_os("HOME").map(PathBuf::from).map(|p| p.join(".cache")))
    .map(|base| base.join(CACHE_PRICE_FILE))
}

pub fn update_cached_prices() -> Result<PathBuf> {
  let path = cached_price_path().context("cannot determine cache directory")?;
  let body = fetch_models_dev_json()?;
  let file = pricing_file_from_models_dev_json(&body)?;
  if let Some(parent) = path.parent() {
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
  }
  let json = format_pricing_file(&file)?;
  let temp = path.with_extension("json.tmp");
  std::fs::write(&temp, json).with_context(|| format!("writing {}", temp.display()))?;
  std::fs::rename(&temp, &path).with_context(|| format!("renaming {} to {}", temp.display(), path.display()))?;
  Ok(path)
}

fn fetch_models_dev_json() -> Result<String> {
  let config = ureq::Agent::config_builder()
    .timeout_global(Some(std::time::Duration::from_secs(30)))
    .build();
  let agent: ureq::Agent = config.into();
  let mut response = agent.get(MODELS_DEV_URL).call().context("requesting models.dev")?;
  response
    .body_mut()
    .read_to_string()
    .context("reading models.dev response body")
}

fn pricing_file_from_models_dev_json(body: &str) -> Result<PricingFile> {
  let api: BTreeMap<String, Value> = serde_json::from_str(body).context("parsing models.dev JSON")?;
  let models = read_bundled_models()?;
  let aliases = build_alias_map(&models)?;
  let rows = flatten_models_dev(&api, &aliases);
  let mut providers = infer_providers(&rows);
  let explicit_providers = read_bundled_providers(&aliases)?;
  merge_providers(&mut providers, explicit_providers);
  normalize_providers(&mut providers);
  let mut prices = build_prices(&rows, &providers);
  prices.sort_by(|a, b| (&a.provider, &a.model, &a.name).cmp(&(&b.provider, &b.model, &b.name)));
  prices = dedupe_prices(prices);
  Ok(PricingFile {
    providers,
    models,
    prices,
  })
}

fn read_bundled_models() -> Result<BTreeMap<String, ModelInfo>> {
  let raw: BTreeMap<String, ModelInfo> = serde_json::from_str(BUNDLED_MODELS).context("parsing bundled models.json")?;
  let mut out = BTreeMap::new();
  for (k, mut v) in raw {
    v.provider = norm(&v.provider);
    v.aliases = v.aliases.into_iter().map(|a| norm(&a)).collect();
    out.insert(norm(&k), v);
  }
  Ok(out)
}

fn build_alias_map(models: &BTreeMap<String, ModelInfo>) -> Result<BTreeMap<String, String>> {
  let canonicals: BTreeSet<&str> = models.keys().map(|s| s.as_str()).collect();
  let mut map = BTreeMap::new();
  for (model, info) in models {
    insert_alias(&mut map, model, model)?;
    for alias in &info.aliases {
      if canonicals.contains(alias.as_str()) && alias != model {
        bail!("alias {alias:?} conflicts with canonical model {alias:?}");
      }
      insert_alias(&mut map, alias, model)?;
    }
  }
  Ok(map)
}

fn insert_alias(map: &mut BTreeMap<String, String>, alias: &str, model: &str) -> Result<()> {
  match map.insert(norm(alias), norm(model)) {
    Some(prev) if prev != norm(model) => bail!("alias {alias:?} maps to both {prev:?} and {model:?}"),
    _ => Ok(()),
  }
}

fn read_bundled_providers(aliases: &BTreeMap<String, String>) -> Result<BTreeMap<String, ProviderEntry>> {
  let raw: BTreeMap<String, ProviderEntry> =
    serde_json::from_str(BUNDLED_PROVIDERS).context("parsing bundled providers.json")?;
  let mut out = BTreeMap::new();
  for (provider, entry) in raw {
    let provider = norm(&provider);
    let dst = out.entry(provider.clone()).or_insert_with(ProviderEntry::default);
    dst.multiplier = entry.multiplier;
    dst.included = entry.included;
    dst.source = entry.source;
    for (model, mo) in entry.models {
      let model = resolve_alias(aliases, &provider, &norm(&model));
      dst.models.insert(model, mo);
    }
  }
  Ok(out)
}

#[derive(Debug, Clone, Default)]
struct ModelsDevRow {
  provider: String,
  model: String,
  name: Option<String>,
  input_cost: Option<f64>,
  output_cost: Option<f64>,
  reasoning_cost: Option<f64>,
  cache_read_cost: Option<f64>,
  cache_write_cost: Option<f64>,
}

fn flatten_models_dev(api: &BTreeMap<String, Value>, aliases: &BTreeMap<String, String>) -> Vec<ModelsDevRow> {
  let mut rows = Vec::new();
  for (provider_id, pv) in api {
    let provider = norm(provider_id);
    let Some(provider_models) = pv.get("models").and_then(|v| v.as_object()) else {
      continue;
    };
    for (name, mv) in provider_models {
      let Some(cost) = mv.get("cost").filter(|v| !v.is_null()) else {
        continue;
      };
      let name = norm(name);
      let model = resolve_alias(aliases, &provider, &name);
      rows.push(ModelsDevRow {
        provider: provider.clone(),
        name: (model != name).then_some(name),
        model,
        input_cost: cost_value(cost, &["input"]),
        output_cost: cost_value(cost, &["output"]),
        reasoning_cost: cost_value(cost, &["reasoning"]),
        cache_read_cost: cost_value(cost, &["cache_read"]),
        cache_write_cost: cost_value(cost, &["cache_write"]),
      });
    }
  }
  rows
}

fn infer_providers(rows: &[ModelsDevRow]) -> BTreeMap<String, ProviderEntry> {
  let included_pairs: BTreeSet<(String, String)> = rows
    .iter()
    .filter(|r| zero_signal(r))
    .map(|r| (r.provider.clone(), r.model.clone()))
    .collect();
  let mut by_provider: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
  for row in rows {
    by_provider
      .entry(row.provider.clone())
      .or_default()
      .insert(row.model.clone());
  }
  let mut providers = BTreeMap::new();
  for (provider, models) in by_provider {
    let all_included = models.len() >= 2
      && models
        .iter()
        .all(|m| included_pairs.contains(&(provider.clone(), m.clone())));
    let entry = providers.entry(provider.clone()).or_insert_with(ProviderEntry::default);
    if all_included {
      entry.included = Some(true);
    } else {
      for model in models {
        if included_pairs.contains(&(provider.clone(), model.clone())) {
          entry.models.entry(model).or_default().included = Some(true);
        }
      }
    }
  }
  providers
}

fn merge_providers(dst: &mut BTreeMap<String, ProviderEntry>, src: BTreeMap<String, ProviderEntry>) {
  for (provider, src_entry) in src {
    let dst_entry = dst.entry(provider).or_default();
    if src_entry.multiplier.is_some() {
      dst_entry.multiplier = src_entry.multiplier;
    }
    if src_entry.included.is_some() {
      dst_entry.included = src_entry.included;
    }
    if src_entry.source.is_some() {
      dst_entry.source = src_entry.source;
    }
    for (model, src_model) in src_entry.models {
      let dst_model = dst_entry.models.entry(model).or_default();
      if src_model.multiplier.is_some() {
        dst_model.multiplier = src_model.multiplier;
      }
      if src_model.included.is_some() {
        dst_model.included = src_model.included;
      }
    }
  }
}

fn normalize_providers(providers: &mut BTreeMap<String, ProviderEntry>) {
  for entry in providers.values_mut() {
    if entry.multiplier == Some(1.0) {
      entry.multiplier = None;
    }
    if entry.included == Some(false) {
      entry.included = None;
    }
    if entry.source == Some(false) {
      entry.source = None;
    }
    let provider_included = entry.included.unwrap_or(false);
    entry.models.retain(|_, mo| {
      if mo.included == Some(provider_included) {
        mo.included = None;
      }
      mo.multiplier.is_some() || mo.included.is_some()
    });
  }
  providers.retain(|_, e| e.multiplier.is_some() || e.included.is_some() || e.source.is_some() || !e.models.is_empty());
}

fn build_prices(rows: &[ModelsDevRow], providers: &BTreeMap<String, ProviderEntry>) -> Vec<PriceRow> {
  rows
    .iter()
    .filter(|row| !zero_signal(row))
    .filter(|row| !included_for(providers, &row.provider, &row.model))
    .map(|row| PriceRow {
      provider: row.provider.clone(),
      model: row.model.clone(),
      name: row.name.clone(),
      input: row.input_cost.unwrap_or(0.0),
      output: row.output_cost.unwrap_or(0.0),
      reasoning: row.reasoning_cost,
      cache_read: row.cache_read_cost.unwrap_or(0.0),
      cache_write: row.cache_write_cost,
    })
    .filter(|row| !price_is_zero(row))
    .collect()
}

fn included_for(providers: &BTreeMap<String, ProviderEntry>, provider: &str, model: &str) -> bool {
  let Some(entry) = providers.get(provider) else {
    return false;
  };
  entry
    .models
    .get(model)
    .and_then(|m| m.included)
    .unwrap_or_else(|| entry.included.unwrap_or(false))
}

fn zero_signal(row: &ModelsDevRow) -> bool {
  let costs = [
    row.input_cost,
    row.output_cost,
    row.reasoning_cost,
    row.cache_read_cost,
    row.cache_write_cost,
  ];
  costs.iter().any(Option::is_some) && costs.iter().all(|v| v.unwrap_or(0.0) == 0.0)
}

fn price_is_zero(row: &PriceRow) -> bool {
  row.input == 0.0
    && row.output == 0.0
    && row.reasoning.unwrap_or(0.0) == 0.0
    && row.cache_read == 0.0
    && row.cache_write.unwrap_or(0.0) == 0.0
}

fn resolve_alias(aliases: &BTreeMap<String, String>, provider: &str, model: &str) -> String {
  aliases
    .get(&format!("{}/{}", norm(provider), norm(model)))
    .or_else(|| aliases.get(&norm(model)))
    .cloned()
    .unwrap_or_else(|| norm(model))
}

fn cost_value(v: &Value, keys: &[&str]) -> Option<f64> {
  for key in keys {
    if let Some(n) = v.get(*key).and_then(|x| x.as_f64()) {
      return Some(n);
    }
  }
  None
}

fn format_pricing_file(file: &PricingFile) -> Result<String> {
  let providers = indent_json(&serde_json::to_string_pretty(&file.providers)?, 2);
  let models = indent_json(&serde_json::to_string_pretty(&file.models)?, 2);
  let mut out = String::new();
  out.push_str("{\n");
  out.push_str("  \"providers\": ");
  out.push_str(providers.trim_start());
  out.push_str(",\n");
  out.push_str("  \"models\": ");
  out.push_str(models.trim_start());
  out.push_str(",\n");
  out.push_str("  \"prices\": [\n");
  for (i, price) in file.prices.iter().enumerate() {
    if i > 0 {
      out.push_str(",\n");
    }
    out.push_str("    ");
    out.push_str(&serde_json::to_string(price)?);
  }
  out.push_str("\n  ]\n}\n");
  Ok(out)
}

fn indent_json(s: &str, spaces: usize) -> String {
  let pad = " ".repeat(spaces);
  s.lines()
    .map(|line| format!("{pad}{line}"))
    .collect::<Vec<_>>()
    .join("\n")
}

fn dedupe_prices(rows: Vec<PriceRow>) -> Vec<PriceRow> {
  let mut out: BTreeMap<(String, String), PriceRow> = BTreeMap::new();
  for row in rows {
    let key = (row.provider.clone(), row.model.clone());
    out
      .entry(key)
      .and_modify(|existing| {
        if prefer_price(&row, existing) {
          *existing = row.clone();
        }
      })
      .or_insert(row);
  }
  out.into_values().collect()
}

fn prefer_price(new: &PriceRow, old: &PriceRow) -> bool {
  let new_name_matches = new.name.as_deref().is_none_or(|n| n == new.model);
  let old_name_matches = old.name.as_deref().is_none_or(|n| n == old.model);
  new_name_matches && !old_name_matches
}

fn token_cost_breakdown(r: &UsageRecord, p: &Price) -> CostBreakdown {
  let m = 1_000_000.0_f64;
  let reasoning_rate = p.reasoning.unwrap_or(p.output);
  let cache_write_rate = p.cache_write.unwrap_or(p.input);
  CostBreakdown {
    prompt: r.prompt as f64 * p.input / m,
    completion: r.completion as f64 * p.output / m,
    reasoning: r.reasoning as f64 * reasoning_rate / m,
    cache_read: r.cache_read as f64 * p.cache_read / m,
    cache_write: r.cache_write as f64 * cache_write_rate / m,
  }
}

fn scale_cost_breakdown(cost: CostBreakdown, mult: f64) -> CostBreakdown {
  CostBreakdown {
    prompt: cost.prompt * mult,
    completion: cost.completion * mult,
    reasoning: cost.reasoning * mult,
    cache_read: cost.cache_read * mult,
    cache_write: cost.cache_write * mult,
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::model_name;

  fn table() -> PricingTable {
    PricingTable::load_bundled()
  }

  #[test]
  fn fuzzy_date_suffix() {
    let t = table();
    assert_eq!(
      t.canonical_model(None, Some("claude-3-haiku-20240307")),
      "claude-3-haiku"
    );
    assert_eq!(
      t.canonical_model(None, Some("claude-opus-4-5-20251101")),
      "claude-opus-4.5"
    );
    assert_eq!(
      t.canonical_model(None, Some("claude-opus-4-5@20251101")),
      "claude-opus-4.5"
    );
    assert_eq!(
      t.canonical_model(None, Some("claude-opus-4-7-20251101")),
      "claude-opus-4.7"
    );
    assert_eq!(t.canonical_model(None, Some("gpt-5-2025-08-07")), "gpt-5");
    assert_eq!(t.canonical_model(None, Some("gpt-5-mini-2025-08-07")), "gpt-5-mini");
    assert_eq!(t.canonical_model(None, Some("o4-mini-2025-04-16")), "openai-o4-mini");
  }

  #[test]
  fn fuzzy_mode_suffix() {
    let t = table();
    assert_eq!(
      t.canonical_model(None, Some("claude-opus-4-5-20251101-thinking")),
      "claude-opus-4.5"
    );
    assert_eq!(
      t.canonical_model(None, Some("claude-opus-4-5-20251101:thinking")),
      "claude-opus-4.5"
    );
    assert_eq!(t.canonical_model(None, Some("claude-opus-4-6-fast")), "claude-opus-4.6");
    assert_eq!(
      t.canonical_model(None, Some("claude-opus-4-6-think")),
      "claude-opus-4.6"
    );
    assert_eq!(
      t.canonical_model(None, Some("claude-opus-4-6-thinking")),
      "claude-opus-4.6"
    );
    assert_eq!(
      t.canonical_model(None, Some("claude-opus-4.7-thinking")),
      "claude-opus-4.7"
    );
    assert_eq!(
      t.canonical_model(None, Some("claude-opus-4-7-thinking")),
      "claude-opus-4.7"
    );
  }

  #[test]
  fn fuzzy_chat_suffix() {
    let t = table();
    assert_eq!(t.canonical_model(None, Some("gpt-5-chat-latest")), "gpt-5");
    assert_eq!(t.canonical_model(None, Some("gpt-5-chat")), "gpt-5");
    assert_eq!(t.canonical_model(None, Some("gpt-5.1-chat-latest")), "gpt-5.1");
    assert_eq!(t.canonical_model(None, Some("gpt-5.1-chat")), "gpt-5.1");
    assert_eq!(t.canonical_model(None, Some("gpt-5.2-chat")), "gpt-5.2");
    assert_eq!(t.canonical_model(None, Some("gpt-5.2-chat-latest")), "gpt-5.2");
    assert_eq!(t.canonical_model(None, Some("gpt-5.3-chat-latest")), "gpt-5.3-chat");
  }

  #[test]
  fn fuzzy_preview_suffix() {
    let t = table();
    assert_eq!(
      t.canonical_model(None, Some("gemini-3.1-pro-preview")),
      "gemini-3.1-pro"
    );
    assert_eq!(
      t.canonical_model(None, Some("gemini-3.1-flash-image-preview")),
      "gemini-3.1-flash-image"
    );
    assert_eq!(
      t.canonical_model(None, Some("gemini-3.1-flash-lite-preview")),
      "gemini-3.1-flash-lite"
    );
    assert_eq!(t.canonical_model(None, Some("gemini-3-pro-preview")), "gemini-3-pro");
    assert_eq!(
      t.canonical_model(None, Some("gemini-3-flash-preview")),
      "gemini-3-flash"
    );
  }

  #[test]
  fn fuzzy_provider_dash_prefix() {
    let t = table();
    assert_eq!(t.canonical_model(None, Some("openai-gpt-5")), "gpt-5");
    assert_eq!(
      t.canonical_model(None, Some("openai-gpt-5.1-codex-max")),
      "gpt-5.1-codex"
    );
    assert_eq!(
      t.canonical_model(None, Some("anthropic-claude-opus-4.5")),
      "claude-opus-4.5"
    );
    assert_eq!(
      t.canonical_model(None, Some("anthropic-claude-opus-4.6")),
      "claude-opus-4.6"
    );
    assert_eq!(
      t.canonical_model(None, Some("anthropic-claude-opus-4.7")),
      "claude-opus-4.7"
    );
  }

  #[test]
  fn fuzzy_slash_prefix() {
    let t = table();
    assert_eq!(
      t.canonical_model(None, Some("anthropic/claude-sonnet-4-5")),
      "claude-sonnet-4.5"
    );
    assert_eq!(t.canonical_model(None, Some("openai/gpt-5")), "gpt-5");
    assert_eq!(t.canonical_model(None, Some("google/gemini-2.5-pro")), "gemini-2.5-pro");
    assert_eq!(t.canonical_model(None, Some("zai/glm-5.1")), "glm-5.1");
    assert_eq!(t.canonical_model(None, Some("zai-org/glm-5.1")), "glm-5.1");
  }

  #[test]
  fn fuzzy_version_sep() {
    let t = table();
    assert_eq!(t.canonical_model(None, Some("claude-sonnet-4-5")), "claude-sonnet-4.5");
    assert_eq!(t.canonical_model(None, Some("claude-3-5-haiku")), "claude-3.5-haiku");
    assert_eq!(t.canonical_model(None, Some("claude-3-5-sonnet")), "claude-3.5-sonnet");
    assert_eq!(t.canonical_model(None, Some("claude-3-7-sonnet")), "claude-3.7-sonnet");
    assert_eq!(t.canonical_model(None, Some("gpt-4-1")), "gpt-4.1");
    assert_eq!(t.canonical_model(None, Some("gpt-4-1-mini")), "gpt-4.1-mini");
    assert_eq!(t.canonical_model(None, Some("gpt-5-3-codex")), "gpt-5.3-codex");
    assert_eq!(t.canonical_model(None, Some("gpt-5-4")), "gpt-5.4");
    assert_eq!(t.canonical_model(None, Some("gpt-5-4-mini")), "gpt-5.4-mini");
    assert_eq!(t.canonical_model(None, Some("gpt-5-5")), "gpt-5.5");
    assert_eq!(t.canonical_model(None, Some("gpt-5-6")), "gpt-5.6-sol");
    assert_eq!(t.canonical_model(None, Some("glm-4-7")), "glm-4.7");
    assert_eq!(t.canonical_model(None, Some("glm-4-6")), "glm-4.6");
    assert_eq!(t.canonical_model(None, Some("glm-4-5")), "glm-4.5");
    assert_eq!(t.canonical_model(None, Some("glm-5-1")), "glm-5.1");
  }

  #[test]
  fn gpt_5_6_alias_and_prices() {
    let t = table();
    assert_eq!(t.canonical_model(None, Some("gpt-5.6")), "gpt-5.6-sol");

    let cases = [
      ("gpt-5.6-sol", 5.0, 30.0, 0.5, 6.25),
      ("gpt-5.6-terra", 2.5, 15.0, 0.25, 3.125),
      ("gpt-5.6-luna", 1.0, 6.0, 0.1, 1.25),
    ];
    for (model, input, output, cache_read, cache_write) in cases {
      let price = t
        .lookup_official_base(Some("openai"), Some(model))
        .unwrap_or_else(|| panic!("missing bundled price for {model}"));
      assert_eq!(price.input, input, "input price for {model}");
      assert_eq!(price.output, output, "output price for {model}");
      assert_eq!(price.cache_read, cache_read, "cache-read price for {model}");
      assert_eq!(price.cache_write, Some(cache_write), "cache-write price for {model}");
    }
  }

  #[test]
  fn fuzzy_combined_strips() {
    let t = table();
    assert_eq!(
      t.canonical_model(None, Some("claude-opus-4-6@default")),
      "claude-opus-4.6"
    );
    assert_eq!(
      t.canonical_model(None, Some("claude-sonnet-4-5-20250929")),
      "claude-sonnet-4.5"
    );
    assert_eq!(
      t.canonical_model(None, Some("claude-sonnet-4-5@20250929")),
      "claude-sonnet-4.5"
    );
    assert_eq!(
      t.canonical_model(None, Some("claude-opus-4-5-20251101-thinking")),
      "claude-opus-4.5"
    );
  }

  #[test]
  fn fuzzy_provider_model_passthrough() {
    let t = table();
    assert_eq!(t.canonical_model(None, Some("o1-preview")), "openai-o1");
    assert_eq!(t.canonical_model(None, Some("claude-opus-4-0")), "claude-opus-4");
    assert_eq!(t.canonical_model(None, Some("claude-sonnet-4-0")), "claude-sonnet-4");
  }

  #[test]
  fn fuzzy_unknown_returns_normalized() {
    let t = table();
    assert_eq!(t.canonical_model(None, Some("future-model-xyz")), "future-model-xyz");
  }

  #[test]
  fn strip_date_suffix_cases() {
    assert_eq!(
      model_name::strip_date_suffix("claude-opus-4-5-20251101"),
      "claude-opus-4-5"
    );
    assert_eq!(
      model_name::strip_date_suffix("claude-opus-4-5@20251101"),
      "claude-opus-4-5"
    );
    assert_eq!(model_name::strip_date_suffix("gpt-5-2025-08-07"), "gpt-5");
    assert_eq!(
      model_name::strip_date_suffix("claude-opus-4-6@default"),
      "claude-opus-4-6"
    );
    assert_eq!(model_name::strip_date_suffix("gpt-5"), "gpt-5");
  }

  #[test]
  fn strip_mode_suffix_cases() {
    assert_eq!(
      model_name::strip_mode_suffix("claude-opus-4-5-thinking"),
      "claude-opus-4-5"
    );
    assert_eq!(
      model_name::strip_mode_suffix("claude-opus-4-5:thinking"),
      "claude-opus-4-5"
    );
    assert_eq!(model_name::strip_mode_suffix("claude-opus-4-6-fast"), "claude-opus-4-6");
    assert_eq!(model_name::strip_mode_suffix("gpt-5"), "gpt-5");
  }

  #[test]
  fn strip_variant_suffix_cases() {
    assert_eq!(model_name::strip_variant_suffix("gpt-5-chat-latest"), "gpt-5-chat");
    assert_eq!(model_name::strip_variant_suffix("gpt-5-chat"), "gpt-5");
    assert_eq!(model_name::strip_variant_suffix("gpt-5.3-chat-latest"), "gpt-5.3-chat");
    assert_eq!(model_name::strip_variant_suffix("gpt-5"), "gpt-5");
    assert_eq!(
      model_name::strip_variant_suffix("gemini-3.1-pro-preview"),
      "gemini-3.1-pro"
    );
  }

  #[test]
  fn strip_provider_prefix_cases() {
    assert_eq!(model_name::strip_provider_prefix("openai-gpt-5"), "gpt-5");
    assert_eq!(
      model_name::strip_provider_prefix("anthropic-claude-opus-4.5"),
      "claude-opus-4.5"
    );
    assert_eq!(model_name::strip_provider_prefix("zai-org-glm-5.1"), "glm-5.1");
    assert_eq!(model_name::strip_provider_prefix("gpt-5"), "gpt-5");
  }

  #[test]
  fn strip_slash_prefix_cases() {
    assert_eq!(
      model_name::strip_slash_prefix("anthropic/claude-sonnet-4-5"),
      "claude-sonnet-4-5"
    );
    assert_eq!(model_name::strip_slash_prefix("openai/gpt-5"), "gpt-5");
    assert_eq!(model_name::strip_slash_prefix("gpt-5"), "gpt-5");
  }

  #[test]
  fn regression_reported_cases() {
    let t = table();
    let cases = vec![
      ("openai/gpt-5.1-chat", "gpt-5.1"),
      ("google/gemini-3-flash-preview", "gemini-3-flash"),
      ("zai-org/glm-5.1", "glm-5.1"),
      ("claude-sonnet-4-6", "claude-sonnet-4.6"),
      ("claude-opus-4-6-fast", "claude-opus-4.6"),
      ("openai/gpt-5.1-codex-max", "gpt-5.1-codex"),
      ("anthropic/claude-opus-4-6", "claude-opus-4.6"),
      ("claude-3-5-haiku-20241022", "claude-3.5-haiku"),
    ];
    for (input, expected) in cases {
      let got = t.canonical_model(None, Some(input));
      assert_eq!(got, expected, "canonical_model({input:?})");
    }
  }
}
