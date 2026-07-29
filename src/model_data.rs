use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::model_name::norm;
use crate::pricing::Price;

const MANIFEST_URL: &str = "https://agentic.tokn-ai.dev/llm-tokei/models/manifest.json";
const CACHE_DIRECTORY: &str = "llm-tokei.models";
const CACHE_MANIFEST: &str = "manifest.json";
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
const MAX_ARTIFACT_BYTES: usize = 512 * 1024 * 1024;

#[derive(Debug, Clone, Deserialize, Serialize)]
struct ModelsManifest {
  schema_version: u32,
  source_repository: String,
  source_ref: String,
  source_commit_sha: String,
  prices: CsvArtifact,
  families: CsvArtifact,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct CsvArtifact {
  path: String,
  latest_path: String,
  bytes: usize,
  sha256: String,
}

#[derive(Debug, Clone)]
pub struct FamilyRoute {
  pub canonical_name: String,
  pub model: String,
  pub provider: String,
}

#[derive(Debug, Clone, Default)]
pub struct HistoricalPrices {
  routes: BTreeMap<(String, String), Vec<PriceEvent>>,
}

#[derive(Debug, Clone)]
struct PriceEvent {
  price: Price,
  sequence: u64,
  ts: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
struct PriceCsvRow {
  op: String,
  ts: DateTime<Utc>,
  sequence: u64,
  provider: String,
  model: String,
  input: Option<f64>,
  output: Option<f64>,
  reasoning: Option<f64>,
  cache_read: Option<f64>,
  cache_write: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct FamilyCsvRow {
  provider: String,
  model: String,
  canonical_name: String,
}

#[derive(Debug)]
pub struct CachedModelData {
  pub families: Vec<FamilyRoute>,
  pub prices: HistoricalPrices,
}

impl CachedModelData {
  pub fn load_default() -> Result<Option<Self>> {
    let Some(directory) = cached_model_data_path() else {
      return Ok(None);
    };
    let manifest_path = directory.join(CACHE_MANIFEST);
    if !manifest_path.exists() {
      return Ok(None);
    }
    let manifest_bytes =
      std::fs::read(&manifest_path).with_context(|| format!("reading {}", manifest_path.display()))?;
    let manifest: ModelsManifest =
      serde_json::from_slice(&manifest_bytes).with_context(|| format!("parsing {}", manifest_path.display()))?;
    validate_manifest(&manifest)?;

    let prices = read_verified_artifact(&directory, &manifest.prices)?;
    let families = read_verified_artifact(&directory, &manifest.families)?;
    Ok(Some(Self {
      prices: HistoricalPrices::from_csv(&prices)?,
      families: parse_families(&families)?,
    }))
  }
}

impl HistoricalPrices {
  pub(crate) fn from_csv(bytes: &[u8]) -> Result<Self> {
    let mut reader = csv::ReaderBuilder::new().flexible(true).from_reader(bytes);
    let mut routes: BTreeMap<(String, String), Vec<PriceEvent>> = BTreeMap::new();
    for row in reader.deserialize::<PriceCsvRow>() {
      let row = row.context("parsing historical prices CSV")?;
      match row.op.as_str() {
        "delete" => continue,
        "upsert" => {}
        other => bail!("historical prices CSV has unsupported operation {other:?}"),
      }
      let key = (norm(&row.provider), norm(&row.model));
      routes.entry(key).or_default().push(PriceEvent {
        price: Price {
          input: row.input.unwrap_or(0.0),
          output: row.output.unwrap_or(0.0),
          reasoning: row.reasoning,
          cache_read: row.cache_read.unwrap_or(0.0),
          cache_write: row.cache_write,
        },
        sequence: row.sequence,
        ts: row.ts,
      });
    }
    for events in routes.values_mut() {
      events.sort_by_key(|event| (event.ts, event.sequence));
    }
    Ok(Self { routes })
  }

  pub fn lookup(&self, provider: &str, model: &str, ts: DateTime<Utc>) -> Option<&Price> {
    let events = self.routes.get(&(norm(provider), norm(model)))?;
    let index = events.partition_point(|event| event.ts <= ts);
    events
      .get(index.saturating_sub(1))
      .or_else(|| events.first())
      .map(|event| &event.price)
  }
}

pub fn cached_model_data_path() -> Option<PathBuf> {
  std::env::var_os("XDG_CACHE_HOME")
    .map(PathBuf::from)
    .or_else(|| {
      std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|path| path.join(".cache"))
    })
    .map(|base| base.join(CACHE_DIRECTORY))
}

pub fn update_cached_model_data() -> Result<PathBuf> {
  let directory = cached_model_data_path().context("cannot determine cache directory")?;
  let agent = http_agent();
  let manifest_bytes = fetch(&agent, MANIFEST_URL, MAX_MANIFEST_BYTES).context("requesting model data manifest")?;
  let manifest: ModelsManifest = serde_json::from_slice(&manifest_bytes).context("parsing model data manifest")?;
  validate_manifest(&manifest)?;

  let base_url = MANIFEST_URL
    .strip_suffix(CACHE_MANIFEST)
    .context("model data manifest URL is invalid")?;
  let prices = fetch_artifact(&agent, base_url, &manifest.prices)?;
  let families = fetch_artifact(&agent, base_url, &manifest.families)?;

  HistoricalPrices::from_csv(&prices)?;
  parse_families(&families)?;

  std::fs::create_dir_all(&directory).with_context(|| format!("creating {}", directory.display()))?;
  write_artifact(&directory, &manifest.prices.path, &prices)?;
  write_artifact(&directory, &manifest.families.path, &families)?;
  let manifest_path = directory.join(CACHE_MANIFEST);
  write_atomic(&manifest_path, &manifest_bytes)?;
  remove_stale_artifacts(&directory, &manifest)?;
  Ok(manifest_path)
}

fn parse_families(bytes: &[u8]) -> Result<Vec<FamilyRoute>> {
  let mut reader = csv::ReaderBuilder::new().flexible(true).from_reader(bytes);
  let mut families = Vec::new();
  for row in reader.deserialize::<FamilyCsvRow>() {
    let row = row.context("parsing model families CSV")?;
    if row.canonical_name.trim().is_empty() {
      continue;
    }
    families.push(FamilyRoute {
      canonical_name: norm(&row.canonical_name),
      model: norm(&row.model),
      provider: norm(&row.provider),
    });
  }
  Ok(families)
}

fn validate_manifest(manifest: &ModelsManifest) -> Result<()> {
  if manifest.schema_version != 2 {
    bail!("unsupported model data manifest schema {}", manifest.schema_version);
  }
  if manifest.source_repository != "https://github.com/anomalyco/models.dev" {
    bail!("model data manifest source repository changed");
  }
  if manifest.source_ref != "dev" {
    bail!("model data manifest source ref changed");
  }
  if !is_hex(&manifest.source_commit_sha, 40, 64) {
    bail!("model data manifest source commit is invalid");
  }
  validate_artifact("changes", &manifest.prices)?;
  validate_artifact("families", &manifest.families)?;
  Ok(())
}

fn validate_artifact(name: &str, artifact: &CsvArtifact) -> Result<()> {
  if artifact.bytes > MAX_ARTIFACT_BYTES {
    bail!("{name} CSV exceeds the {MAX_ARTIFACT_BYTES}-byte safety limit");
  }
  if !is_hex(&artifact.sha256, 64, 64) {
    bail!("{name} CSV checksum is invalid");
  }
  if artifact.path != format!("{name}.{}.csv", artifact.sha256) {
    bail!("{name} CSV path does not match its checksum");
  }
  if artifact.latest_path != format!("{name}.csv") {
    bail!("{name} CSV shortcut path is invalid");
  }
  Ok(())
}

fn read_verified_artifact(directory: &Path, artifact: &CsvArtifact) -> Result<Vec<u8>> {
  let path = directory.join(&artifact.path);
  let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
  verify_artifact(artifact, &bytes)?;
  Ok(bytes)
}

fn fetch_artifact(agent: &ureq::Agent, base_url: &str, artifact: &CsvArtifact) -> Result<Vec<u8>> {
  let url = format!("{base_url}{}", artifact.path);
  let bytes =
    fetch(agent, &url, artifact.bytes.saturating_add(1)).with_context(|| format!("requesting {}", artifact.path))?;
  verify_artifact(artifact, &bytes)?;
  Ok(bytes)
}

fn fetch(agent: &ureq::Agent, url: &str, limit: usize) -> Result<Vec<u8>> {
  let mut response = agent.get(url).call()?;
  response
    .body_mut()
    .with_config()
    .limit(limit as u64)
    .read_to_vec()
    .context("reading response body")
}

fn verify_artifact(artifact: &CsvArtifact, bytes: &[u8]) -> Result<()> {
  if bytes.len() != artifact.bytes {
    bail!(
      "{} size mismatch: expected {}, received {}",
      artifact.path,
      artifact.bytes,
      bytes.len()
    );
  }
  let checksum = format!("{:x}", Sha256::digest(bytes));
  if checksum != artifact.sha256 {
    bail!("{} checksum mismatch", artifact.path);
  }
  Ok(())
}

fn write_artifact(directory: &Path, name: &str, bytes: &[u8]) -> Result<()> {
  let path = directory.join(name);
  if path.exists() {
    let existing = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    if existing == bytes {
      return Ok(());
    }
  }
  write_atomic(&path, bytes)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
  let temp = path.with_extension("tmp");
  std::fs::write(&temp, bytes).with_context(|| format!("writing {}", temp.display()))?;
  match std::fs::rename(&temp, path) {
    Ok(()) => Ok(()),
    Err(_) if path.exists() => {
      std::fs::remove_file(path).with_context(|| format!("replacing {}", path.display()))?;
      std::fs::rename(&temp, path).with_context(|| format!("renaming {} to {}", temp.display(), path.display()))
    }
    Err(error) => Err(error).with_context(|| format!("renaming {} to {}", temp.display(), path.display())),
  }
}

fn remove_stale_artifacts(directory: &Path, manifest: &ModelsManifest) -> Result<()> {
  let keep = [&manifest.prices.path, &manifest.families.path];
  for entry in std::fs::read_dir(directory).with_context(|| format!("reading {}", directory.display()))? {
    let entry = entry?;
    let name = entry.file_name();
    let name = name.to_string_lossy();
    let generated_artifact = (name.starts_with("changes.") || name.starts_with("families.")) && name.ends_with(".csv");
    if generated_artifact && !keep.iter().any(|current| current.as_str() == name) {
      std::fs::remove_file(entry.path()).with_context(|| format!("removing {}", entry.path().display()))?;
    }
  }
  Ok(())
}

fn http_agent() -> ureq::Agent {
  let config = ureq::Agent::config_builder()
    .timeout_global(Some(std::time::Duration::from_secs(30)))
    .build();
  config.into()
}

fn is_hex(value: &str, minimum: usize, maximum: usize) -> bool {
  (minimum..=maximum).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
  use super::*;
  use chrono::TimeZone;

  const HISTORY: &str = "\
op,ts,commit_sha,sequence,provider,model,input,output,reasoning,cache_read,cache_write,input_audio,output_audio
upsert,2025-06-01T00:00:00Z,aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa,1,openai,gpt-test,1,2,,0.1,,,
delete,2025-07-01T00:00:00Z,bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb,2,openai,gpt-test,,,,,,,,
upsert,2025-08-01T00:00:00Z,cccccccccccccccccccccccccccccccccccccccc,3,openai,gpt-test,3,4,,0.3,,,
";

  #[test]
  fn historical_lookup_uses_first_price_before_history() {
    let history = HistoricalPrices::from_csv(HISTORY.as_bytes()).unwrap();
    let ts = Utc.with_ymd_and_hms(2025, 5, 1, 0, 0, 0).unwrap();
    assert_eq!(history.lookup("openai", "gpt-test", ts).unwrap().input, 1.0);
  }

  #[test]
  fn historical_lookup_carries_price_across_delete() {
    let history = HistoricalPrices::from_csv(HISTORY.as_bytes()).unwrap();
    let ts = Utc.with_ymd_and_hms(2025, 7, 15, 0, 0, 0).unwrap();
    assert_eq!(history.lookup("openai", "gpt-test", ts).unwrap().input, 1.0);
  }

  #[test]
  fn historical_lookup_uses_new_price_after_reappearance() {
    let history = HistoricalPrices::from_csv(HISTORY.as_bytes()).unwrap();
    let ts = Utc.with_ymd_and_hms(2025, 8, 2, 0, 0, 0).unwrap();
    assert_eq!(history.lookup("openai", "gpt-test", ts).unwrap().input, 3.0);
  }
}
