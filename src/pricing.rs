use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::model::{ServiceTier, UsageRecord};
use crate::model_data::{CachedModelData, HistoricalPrices};
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

pub struct PricedUsage {
  pub costs: CostBreakdown,
  pub base_cost: f64,
  pub service_tier_multiplier: Option<f64>,
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
  #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
  pub service_tier_multipliers: BTreeMap<ServiceTier, Vec<ServiceTierRate>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServiceTierRate {
  pub factor: f64,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub since: Option<DateTime<Utc>>,
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
  family_routes: BTreeMap<(String, String), Vec<String>>,
  historical_prices: HistoricalPrices,
}

const BUNDLED_MODELS: &str = include_str!("../data/models.json");
const BUNDLED_PROVIDERS: &str = include_str!("../data/providers.json");
impl PricingTable {
  #[cfg(test)]
  pub fn load_bundled() -> Self {
    Self::load_bundled_result().expect("embedded model data must be valid")
  }

  pub fn load_default() -> Result<Self> {
    let mut table = Self::load_static_config()?;
    table.merge_model_data(CachedModelData::load_freshest()?);
    Ok(table)
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

  #[cfg(test)]
  fn load_bundled_result() -> Result<Self> {
    let mut table = Self::load_static_config()?;
    table.merge_model_data(CachedModelData::load_bundled()?);
    Ok(table)
  }

  fn load_static_config() -> Result<Self> {
    let models = serde_json::from_str(BUNDLED_MODELS).context("parsing bundled model metadata")?;
    let providers = serde_json::from_str(BUNDLED_PROVIDERS).context("parsing bundled provider metadata")?;
    let mut table = Self::default();
    table.merge(PricingFile {
      providers,
      models,
      prices: Vec::new(),
    });
    Ok(table)
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
        slot.service_tier_multipliers.extend(mv.service_tier_multipliers);
      }
    }

    for row in file.prices {
      let provider = norm(&row.provider);
      let model = self.canonical_model_strict(Some(&provider), Some(&row.model));
      self.prices.insert((provider, model), row.into());
    }
  }

  fn merge_model_data(&mut self, model_data: CachedModelData) {
    for route in model_data.families {
      self.aliases.insert(
        format!("{}/{}", route.provider, route.model),
        route.canonical_name.clone(),
      );
      let routes = self
        .family_routes
        .entry((route.provider, route.canonical_name))
        .or_default();
      if !routes.contains(&route.model) {
        routes.push(route.model);
      }
    }
    self.historical_prices = model_data.prices;
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

  fn lookup_standard_at<'a>(
    &'a self,
    reported_provider: Option<&'a str>,
    reported_model: &str,
    canonical: &str,
    ts: DateTime<Utc>,
  ) -> Option<(&'a str, &'a Price)> {
    if canonical == "-" {
      return None;
    }
    let official_provider = self.models.get(canonical).map(|info| info.provider.as_str());
    let routes = [
      reported_provider.map(|provider| (provider, reported_model)),
      official_provider.map(|provider| (provider, canonical)),
    ];
    // As before, try historical rates on both routes before static rates.
    routes
      .into_iter()
      .flatten()
      .find_map(|(provider, model)| {
        self
          .lookup_historical(provider, model, canonical, ts)
          .map(|price| (provider, price))
      })
      .or_else(|| {
        routes.into_iter().flatten().find_map(|(provider, _)| {
          self
            .prices
            .get(&(norm(provider), canonical.to_owned()))
            .map(|price| (provider, price))
        })
      })
  }

  #[cfg(test)]
  fn lookup_official_base(&self, provider: Option<&str>, model: Option<&str>) -> Option<&Price> {
    let canonical = self.canonical_model(provider, model);
    self
      .lookup_standard_at(None, &canonical, &canonical, DateTime::<Utc>::MAX_UTC)
      .map(|(_, price)| price)
  }

  fn lookup_historical(
    &self,
    provider: &str,
    reported_model: &str,
    canonical: &str,
    ts: chrono::DateTime<chrono::Utc>,
  ) -> Option<&Price> {
    if let Some(price) = self.historical_prices.lookup(provider, reported_model, ts) {
      return Some(price);
    }
    if reported_model != canonical {
      if let Some(price) = self.historical_prices.lookup(provider, canonical, ts) {
        return Some(price);
      }
    }
    self
      .family_routes
      .get(&(norm(provider), canonical.to_string()))
      .and_then(|routes| {
        routes
          .iter()
          .find_map(|route| self.historical_prices.lookup(provider, route, ts))
      })
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

  pub fn priced_usage(&self, r: &UsageRecord, mode: CostMode) -> Option<PricedUsage> {
    let provider = r.provider.as_deref();
    let model = r.model.as_deref();
    let canonical = self.canonical_model(provider, model);
    let provider_price = if mode == CostMode::Official {
      None
    } else {
      self.lookup_standard_at(provider, model.unwrap_or(&canonical), &canonical, r.ts)
    };
    let included =
      self.lookup_included(provider, model) || provider_price.is_some_and(|(_, price)| price_is_zero(price));

    if mode == CostMode::Actual && included {
      return Some(PricedUsage {
        costs: CostBreakdown::default(),
        base_cost: 0.0,
        service_tier_multiplier: None,
      });
    }
    let (price_provider, price) = if mode == CostMode::Official || included {
      self.lookup_standard_at(None, &canonical, &canonical, r.ts)?
    } else {
      provider_price?
    };
    let mut costs = token_cost_breakdown(r, price);
    if mode == CostMode::Actual {
      costs = scale_cost_breakdown(costs, self.lookup_multiplier(provider, model));
    }
    let base_cost = costs.total();
    let service_tier_multiplier = if base_cost > 0.0 {
      r.service_tier
        .and_then(|tier| self.service_tier_multiplier(price_provider, &canonical, tier, r.ts))
    } else {
      None
    };
    if let Some(factor) = service_tier_multiplier {
      costs = scale_cost_breakdown(costs, factor);
    }
    Some(PricedUsage {
      costs,
      base_cost,
      service_tier_multiplier,
    })
  }

  fn service_tier_multiplier(&self, provider: &str, model: &str, tier: ServiceTier, ts: DateTime<Utc>) -> Option<f64> {
    let rates = self
      .providers
      .get(&norm(provider))?
      .models
      .get(model)?
      .service_tier_multipliers
      .get(&tier)?;
    rates
      .iter()
      .filter(|rate| rate.since.is_none_or(|since| since <= ts))
      .max_by_key(|rate| rate.since)
      .map(|rate| rate.factor)
      .filter(|factor| factor.is_finite() && *factor > 0.0 && *factor != 1.0)
  }

  #[cfg(test)]
  fn cost_breakdown_for(&self, r: &UsageRecord, mode: CostMode) -> Option<CostBreakdown> {
    self.priced_usage(r, mode).map(|usage| usage.costs)
  }
}

fn price_is_zero(price: &Price) -> bool {
  price.input == 0.0
    && price.output == 0.0
    && price.reasoning.unwrap_or(0.0) == 0.0
    && price.cache_read == 0.0
    && price.cache_write.unwrap_or(0.0) == 0.0
}

pub fn update_cached_prices() -> Result<PathBuf> {
  crate::model_data::update_cached_model_data()
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
  use crate::model::{SessionKind, Source};
  use crate::model_data::FamilyRoute;
  use crate::model_name;
  use chrono::{TimeZone, Utc};

  fn table() -> PricingTable {
    PricingTable::load_bundled()
  }

  fn usage_record(ts: chrono::DateTime<Utc>, provider: &str, model: &str) -> UsageRecord {
    UsageRecord {
      source: Source::Codex,
      session_id: "session".into(),
      session_kind: SessionKind::Root,
      parent_session_id: None,
      session_title: None,
      project_cwd: None,
      project_name: None,
      provider: Some(provider.into()),
      model: Some(model.into()),
      service_tier: None,
      ts,
      prompt: 1_000_000,
      completion: 0,
      input_bytes: 0,
      output_bytes: 0,
      input_estimated: false,
      output_estimated: false,
      input_bytes_estimated: false,
      output_bytes_estimated: false,
      reasoning: 0,
      cache_read: 0,
      cache_write: 0,
      total_direct: None,
      mode: None,
      agent: None,
      is_compaction: false,
      rounds: 1,
      calls: 1,
      cost_embedded: None,
    }
  }

  #[test]
  fn cost_uses_provider_route_price_at_record_time() {
    let history = HistoricalPrices::from_csv(
      b"op,ts,commit_sha,sequence,provider,model,input,output,reasoning,cache_read,cache_write,input_audio,output_audio\n\
upsert,2025-06-01T00:00:00Z,aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa,1,test-provider,Test-Route,1,2,,0.1,,,\n\
delete,2025-07-01T00:00:00Z,bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb,2,test-provider,Test-Route,,,,,,,,\n\
upsert,2025-08-01T00:00:00Z,cccccccccccccccccccccccccccccccccccccccc,3,test-provider,Test-Route,3,4,,0.3,,,\n",
    )
    .unwrap();
    let mut table = table();
    table.merge_model_data(CachedModelData {
      families: vec![FamilyRoute {
        canonical_name: "gpt-5".into(),
        model: "test-route".into(),
        provider: "test-provider".into(),
      }],
      prices: history,
    });

    let before = usage_record(
      Utc.with_ymd_and_hms(2025, 5, 1, 0, 0, 0).unwrap(),
      "test-provider",
      "Test-Route",
    );
    let deleted = usage_record(
      Utc.with_ymd_and_hms(2025, 7, 15, 0, 0, 0).unwrap(),
      "test-provider",
      "Test-Route",
    );
    let reappeared = usage_record(
      Utc.with_ymd_and_hms(2025, 8, 2, 0, 0, 0).unwrap(),
      "test-provider",
      "Test-Route",
    );

    assert_eq!(
      table.canonical_model(before.provider.as_deref(), before.model.as_deref()),
      "gpt-5"
    );
    assert_eq!(
      table.cost_breakdown_for(&before, CostMode::Actual).unwrap().total(),
      1.0
    );
    assert_eq!(
      table.cost_breakdown_for(&deleted, CostMode::Actual).unwrap().total(),
      1.0
    );
    assert_eq!(
      table.cost_breakdown_for(&reappeared, CostMode::Actual).unwrap().total(),
      3.0
    );
  }

  #[test]
  fn bundled_gpt_6_prices_cover_provider_and_official_routes() {
    let table = table();
    let usage = usage_record(
      Utc.with_ymd_and_hms(2026, 9, 5, 3, 0, 0).unwrap(),
      "openrouter",
      "openai/gpt-6-astra-pro",
    );

    assert_eq!(
      table.canonical_model(usage.provider.as_deref(), usage.model.as_deref()),
      "gpt-6-astra"
    );
    assert_eq!(
      table.cost_breakdown_for(&usage, CostMode::Actual).unwrap().total(),
      10.0
    );
    assert_eq!(
      table.cost_breakdown_for(&usage, CostMode::Official).unwrap().total(),
      10.0
    );
  }

  #[test]
  fn bundled_gpt_6_2_prices_cover_sol_and_astra() {
    let table = table();
    let ts = Utc.with_ymd_and_hms(2026, 11, 22, 0, 0, 0).unwrap();
    for (model, input, output, cache_read, cache_write) in [
      ("gpt-6.2-sol", 2.0, 10.0, 0.1, 2.5),
      ("gpt-6.2-astra", 8.0, 40.0, 0.4, 10.0),
    ] {
      let route = format!("openai/{model}");
      let mut usage = usage_record(ts, "openrouter", &route);
      usage.completion = 1_000_000;
      usage.cache_read = 1_000_000;
      usage.cache_write = 1_000_000;

      assert_eq!(table.canonical_model(Some("openrouter"), Some(&route)), model);
      for mode in [CostMode::Actual, CostMode::Official] {
        let costs = table.cost_breakdown_for(&usage, mode).unwrap();
        assert_eq!(costs.prompt, input, "{model}");
        assert_eq!(costs.completion, output, "{model}");
        assert_eq!(costs.cache_read, cache_read, "{model}");
        assert_eq!(costs.cache_write, cache_write, "{model}");
      }
    }
  }

  #[test]
  fn bundled_service_tiers_apply_dated_multipliers_to_all_cost_components() {
    let table = table();
    let mut usage = usage_record(
      Utc.with_ymd_and_hms(2027, 3, 1, 7, 59, 59).unwrap(),
      "openai",
      "gpt-6.2-sol",
    );
    usage.completion = 1_000_000;
    usage.cache_read = 1_000_000;
    usage.cache_write = 1_000_000;
    usage.reasoning = 1_000_000;

    for mode in [CostMode::Actual, CostMode::Mixed, CostMode::Official] {
      usage.service_tier = Some(ServiceTier::Fast);
      let priced = table.priced_usage(&usage, mode).unwrap();
      assert_eq!(priced.service_tier_multiplier, Some(2.0));
      assert_eq!(priced.costs.prompt, 4.0);
      assert_eq!(priced.costs.completion, 20.0);
      assert_eq!(priced.costs.reasoning, 20.0);
      assert_eq!(priced.costs.cache_read, 0.2);
      assert_eq!(priced.costs.cache_write, 5.0);

      usage.service_tier = Some(ServiceTier::Ultrafast);
      let promotional = table.priced_usage(&usage, mode).unwrap();
      assert_eq!(promotional.service_tier_multiplier, Some(3.0));
      usage.ts += chrono::Duration::seconds(1);
      let list_price = table.priced_usage(&usage, mode).unwrap();
      assert_eq!(list_price.service_tier_multiplier, Some(6.0));
      assert_eq!(list_price.costs.total(), promotional.costs.total() * 2.0);
      usage.ts -= chrono::Duration::seconds(1);
    }
  }

  #[test]
  fn service_tier_pricing_respects_provider_routes_and_included_usage() {
    let table = table();
    let ts = Utc.with_ymd_and_hms(2026, 11, 25, 0, 0, 0).unwrap();
    let mut usage = usage_record(ts, "vercel", "openai/gpt-6.2-sol-fast");
    usage.service_tier = Some(ServiceTier::Fast);
    let routed = table.priced_usage(&usage, CostMode::Actual).unwrap();
    assert_eq!(routed.service_tier_multiplier, None);
    assert_eq!(routed.costs.total(), 4.0);

    usage.provider = Some("github-copilot".into());
    usage.model = Some("gpt-6.2-sol".into());
    let included = table.priced_usage(&usage, CostMode::Actual).unwrap();
    assert_eq!(included.service_tier_multiplier, None);
    assert_eq!(included.costs.total(), 0.0);
    let official_fallback = table.priced_usage(&usage, CostMode::Mixed).unwrap();
    assert_eq!(official_fallback.service_tier_multiplier, Some(2.0));
    assert_eq!(official_fallback.costs.total(), 4.0);

    usage.model = Some("gpt-6-sol".into());
    usage.service_tier = Some(ServiceTier::Ultrafast);
    let unavailable = table.priced_usage(&usage, CostMode::Official).unwrap();
    assert_eq!(unavailable.service_tier_multiplier, None);
    assert_eq!(unavailable.costs.total(), unavailable.base_cost);
  }

  #[test]
  fn zero_history_price_is_included_for_actual_and_mixed_costs() {
    let history = HistoricalPrices::from_csv(
      b"op,ts,commit_sha,sequence,provider,model,input,output,reasoning,cache_read,cache_write,input_audio,output_audio\n\
upsert,2025-01-01T00:00:00Z,aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa,1,free-provider,free-route,0,0,,0,,,\n\
upsert,2025-01-01T00:00:00Z,bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb,2,openai,gpt-5,2,10,,0.2,,,\n",
    )
    .unwrap();
    let mut table = table();
    table.merge_model_data(CachedModelData {
      families: vec![
        FamilyRoute {
          canonical_name: "gpt-5".into(),
          model: "free-route".into(),
          provider: "free-provider".into(),
        },
        FamilyRoute {
          canonical_name: "gpt-5".into(),
          model: "gpt-5".into(),
          provider: "openai".into(),
        },
      ],
      prices: history,
    });
    let usage = usage_record(
      Utc.with_ymd_and_hms(2025, 2, 1, 0, 0, 0).unwrap(),
      "free-provider",
      "free-route",
    );

    assert_eq!(table.cost_breakdown_for(&usage, CostMode::Actual).unwrap().total(), 0.0);
    assert_eq!(table.cost_breakdown_for(&usage, CostMode::Mixed).unwrap().total(), 2.0);
  }

  #[test]
  fn history_without_reported_provider_uses_official_route_at_record_time() {
    let history = HistoricalPrices::from_csv(
      b"op,ts,commit_sha,sequence,provider,model,input,output,reasoning,cache_read,cache_write,input_audio,output_audio\n\
upsert,2025-01-01T00:00:00Z,aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa,1,openai,gpt-5,1,10,,0.1,,,\n\
upsert,2025-03-01T00:00:00Z,bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb,2,openai,gpt-5,3,30,,0.3,,,\n",
    )
    .unwrap();
    let mut table = table();
    table.merge_model_data(CachedModelData {
      families: vec![FamilyRoute {
        canonical_name: "gpt-5".into(),
        model: "gpt-5".into(),
        provider: "openai".into(),
      }],
      prices: history,
    });
    let mut usage = usage_record(Utc.with_ymd_and_hms(2025, 2, 1, 0, 0, 0).unwrap(), "unused", "gpt-5");
    usage.provider = None;

    assert_eq!(table.cost_breakdown_for(&usage, CostMode::Actual).unwrap().total(), 1.0);
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
      ("gpt-5.6-sol", 4.0, 20.0, 0.4, 5.0),
      ("gpt-5.6-terra", 2.0, 12.0, 0.2, 2.5),
      ("gpt-5.6-luna", 0.2, 1.2, 0.02, 0.25),
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
