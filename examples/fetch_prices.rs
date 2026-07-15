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

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ModelInfo {
  provider: String,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  name: Option<String>,
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
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
  name: Option<String>,
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
  alias_candidates: usize,
  alias_rejected_price: usize,
  alias_skipped_ambiguous: usize,
  alias_skipped_no_source_price: usize,
}

#[derive(Debug, Clone, Copy, Default)]
struct SourceAliasStats {
  canonical_with_aliases: usize,
  alias_rows_checked: usize,
  alias_cost_mismatches: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct PriceKey {
  input: Option<i64>,
  output: Option<i64>,
  reasoning: Option<i64>,
  cache_read: Option<i64>,
  cache_write: Option<i64>,
  audio_input: Option<i64>,
  audio_output: Option<i64>,
}

fn main() -> Result<()> {
  let auto_import = std::env::args().any(|a| a == "--auto-import");
  let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
  let models_path = manifest.join("npm/model-catalog/src/models.json");
  let providers_path = manifest.join("data/providers.json");
  let models_dev_csv_path = manifest.join("data/models.dev.csv");
  let cache_dir = manifest.join(CACHE_DIR);
  std::fs::create_dir_all(&cache_dir).with_context(|| format!("creating {}", cache_dir.display()))?;
  let log_path = cache_dir.join(FETCH_LOG_FILE);
  let mut log =
    BufWriter::new(std::fs::File::create(&log_path).with_context(|| format!("creating {}", log_path.display()))?);

  let mut models = read_models(&models_path)?;
  let source_providers = read_source_providers(&providers_path)?;

  let body = load_models_dev_json(&cache_dir, &mut log)?;
  let api: BTreeMap<String, Value> = serde_json::from_str(&body).context("parsing models.dev JSON")?;
  eprintln!("  got {} providers", api.len());

  if auto_import {
    let imported = auto_import_models(&mut models, &api, &source_providers, &mut log)?;
    if imported > 0 {
      write_models(&models_path, &models)?;
      eprintln!("  auto-imported {} models into {}", imported, models_path.display());
    } else {
      eprintln!("  no new models to auto-import");
    }
  }

  let aliases = build_alias_map(&models)?;
  check_model_names_in_source_provider(&api, &models, &source_providers, &mut log)?;
  let source_prices = build_source_price_index(&api, &aliases, &models, &source_providers);
  let source_alias_stats =
    check_source_alias_cost_mismatch(&api, &aliases, &models, &source_providers, &source_prices, &mut log)?;

  let mut stats = FlattenStats::default();
  let mut rows = flatten_models_dev(
    &api,
    &aliases,
    &models,
    &source_providers,
    &source_prices,
    &mut log,
    &mut stats,
  )?;
  rows.sort_by(|a, b| (&a.provider, &a.model, &a.name).cmp(&(&b.provider, &b.model, &b.name)));
  write_csv(&models_dev_csv_path, &rows)?;
  writeln!(
        log,
        "[summary] resolved_rows={} unresolved_rows={} unresolved_source_rows={} substring_hits={} alias_candidates={} alias_rejected_price={} alias_skipped_ambiguous={} alias_skipped_no_source_price={}",
        stats.resolved_rows,
        stats.unresolved_rows,
        stats.unresolved_source_rows,
        stats.substring_hits,
        stats.alias_candidates,
        stats.alias_rejected_price,
        stats.alias_skipped_ambiguous,
        stats.alias_skipped_no_source_price
    )?;
  log.flush().context("flushing fetch log")?;

  eprintln!("  log: {}", log_path.display());
  eprintln!("  wrote {} rows to {}", rows.len(), models_dev_csv_path.display());
  eprintln!(
    "  unresolved rows: {} (source warnings: {}, substring hits: {}, alias candidates: {})",
    stats.unresolved_rows, stats.unresolved_source_rows, stats.substring_hits, stats.alias_candidates
  );
  eprintln!(
    "  source alias rows checked: {} (canonicals with aliases: {}, mismatches: {})",
    source_alias_stats.alias_rows_checked,
    source_alias_stats.canonical_with_aliases,
    source_alias_stats.alias_cost_mismatches
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
    v.name = v.name.map(|n| norm(&n));
    v.aliases = v.aliases.into_iter().map(|a| norm(&a)).collect();
    out.insert(norm(&k), v);
  }
  Ok(out)
}

fn read_source_providers(path: &Path) -> Result<BTreeSet<String>> {
  let s = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
  let raw: BTreeMap<String, ProviderInfo> =
    serde_json::from_str(&s).with_context(|| format!("parsing {}", path.display()))?;
  Ok(
    raw
      .into_iter()
      .filter_map(|(provider, entry)| entry.source.then(|| norm(&provider)))
      .collect(),
  )
}

fn build_alias_map(models: &BTreeMap<String, ModelInfo>) -> Result<BTreeMap<String, String>> {
  let canonicals: BTreeSet<&str> = models.keys().map(|s| s.as_str()).collect();
  let mut map = BTreeMap::new();
  for (model, info) in models {
    insert_alias(&mut map, model, model)?;
    let name = info.name.as_deref().unwrap_or(model.as_str());
    insert_alias(&mut map, name, model)?;
    insert_alias(&mut map, &format!("{}/{}", info.provider, name), model)?;
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
      return std::fs::read_to_string(&cache_path).with_context(|| format!("reading {}", cache_path.display()));
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
  let config = ureq::Agent::config_builder()
    .timeout_global(Some(Duration::from_secs(30)))
    .build();
  let agent: ureq::Agent = config.into();
  let mut response = agent.get(MODELS_DEV_URL).call().context("requesting models.dev")?;
  let body = response
    .body_mut()
    .read_to_string()
    .context("reading models.dev response body")?;

  let temp_path = cache_dir.join(format!("{MODELS_DEV_CACHE_FILE}.tmp"));
  std::fs::write(&temp_path, &body).with_context(|| format!("writing {}", temp_path.display()))?;
  std::fs::rename(&temp_path, &cache_path)
    .with_context(|| format!("renaming {} to {}", temp_path.display(), cache_path.display()))?;
  writeln!(log, "[cache-write] path={} bytes={}", cache_path.display(), body.len())?;
  Ok(body)
}

fn cache_age(path: &Path) -> Result<Duration> {
  let modified = std::fs::metadata(path)
    .with_context(|| format!("reading {} metadata", path.display()))?
    .modified()
    .with_context(|| format!("reading {} mtime", path.display()))?;
  Ok(
    SystemTime::now()
      .duration_since(modified)
      .unwrap_or_else(|_| Duration::from_secs(0)),
  )
}

fn build_source_price_index(
  api: &BTreeMap<String, Value>,
  aliases: &BTreeMap<String, String>,
  models: &BTreeMap<String, ModelInfo>,
  source_providers: &BTreeSet<String>,
) -> BTreeMap<String, PriceKey> {
  let mut prices = BTreeMap::new();
  for (provider_id, pv) in api {
    let provider = norm(provider_id);
    if !source_providers.contains(&provider) {
      continue;
    }
    let Some(provider_models) = pv.get("models").and_then(|v| v.as_object()) else {
      continue;
    };
    for (name, mv) in provider_models {
      let Some(cost) = mv.get("cost").filter(|v| !v.is_null()) else {
        continue;
      };
      let name = norm(name);
      let Some(model) = resolve_alias(aliases, &provider, &name) else {
        continue;
      };
      let Some(info) = models.get(&model) else {
        continue;
      };
      if info.provider == provider {
        let key = price_key(cost);
        let current_is_canonical = name == model;
        match prices.get_mut(&model) {
          None => {
            prices.insert(model, key);
          }
          Some(prev) if current_is_canonical => {
            *prev = key;
          }
          _ => {}
        }
      }
    }
  }
  prices
}

fn check_source_alias_cost_mismatch<W: Write>(
  api: &BTreeMap<String, Value>,
  aliases: &BTreeMap<String, String>,
  models: &BTreeMap<String, ModelInfo>,
  source_providers: &BTreeSet<String>,
  source_prices: &BTreeMap<String, PriceKey>,
  log: &mut W,
) -> Result<SourceAliasStats> {
  let mut stats = SourceAliasStats::default();
  let mut canonical_with_aliases = BTreeSet::new();

  for (provider_id, pv) in api {
    let provider = norm(provider_id);
    if !source_providers.contains(&provider) {
      continue;
    }
    let Some(provider_models) = pv.get("models").and_then(|v| v.as_object()) else {
      continue;
    };

    for (name, mv) in provider_models {
      let Some(cost) = mv.get("cost").filter(|v| !v.is_null()) else {
        continue;
      };
      let name = norm(name);
      let Some(model) = resolve_alias(aliases, &provider, &name) else {
        continue;
      };
      let Some(info) = models.get(&model) else {
        continue;
      };
      if info.provider != provider || name == model {
        continue;
      }
      if is_preview_variant(&name) {
        continue;
      }

      stats.alias_rows_checked += 1;
      canonical_with_aliases.insert(model.clone());
      let got = price_key(cost);
      if let Some(source) = source_prices.get(&model) {
        if source != &got {
          stats.alias_cost_mismatches += 1;
          let diff = source.diff(&got);
          let warning = format!(
                        "warning: source alias cost mismatch provider='{provider}' canonical='{model}' alias='{name}' diff='{diff}'"
                    );
          writeln!(log, "[alias-cost-mismatch] {warning}")?;
          eprintln!("{warning}");
        }
      }
    }
  }

  stats.canonical_with_aliases = canonical_with_aliases.len();
  writeln!(
    log,
    "[source-alias-summary] canonical_with_aliases={} alias_rows_checked={} alias_cost_mismatches={}",
    stats.canonical_with_aliases, stats.alias_rows_checked, stats.alias_cost_mismatches
  )?;
  Ok(stats)
}

fn check_model_names_in_source_provider<W: Write>(
  api: &BTreeMap<String, Value>,
  models: &BTreeMap<String, ModelInfo>,
  source_providers: &BTreeSet<String>,
  log: &mut W,
) -> Result<()> {
  for (model_id, info) in models {
    if !source_providers.contains(&info.provider) {
      continue;
    }
    let Some(provider) = api.get(&info.provider) else {
      continue;
    };
    let Some(provider_models) = provider.get("models").and_then(|v| v.as_object()) else {
      continue;
    };
    let model_name = info.name.as_deref().unwrap_or(model_id.as_str());
    if !provider_models.contains_key(model_name) {
      let warning = format!(
        "warning: model '{model_id}' name '{model_name}' not found in source provider '{}'",
        info.provider
      );
      writeln!(log, "[model-name-missing] {warning}")?;
      eprintln!("{warning}");
    }
  }
  Ok(())
}

fn flatten_models_dev<W: Write>(
  api: &BTreeMap<String, Value>,
  aliases: &BTreeMap<String, String>,
  models: &BTreeMap<String, ModelInfo>,
  source_providers: &BTreeSet<String>,
  source_prices: &BTreeMap<String, PriceKey>,
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
      let model = match resolve_alias(aliases, &provider, &name) {
        Some(model) => {
          stats.resolved_rows += 1;
          Some(model)
        }
        None => {
          stats.unresolved_rows += 1;
          let candidates = substring_candidates(models, &name);
          for candidate in &candidates {
            stats.substring_hits += 1;
            writeln!(log, "[substring] provider={provider} name={name} candidate={candidate}")?;
          }

          match candidates.as_slice() {
            [] => {}
            [candidate] => log_alias_candidate(&provider, &name, cost, candidate, source_prices, log, stats)?,
            _ => {
              stats.alias_skipped_ambiguous += 1;
              writeln!(
                log,
                "[alias-skipped-ambiguous] name={}/{} candidates={}",
                provider,
                name,
                candidates.join(",")
              )?;
            }
          }

          if source_providers.contains(&provider) && should_warn_unresolved_source_model(&name) {
            stats.unresolved_source_rows += 1;
            let warning = format!(
                        "warning: source provider '{provider}' has unmapped model '{name}' (no canonical id; add to npm/model-catalog/src/models.json)"
                    );
            writeln!(log, "[warning] {warning}")?;
            eprintln!("{warning}");
          }

          None
        }
      };

      let (model, name) = match model {
        Some(model) => (model, Some(name)),
        None => (name, None),
      };
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

fn substring_candidates(models: &BTreeMap<String, ModelInfo>, name: &str) -> Vec<String> {
  models
    .iter()
    .filter(|(model, _)| name.contains(model.as_str()))
    .map(|(model, _)| model.clone())
    .collect()
}

fn log_alias_candidate<W: Write>(
  provider: &str,
  name: &str,
  cost: &Value,
  candidate: &str,
  source_prices: &BTreeMap<String, PriceKey>,
  log: &mut W,
  stats: &mut FlattenStats,
) -> Result<()> {
  let got = price_key(cost);
  let Some(source) = source_prices.get(candidate) else {
    stats.alias_skipped_no_source_price += 1;
    writeln!(
      log,
      "[alias-skipped-no-source-price] canonical={candidate} name={provider}/{name} got={}",
      got.display()
    )?;
    return Ok(());
  };

  if *source == got {
    stats.alias_candidates += 1;
    writeln!(
      log,
      "[alias-candidate] canonical={candidate} name={provider}/{name} price={}",
      got.display()
    )?;
  } else {
    stats.alias_rejected_price += 1;
    writeln!(
      log,
      "[alias-rejected-price] canonical={candidate} name={provider}/{name} source={} got={}",
      source.display(),
      got.display()
    )?;
  }

  Ok(())
}

fn price_key(cost: &Value) -> PriceKey {
  PriceKey {
    input: rounded_cost(cost, &["input"]),
    output: rounded_cost(cost, &["output"]),
    reasoning: rounded_cost(cost, &["reasoning"]),
    cache_read: rounded_cost(cost, &["cache_read"]),
    cache_write: rounded_cost(cost, &["cache_write"]),
    audio_input: rounded_cost(cost, &["audio_input", "input_audio"]),
    audio_output: rounded_cost(cost, &["audio_output", "output_audio"]),
  }
}

fn rounded_cost(v: &Value, keys: &[&str]) -> Option<i64> {
  cost_value(v, keys).map(|n| (n * 10_000.0).round() as i64)
}

impl PriceKey {
  fn display(&self) -> String {
    format!(
      "input={} output={} reasoning={} cache_read={} cache_write={} audio_input={} audio_output={}",
      display_cost(self.input),
      display_cost(self.output),
      display_cost(self.reasoning),
      display_cost(self.cache_read),
      display_cost(self.cache_write),
      display_cost(self.audio_input),
      display_cost(self.audio_output)
    )
  }

  fn diff(&self, other: &Self) -> String {
    let mut out = Vec::new();
    push_diff(&mut out, "input", self.input, other.input);
    push_diff(&mut out, "output", self.output, other.output);
    push_diff(&mut out, "reasoning", self.reasoning, other.reasoning);
    push_diff(&mut out, "cache_read", self.cache_read, other.cache_read);
    push_diff(&mut out, "cache_write", self.cache_write, other.cache_write);
    push_diff(&mut out, "audio_input", self.audio_input, other.audio_input);
    push_diff(&mut out, "audio_output", self.audio_output, other.audio_output);
    if out.is_empty() {
      "(none)".to_string()
    } else {
      out.join(", ")
    }
  }
}

fn display_cost(v: Option<i64>) -> String {
  v.map(|n| format!("{:.4}", n as f64 / 10_000.0))
    .unwrap_or_else(|| "-".to_string())
}

fn push_diff(out: &mut Vec<String>, name: &str, base: Option<i64>, got: Option<i64>) {
  if base != got {
    out.push(format!("{name}: {} -> {}", display_cost(base), display_cost(got)));
  }
}

fn should_warn_unresolved_source_model(model: &str) -> bool {
  !is_preview_variant(model)
}

fn is_preview_variant(model: &str) -> bool {
  model.contains("-preview-")
}

fn write_csv(path: &Path, rows: &[CsvRow]) -> Result<()> {
  let mut w = csv::Writer::from_path(path).with_context(|| format!("creating {}", path.display()))?;
  for row in rows {
    w.serialize(row)?;
  }
  w.flush()?;
  Ok(())
}

fn resolve_alias(aliases: &BTreeMap<String, String>, provider: &str, model: &str) -> Option<String> {
  llm_tokei::model_name::resolve_alias(aliases, provider, model)
}

fn auto_import_models<W: Write>(
  models: &mut BTreeMap<String, ModelInfo>,
  api: &BTreeMap<String, Value>,
  source_providers: &BTreeSet<String>,
  log: &mut W,
) -> Result<usize> {
  let aliases = build_alias_map(models)?;
  let mut imported = 0;
  for (provider_id, pv) in api {
    let provider = norm(provider_id);
    if !source_providers.contains(&provider) {
      continue;
    }
    let Some(provider_models) = pv.get("models").and_then(|v| v.as_object()) else {
      continue;
    };
    for (name, mv) in provider_models {
      let Some(cost) = mv.get("cost").filter(|v| !v.is_null()) else {
        continue;
      };
      let name = norm(name);
      if resolve_alias(&aliases, &provider, &name).is_some() {
        continue;
      }
      if !should_warn_unresolved_source_model(&name) {
        continue;
      }
      if !has_nonzero_cost(cost) {
        continue;
      }
      let canonical = name.clone();
      models.entry(canonical.clone()).or_insert_with(|| {
        imported += 1;
        writeln!(log, "[auto-import] provider={} model={}", provider, canonical).ok();
        eprintln!("  + {provider}/{canonical}");
        ModelInfo {
          provider: provider.clone(),
          name: None,
          aliases: Vec::new(),
        }
      });
    }
  }
  Ok(imported)
}

fn has_nonzero_cost(cost: &Value) -> bool {
  let keys = [
    "input",
    "output",
    "reasoning",
    "cache_read",
    "cache_write",
    "audio_input",
    "input_audio",
    "audio_output",
    "output_audio",
  ];
  keys
    .iter()
    .any(|k| cost.get(k).and_then(|v| v.as_f64()).is_some_and(|v| v != 0.0))
}

fn write_models(path: &Path, models: &BTreeMap<String, ModelInfo>) -> Result<()> {
  let json = serde_json::to_string_pretty(models).with_context(|| format!("serializing {}", path.display()))?;
  std::fs::write(path, json).with_context(|| format!("writing {}", path.display()))
}

fn cost_value(v: &Value, keys: &[&str]) -> Option<f64> {
  keys.iter().find_map(|k| v.get(*k).and_then(|x| x.as_f64()))
}

fn norm(s: &str) -> String {
  s.trim().to_lowercase()
}
