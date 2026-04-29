//! Fetches https://models.dev/api.json and writes `data/models.dev.csv`.
//!
//! Run: `cargo run --example fetch_prices`

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const MODELS_DEV_URL: &str = "https://models.dev/api.json";
const CACHE_DIR: &str = ".cache";
const MODELS_DEV_CACHE_FILE: &str = "models.dev.api.json";
const FETCH_LOG_FILE: &str = "fetch_prices.log";
const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, Default, Deserialize)]
struct ModelInfo {
    provider: String,
    #[serde(default)]
    aliases: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ProviderInfo {
    #[serde(default)]
    source: bool,
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

#[derive(Debug, Clone, Default)]
struct FlattenStats {
    resolved_rows: usize,
    unresolved_rows: usize,
    unresolved_source_rows: usize,
    substring_hits: usize,
}

fn main() -> Result<()> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let models_path = manifest.join("data/models.json");
    let providers_path = manifest.join("data/providers.json");
    let models_dev_csv_path = manifest.join("data/models.dev.csv");
    let cache_dir = manifest.join(CACHE_DIR);
    std::fs::create_dir_all(&cache_dir)
        .with_context(|| format!("creating {}", cache_dir.display()))?;
    let log_path = cache_dir.join(FETCH_LOG_FILE);
    let mut log = BufWriter::new(
        std::fs::File::create(&log_path)
            .with_context(|| format!("creating {}", log_path.display()))?,
    );

    let models = read_models(&models_path)?;
    let aliases = build_alias_map(&models)?;
    let source_providers = read_source_providers(&providers_path)?;

    let body = load_models_dev_json(&cache_dir, &mut log)?;
    let api: BTreeMap<String, Value> =
        serde_json::from_str(&body).context("parsing models.dev JSON")?;
    eprintln!("  got {} providers", api.len());

    let mut stats = FlattenStats::default();
    let mut rows = flatten_models_dev(
        &api,
        &aliases,
        &models,
        &source_providers,
        &mut log,
        &mut stats,
    )?;
    rows.sort_by(|a, b| (&a.provider, &a.model, &a.name).cmp(&(&b.provider, &b.model, &b.name)));
    write_csv(&models_dev_csv_path, &rows)?;
    writeln!(
        log,
        "[summary] resolved_rows={} unresolved_rows={} unresolved_source_rows={} substring_hits={}",
        stats.resolved_rows,
        stats.unresolved_rows,
        stats.unresolved_source_rows,
        stats.substring_hits
    )?;
    log.flush().context("flushing fetch log")?;

    eprintln!("  log: {}", log_path.display());
    eprintln!(
        "  wrote {} rows to {}",
        rows.len(),
        models_dev_csv_path.display()
    );
    eprintln!(
        "  filtered unresolved rows: {} (source warnings: {}, substring hits: {})",
        stats.unresolved_rows, stats.unresolved_source_rows, stats.substring_hits
    );
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

fn read_source_providers(path: &Path) -> Result<BTreeSet<String>> {
    let s = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let raw: BTreeMap<String, ProviderInfo> =
        serde_json::from_str(&s).with_context(|| format!("parsing {}", path.display()))?;
    Ok(raw
        .into_iter()
        .filter_map(|(provider, entry)| entry.source.then(|| norm(&provider)))
        .collect())
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

fn load_models_dev_json<W: Write>(cache_dir: &Path, log: &mut W) -> Result<String> {
    let cache_path = cache_dir.join(MODELS_DEV_CACHE_FILE);
    if cache_path.exists() {
        let age = cache_age(&cache_path)?;
        if age <= CACHE_TTL {
            eprintln!(
                "Using cached {MODELS_DEV_URL} from {} (age {:.1}h)",
                cache_path.display(),
                age.as_secs_f64() / 3600.0
            );
            writeln!(
                log,
                "[cache-hit] path={} age_seconds={}",
                cache_path.display(),
                age.as_secs()
            )?;
            return std::fs::read_to_string(&cache_path)
                .with_context(|| format!("reading {}", cache_path.display()));
        }
        writeln!(
            log,
            "[cache-stale] path={} age_seconds={}",
            cache_path.display(),
            age.as_secs()
        )?;
    } else {
        writeln!(log, "[cache-miss] path={}", cache_path.display())?;
    }

    eprintln!("Fetching {MODELS_DEV_URL} ...");
    let body = ureq::get(MODELS_DEV_URL)
        .timeout(Duration::from_secs(30))
        .call()
        .context("requesting models.dev")?
        .into_string()
        .context("reading models.dev response body")?;

    let temp_path = cache_dir.join(format!("{MODELS_DEV_CACHE_FILE}.tmp"));
    std::fs::write(&temp_path, &body)
        .with_context(|| format!("writing {}", temp_path.display()))?;
    std::fs::rename(&temp_path, &cache_path).with_context(|| {
        format!(
            "renaming {} to {}",
            temp_path.display(),
            cache_path.display()
        )
    })?;
    writeln!(
        log,
        "[cache-write] path={} bytes={}",
        cache_path.display(),
        body.len()
    )?;
    Ok(body)
}

fn cache_age(path: &Path) -> Result<Duration> {
    let modified = std::fs::metadata(path)
        .with_context(|| format!("reading {} metadata", path.display()))?
        .modified()
        .with_context(|| format!("reading {} mtime", path.display()))?;
    Ok(SystemTime::now()
        .duration_since(modified)
        .unwrap_or_else(|_| Duration::from_secs(0)))
}

fn flatten_models_dev<W: Write>(
    api: &BTreeMap<String, Value>,
    aliases: &BTreeMap<String, String>,
    models: &BTreeMap<String, ModelInfo>,
    source_providers: &BTreeSet<String>,
    log: &mut W,
    stats: &mut FlattenStats,
) -> Result<Vec<CsvRow>> {
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
            let Some(model) = resolve_alias(aliases, &provider, &name) else {
                stats.unresolved_rows += 1;
                let candidates = substring_candidates(models, &provider, &name);
                if candidates.is_empty() {
                    writeln!(log, "[unresolved] provider={provider} name={name}")?;
                } else {
                    for candidate in candidates {
                        stats.substring_hits += 1;
                        writeln!(
                            log,
                            "[substring] provider={provider} name={name} candidate={candidate}"
                        )?;
                    }
                }
                if source_providers.contains(&provider) {
                    stats.unresolved_source_rows += 1;
                    let warning = format!(
                        "warning: source provider '{provider}' has unmapped model '{name}' (no canonical id; add to data/models.json)"
                    );
                    writeln!(log, "[warning] {warning}")?;
                    eprintln!("{warning}");
                }
                continue;
            };

            stats.resolved_rows += 1;
            rows.push(CsvRow {
                provider: provider.clone(),
                model,
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
    Ok(rows)
}

fn substring_candidates(
    models: &BTreeMap<String, ModelInfo>,
    provider: &str,
    name: &str,
) -> Vec<String> {
    models
        .iter()
        .filter(|(model, info)| info.provider == provider && name.contains(model.as_str()))
        .map(|(model, _)| model.clone())
        .collect()
}

fn write_csv(path: &Path, rows: &[CsvRow]) -> Result<()> {
    let mut w =
        csv::Writer::from_path(path).with_context(|| format!("creating {}", path.display()))?;
    for row in rows {
        w.serialize(row)?;
    }
    w.flush()?;
    Ok(())
}

fn resolve_alias(
    aliases: &BTreeMap<String, String>,
    provider: &str,
    model: &str,
) -> Option<String> {
    aliases
        .get(&format!("{}/{}", norm(provider), norm(model)))
        .or_else(|| aliases.get(&norm(model)))
        .cloned()
}

fn cost_value(v: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|k| v.get(*k).and_then(|x| x.as_f64()))
}

fn norm(s: &str) -> String {
    s.trim().to_lowercase()
}
