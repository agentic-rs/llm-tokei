use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
struct ModelInfo {
    provider: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    aliases: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
struct ModelOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    multiplier: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    included: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
struct ProviderEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    multiplier: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    included: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source: Option<bool>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    models: BTreeMap<String, ModelOverride>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
struct PricingFile {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    providers: BTreeMap<String, ProviderEntry>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    models: BTreeMap<String, ModelInfo>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    prices: Vec<PriceRow>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
struct PriceRow {
    provider: String,
    model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(default, skip_serializing_if = "is_zero")]
    input: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    output: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reasoning: Option<f64>,
    #[serde(default, skip_serializing_if = "is_zero")]
    cache_read: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    cache_write: f64,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
struct CsvRow {
    provider: String,
    model: String,
    name: String,
    input_cost: Option<f64>,
    output_cost: Option<f64>,
    reasoning_cost: Option<f64>,
    cache_read_cost: Option<f64>,
    cache_write_cost: Option<f64>,
    audio_input_cost: Option<f64>,
    audio_output_cost: Option<f64>,
}

fn main() -> Result<()> {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    let data = manifest.join("data");
    let models_path = data.join("models.json");
    let providers_path = data.join("providers.json");
    let models_dev_csv_path = data.join("models.dev.csv");
    let override_csv_path = data.join("prices.override.csv");

    for path in [
        &models_path,
        &providers_path,
        &models_dev_csv_path,
        &override_csv_path,
    ] {
        println!("cargo:rerun-if-changed={}", path.display());
    }

    let models = read_models(&models_path)?;
    let aliases = build_alias_map(&models)?;
    let rows = read_csv(&models_dev_csv_path, &aliases)?;
    let overrides = read_csv(&override_csv_path, &aliases)?;
    let merged = merge_rows(rows, overrides);

    let mut providers = infer_providers(&merged);
    let explicit_providers = read_providers(&providers_path, &aliases)?;
    merge_providers(&mut providers, explicit_providers);
    normalize_providers(&mut providers);

    let mut prices = build_prices(&merged, &providers);
    prices.sort_by(|a, b| (&a.provider, &a.model, &a.name).cmp(&(&b.provider, &b.model, &b.name)));
    prices = dedupe_prices(prices);

    let out = PricingFile {
        providers,
        models,
        prices,
    };
    let json = format_pricing_file(&out)?;
    let out_dir = PathBuf::from(std::env::var("OUT_DIR")?);
    std::fs::write(out_dir.join("prices.json"), json).context("writing generated prices.json")?;
    Ok(())
}

fn read_models(path: &Path) -> Result<BTreeMap<String, ModelInfo>> {
    let s = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let raw: BTreeMap<String, ModelInfo> =
        serde_json::from_str(&s).with_context(|| format!("parsing {}", path.display()))?;
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
        Some(prev) if prev != norm(model) => {
            bail!("alias {alias:?} maps to both {prev:?} and {model:?}")
        }
        _ => Ok(()),
    }
}

fn read_csv(path: &Path, aliases: &BTreeMap<String, String>) -> Result<Vec<CsvRow>> {
    let mut rdr = csv::ReaderBuilder::new()
        .flexible(true)
        .from_path(path)
        .with_context(|| format!("reading {}", path.display()))?;
    let mut rows = Vec::new();
    for result in rdr.deserialize() {
        let mut row: CsvRow = result.with_context(|| format!("parsing {}", path.display()))?;
        row.provider = norm(&row.provider);
        row.name = norm(if row.name.is_empty() {
            &row.model
        } else {
            &row.name
        });
        row.model = resolve_alias(aliases, &row.provider, &norm(&row.model));
        rows.push(row);
    }
    Ok(rows)
}

fn merge_rows(base: Vec<CsvRow>, overrides: Vec<CsvRow>) -> Vec<CsvRow> {
    let mut out: BTreeMap<(String, String, String), CsvRow> = BTreeMap::new();
    for row in base {
        out.insert(
            (row.provider.clone(), row.model.clone(), row.name.clone()),
            row,
        );
    }
    for row in overrides {
        let key = (row.provider.clone(), row.model.clone(), row.name.clone());
        out.entry(key)
            .and_modify(|base| overlay_csv_row(base, &row))
            .or_insert(row);
    }
    out.into_values().collect()
}

fn overlay_csv_row(base: &mut CsvRow, over: &CsvRow) {
    if over.input_cost.is_some() {
        base.input_cost = over.input_cost;
    }
    if over.output_cost.is_some() {
        base.output_cost = over.output_cost;
    }
    if over.reasoning_cost.is_some() {
        base.reasoning_cost = over.reasoning_cost;
    }
    if over.cache_read_cost.is_some() {
        base.cache_read_cost = over.cache_read_cost;
    }
    if over.cache_write_cost.is_some() {
        base.cache_write_cost = over.cache_write_cost;
    }
    if over.audio_input_cost.is_some() {
        base.audio_input_cost = over.audio_input_cost;
    }
    if over.audio_output_cost.is_some() {
        base.audio_output_cost = over.audio_output_cost;
    }
}

fn read_providers(
    path: &Path,
    aliases: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, ProviderEntry>> {
    let s = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let raw: BTreeMap<String, ProviderEntry> =
        serde_json::from_str(&s).with_context(|| format!("parsing {}", path.display()))?;
    let mut out = BTreeMap::new();
    for (provider, entry) in raw {
        let provider = norm(&provider);
        let dst = out
            .entry(provider.clone())
            .or_insert_with(ProviderEntry::default);
        dst.multiplier = entry.multiplier;
        dst.included = entry.included;
        for (model, mo) in entry.models {
            let model = resolve_alias(aliases, &provider, &norm(&model));
            dst.models.insert(model, mo);
        }
    }
    Ok(out)
}

fn infer_providers(rows: &[CsvRow]) -> BTreeMap<String, ProviderEntry> {
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
        let entry = providers
            .entry(provider.clone())
            .or_insert_with(ProviderEntry::default);
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

fn merge_providers(
    dst: &mut BTreeMap<String, ProviderEntry>,
    src: BTreeMap<String, ProviderEntry>,
) {
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
    providers.retain(|_, e| {
        e.multiplier.is_some() || e.included.is_some() || e.source.is_some() || !e.models.is_empty()
    });
}

fn build_prices(rows: &[CsvRow], providers: &BTreeMap<String, ProviderEntry>) -> Vec<PriceRow> {
    rows.iter()
        .filter(|row| !zero_signal(row))
        .filter(|row| !included_for(providers, &row.provider, &row.model))
        .map(|row| PriceRow {
            provider: row.provider.clone(),
            model: row.model.clone(),
            name: (row.name != row.model).then(|| row.name.clone()),
            input: row.input_cost.unwrap_or(0.0),
            output: row.output_cost.unwrap_or(0.0),
            reasoning: row.reasoning_cost,
            cache_read: row.cache_read_cost.unwrap_or(0.0),
            cache_write: row.cache_write_cost.unwrap_or(0.0),
        })
        .filter(|row| !price_is_zero(row))
        .collect()
}

fn dedupe_prices(rows: Vec<PriceRow>) -> Vec<PriceRow> {
    let mut out: BTreeMap<(String, String), PriceRow> = BTreeMap::new();
    for row in rows {
        let key = (row.provider.clone(), row.model.clone());
        out.entry(key)
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

fn zero_signal(row: &CsvRow) -> bool {
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
        && row.cache_write == 0.0
}

fn resolve_alias(aliases: &BTreeMap<String, String>, provider: &str, model: &str) -> String {
    aliases
        .get(&format!("{}/{}", norm(provider), norm(model)))
        .or_else(|| aliases.get(&norm(model)))
        .cloned()
        .unwrap_or_else(|| norm(model))
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

fn norm(s: &str) -> String {
    s.trim().to_lowercase()
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}
