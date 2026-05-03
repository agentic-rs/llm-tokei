mod aggregate;
mod cache;
mod cli;
mod format;
mod model;
mod pricing;
mod sources;
mod time;

use anyhow::{Context, Result};
use clap::Parser;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::aggregate::{aggregate, sort_aggs, Filters, GroupDim, SortKey};
use crate::cache::{CacheDb, CacheStats};
use crate::cli::{Args, Format, Period};
use crate::format::{json::render_json, table::render_table};
use crate::model::UsageRecord;
use crate::pricing::PricingTable;
use crate::sources::{
  claude::ClaudeSource, codex::CodexSource, copilot::CopilotSource, opencode::OpenCodeSource, UsageSource,
};

fn main() -> Result<()> {
  let args = Args::parse();
  let use_color = !args.no_color && std::env::var_os("NO_COLOR").is_none();
  let cache = if args.no_cache {
    None
  } else {
    match CacheDb::open() {
      Ok(db) => Some(db),
      Err(e) => {
        if args.verbose {
          eprintln!("cache: error: {e:#}; falling back to direct parsing");
        }
        None
      }
    }
  };

  // Resolve sources.
  let want = args
    .source
    .as_ref()
    .map(|v| v.iter().map(|s| s.to_lowercase()).collect::<Vec<_>>())
    .unwrap_or_else(|| vec!["codex".into(), "opencode".into(), "claude".into(), "copilot".into()]);

  let mut all: Vec<UsageRecord> = Vec::new();

  if want.iter().any(|s| s == "codex") {
    let path = args.codex_dir.clone().or_else(CodexSource::default_path);
    if let Some(p) = path {
      let src = CodexSource::new(p);
      let result = if let Some(c) = cache.as_ref() {
        collect_one_record_source_with_cache(c, "codex", src.discover_files(), CodexSource::parse_file)
      } else {
        src.collect().map(|records| {
          let mut stats = CacheStats::new();
          stats.scanned = src.discover_files().len();
          (records, stats)
        })
      };
      match result {
        Ok((mut v, stats)) => {
          if args.verbose {
            eprintln!("{}", format_cache_stats("codex", "files", &stats));
          }
          all.append(&mut v);
        }
        Err(e) if args.verbose => eprintln!("codex: error: {e:#}"),
        Err(_) => {}
      }
    }
  }

  if want.iter().any(|s| s == "opencode") {
    let path = args.opencode_db.clone().or_else(OpenCodeSource::default_path);
    if let Some(p) = path {
      let src = OpenCodeSource::new(p);
      let result = if let Some(c) = cache.as_ref() {
        collect_opencode_with_cache(c, &src)
      } else {
        src.collect().map(|records| {
          let mut stats = CacheStats::new();
          stats.scanned = usize::from(src.db_path.exists());
          (records, stats)
        })
      };
      match result {
        Ok((mut v, stats)) => {
          if args.verbose {
            eprintln!("{}", format_cache_stats("opencode", "db files", &stats));
          }
          all.append(&mut v);
        }
        Err(e) if args.verbose => eprintln!("opencode: error: {e:#}"),
        Err(_) => {}
      }
    }
  }

  if want.iter().any(|s| s == "claude") {
    let path = args.claude_dir.clone().or_else(ClaudeSource::default_path);
    if let Some(p) = path {
      let src = ClaudeSource::new(p);
      let result = if let Some(c) = cache.as_ref() {
        collect_one_record_source_with_cache(c, "claude", src.discover_files(), ClaudeSource::parse_file)
      } else {
        src.collect().map(|records| {
          let mut stats = CacheStats::new();
          stats.scanned = src.discover_files().len();
          (records, stats)
        })
      };
      match result {
        Ok((mut v, stats)) => {
          if args.verbose {
            eprintln!("{}", format_cache_stats("claude", "files", &stats));
          }
          all.append(&mut v);
        }
        Err(e) if args.verbose => eprintln!("claude: error: {e:#}"),
        Err(_) => {}
      }
    }
  }

  if want.iter().any(|s| s == "copilot") {
    let roots = args.copilot_dir.clone().unwrap_or_else(CopilotSource::default_paths);
    if !roots.is_empty() {
      let src = CopilotSource::new(roots);
      let result = if let Some(c) = cache.as_ref() {
        collect_one_record_source_with_cache(c, "copilot", src.discover_files(), CopilotSource::parse_file)
      } else {
        src.collect().map(|records| {
          let mut stats = CacheStats::new();
          stats.scanned = src.discover_files().len();
          (records, stats)
        })
      };
      match result {
        Ok((mut v, stats)) => {
          if args.verbose {
            eprintln!(
              "{} (input/output are estimates from rendered text length)",
              format_cache_stats("copilot", "files", &stats)
            );
          }
          all.append(&mut v);
        }
        Err(e) if args.verbose => eprintln!("copilot: error: {e:#}"),
        Err(_) => {}
      }
    }
  }

  // Filters.
  let period_since = match args.period {
    Some(Period::Today) => Some(crate::time::start_of_today()),
    Some(Period::Week) => Some(crate::time::last_7d()),
    Some(Period::Month) => Some(crate::time::start_of_month()),
    None => None,
  };

  let since = args
    .since
    .as_deref()
    .map(time::parse_when)
    .transpose()
    .context("parsing --since")?
    .or(period_since);
  let until = args
    .until
    .as_deref()
    .map(time::parse_when)
    .transpose()
    .context("parsing --until")?;
  let filters = Filters {
    since,
    until,
    model_glob: args
      .model
      .as_deref()
      .map(glob::Pattern::new)
      .transpose()
      .context("parsing --model glob")?,
    provider_glob: args
      .provider
      .as_deref()
      .map(glob::Pattern::new)
      .transpose()
      .context("parsing --provider glob")?,
    cwd_glob: args
      .cwd
      .as_deref()
      .map(glob::Pattern::new)
      .transpose()
      .context("parsing --cwd glob")?,
  };

  // Pricing.
  let mut pricing = PricingTable::load_bundled();
  if let Some(p) = &args.pricing {
    pricing.merge_file(p)?;
  }

  // Group dims.
  let dims: Vec<GroupDim> = args.group_by.iter().filter_map(|s| GroupDim::parse(s)).collect();
  let dims = if dims.is_empty() {
    vec![GroupDim::Source, GroupDim::Model]
  } else {
    dims
  };

  let mut aggs = aggregate(&all, &dims, args.date_bucket.as_str(), &filters, &pricing);

  let sort_key = SortKey::parse(&args.sort).unwrap_or(SortKey::Total);
  sort_aggs(&mut aggs, sort_key, !args.asc);

  if let Some(n) = args.limit {
    aggs.truncate(n);
  }

  let show_cost = !args.no_cost;

  match args.format {
    Format::Table => {
      if aggs.is_empty() {
        println!("(no records found)");
      } else {
        println!(
          "{}",
          render_table(&aggs, &dims, &crate::format::table::TableOpts { show_cost, use_color },)
        );
      }
    }
    Format::Json => {
      println!("{}", render_json(&aggs, &dims));
    }
  }

  Ok(())
}

fn collect_one_record_source_with_cache<F>(
  cache: &CacheDb,
  source: &str,
  files: Vec<PathBuf>,
  parse_file: F,
) -> Result<(Vec<UsageRecord>, CacheStats)>
where
  F: Fn(&Path) -> Result<Option<UsageRecord>>,
{
  let mut out = Vec::new();
  let mut stats = CacheStats::new();
  stats.scanned = files.len();

  let known = cache.file_mtimes_for(source)?;
  let mut seen: HashSet<PathBuf> = HashSet::new();

  for file in files {
    seen.insert(file.clone());
    let mtime = file_mtime_secs(&file).unwrap_or(0);
    let was_known = known.get(&file).copied();

    if was_known == Some(mtime) {
      let mut cached = cache.load_active_for_file(source, &file)?;
      if cached.is_empty() {
        let mut parsed = Vec::new();
        if let Some(rec) = parse_file(&file)? {
          parsed.push(rec);
        }
        if let Some(prev) = was_known {
          if prev == mtime {
            stats.updated += 1;
          }
        }
        cache.upsert_file(&file, mtime, source, &parsed)?;
        out.extend(parsed);
      } else {
        stats.cached += 1;
        out.append(&mut cached);
      }
      continue;
    }

    let mut parsed = Vec::new();
    if let Some(rec) = parse_file(&file)? {
      parsed.push(rec);
    }
    if was_known.is_some() {
      stats.updated += 1;
    } else {
      stats.added += 1;
    }
    cache.upsert_file(&file, mtime, source, &parsed)?;
    out.extend(parsed);
  }

  let to_prune: Vec<PathBuf> = cache
    .active_file_paths(source)?
    .into_iter()
    .filter(|p| !seen.contains(p))
    .collect();
  if !to_prune.is_empty() {
    let _ = cache.prune_files(source, &to_prune)?;
    stats.pruned = to_prune.len();
  }

  Ok((out, stats))
}

fn collect_opencode_with_cache(cache: &CacheDb, src: &OpenCodeSource) -> Result<(Vec<UsageRecord>, CacheStats)> {
  let mut stats = CacheStats::new();
  let mut out = Vec::new();
  let file = src.db_path.clone();

  if !file.exists() {
    let to_prune = cache.active_file_paths("opencode")?;
    if !to_prune.is_empty() {
      let _ = cache.prune_files("opencode", &to_prune)?;
      stats.pruned = to_prune.len();
    }
    return Ok((out, stats));
  }

  stats.scanned = 1;
  let mtime = file_mtime_secs(&file).unwrap_or(0);
  let known = cache.file_mtimes_for("opencode")?;
  let was_known = known.get(&file).copied();

  if was_known == Some(mtime) {
    out = cache.load_active_for_file("opencode", &file)?;
    if !out.is_empty() {
      stats.cached = 1;
      return Ok((out, stats));
    }
  }

  out = src.collect()?;
  if was_known.is_some() {
    stats.updated = 1;
  } else {
    stats.added = 1;
  }
  cache.upsert_file(&file, mtime, "opencode", &out)?;
  Ok((out, stats))
}

fn file_mtime_secs(path: &Path) -> Option<i64> {
  let meta = std::fs::metadata(path).ok()?;
  let modified = meta.modified().ok()?;
  let dur = modified.duration_since(UNIX_EPOCH).ok()?;
  Some(dur.as_secs() as i64)
}

fn format_cache_stats(source: &str, unit: &str, stats: &CacheStats) -> String {
  if stats.scanned == 0 {
    return format!("{source}: 0 {unit}");
  }
  if stats.cached == 0 && stats.added == 0 && stats.updated == 0 && stats.pruned == 0 {
    return format!("{source}: {} {unit}", stats.scanned);
  }
  if stats.pruned > 0 {
    format!(
      "{source}: {} {unit}, {} cached, {} added, {} updated, {} pruned",
      stats.scanned, stats.cached, stats.added, stats.updated, stats.pruned
    )
  } else {
    format!(
      "{source}: {} {unit}, {} cached, {} added, {} updated",
      stats.scanned, stats.cached, stats.added, stats.updated
    )
  }
}
