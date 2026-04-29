//! Fetches https://models.dev/api.json and writes `data/models.dev.csv`.
//!
//! Run: `cargo run --example fetch_prices`

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

const MODELS_DEV_URL: &str = "https://models.dev/api.json";

#[derive(Debug, Clone, Default, Deserialize)]
struct ModelInfo {
    provider: String,
    #[serde(default)]
    aliases: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
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
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let models_path = manifest.join("data/models.json");
    let models_dev_csv_path = manifest.join("data/models.dev.csv");

    let models = read_models(&models_path)?;
    let aliases = build_alias_map(&models)?;

    eprintln!("Fetching {MODELS_DEV_URL} ...");
    let body = ureq::get(MODELS_DEV_URL)
        .timeout(std::time::Duration::from_secs(30))
        .call()
        .context("requesting models.dev")?
        .into_string()
        .context("reading models.dev response body")?;
    let api: BTreeMap<String, Value> =
        serde_json::from_str(&body).context("parsing models.dev JSON")?;
    eprintln!("  got {} providers", api.len());

    let mut rows = flatten_models_dev(&api, &aliases);
    rows.sort_by(|a, b| (&a.provider, &a.model, &a.name).cmp(&(&b.provider, &b.model, &b.name)));
    write_csv(&models_dev_csv_path, &rows)?;
    eprintln!("  wrote {} rows to {}", rows.len(), models_dev_csv_path.display());
    Ok(())
}

fn read_models(path: &PathBuf) -> Result<BTreeMap<String, ModelInfo>> {
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

fn flatten_models_dev(api: &BTreeMap<String, Value>, aliases: &BTreeMap<String, String>) -> Vec<CsvRow> {
    let mut rows = Vec::new();
    for (provider_id, pv) in api {
        let provider = norm(provider_id);
        let Some(models) = pv.get("models").and_then(|v| v.as_object()) else {
            continue;
        };
        for (name, mv) in models {
            let Some(cost) = mv.get("cost").filter(|v| !v.is_null()) else {
                continue;
            };
            let name = norm(name);
            rows.push(CsvRow {
                provider: provider.clone(),
                model: resolve_alias(aliases, &provider, &name),
                name,
                input_cost: cost_value(cost, &["input"]),
                output_cost: cost_value(cost, &["output"]),
                reasoning_cost: cost_value(cost, &["reasoning"]),
                cache_read_cost: cost_value(cost, &["cache_read"]),
                cache_write_cost: cost_value(cost, &["cache_write"]),
                audio_input_cost: cost_value(cost, &["audio_input", "input_audio"]),
                audio_output_cost: cost_value(cost, &["audio_output", "output_audio"]),
            });
        }
    }
    rows
}

fn write_csv(path: &PathBuf, rows: &[CsvRow]) -> Result<()> {
    let mut w = csv::Writer::from_path(path).with_context(|| format!("creating {}", path.display()))?;
    for row in rows {
        w.serialize(row)?;
    }
    w.flush()?;
    Ok(())
}

fn resolve_alias(aliases: &BTreeMap<String, String>, provider: &str, model: &str) -> String {
    aliases
        .get(&format!("{}/{}", norm(provider), norm(model)))
        .or_else(|| aliases.get(&norm(model)))
        .cloned()
        .unwrap_or_else(|| norm(model))
}

fn cost_value(v: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter()
        .find_map(|k| v.get(*k).and_then(|x| x.as_f64()))
}

fn norm(s: &str) -> String {
    s.trim().to_lowercase()
}
