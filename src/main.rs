mod activity;
mod aggregate;
mod cache;
mod cli;
mod config;
mod format;
mod model;
mod model_name;
mod pricing;
mod sources;
mod text_count;
mod time;

use anyhow::{Context, Result};
use std::collections::HashSet;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use tracing::debug;
use tracing_subscriber::EnvFilter;

use crate::aggregate::{aggregate, sort_aggs, Filters, GroupDim, SortKey};
use crate::cache::{CacheDb, CacheStats};
use crate::cli::{Args, Cmd, ConfigCmd, Format, Unit};
use crate::format::{
  json::render_json,
  svg::render_svg_terminal,
  table::{render_table, TableOpts},
};
use crate::model::UsageRecord;
use crate::pricing::{update_cached_prices, PricingTable};
use crate::sources::{
  claude::ClaudeSource, codex::CodexSource, copilot::CopilotSource, copilot_cli::CopilotCliSource,
  opencode::OpenCodeSource, pi_agent::PiAgentSource, UsageSource,
};

fn main() -> Result<()> {
  let args = config::parse_args()?;
  init_tracing(args.verbose);

  if let Some(cmd) = args.cmd.as_ref() {
    return run_subcommand(cmd, &args);
  }

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
    .unwrap_or_else(|| {
      vec![
        "codex".into(),
        "opencode".into(),
        "claude".into(),
        "copilot".into(),
        "copilot-cli".into(),
        "pi-agent".into(),
      ]
    });

  let mut all: Vec<UsageRecord> = Vec::new();

  if want.iter().any(|s| s == "codex") {
    let path = args.codex_dir.clone().or_else(CodexSource::default_path);
    if let Some(p) = path {
      let src = CodexSource::new(p);
      let progress = ProcessingProgress::new(args.format);
      let result = if let Some(c) = cache.as_ref() {
        collect_one_record_source_with_cache(c, "codex", src.discover_files(), CodexSource::parse_file, progress)
      } else {
        collect_one_record_source_direct("codex", src.discover_files(), CodexSource::parse_file, progress)
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

  if want.iter().any(|s| s == "copilot-cli") {
    let roots = args
      .copilot_cli_dir
      .clone()
      .unwrap_or_else(CopilotCliSource::default_paths);
    if !roots.is_empty() {
      let src = CopilotCliSource::new(roots);
      let progress = ProcessingProgress::new(args.format);
      let result = if let Some(c) = cache.as_ref() {
        collect_one_record_source_with_cache(
          c,
          "copilot-cli",
          src.discover_files(),
          CopilotCliSource::parse_file,
          progress,
        )
      } else {
        collect_one_record_source_direct(
          "copilot-cli",
          src.discover_files(),
          CopilotCliSource::parse_file,
          progress,
        )
      };
      match result {
        Ok((mut v, stats)) => {
          if args.verbose {
            eprintln!(
              "{} (uses exact shutdown metrics when present; otherwise input is estimated)",
              format_cache_stats("copilot-cli", "files", &stats)
            );
          }
          all.append(&mut v);
        }
        Err(e) if args.verbose => eprintln!("copilot-cli: error: {e:#}"),
        Err(_) => {}
      }
    }
  }

  if want.iter().any(|s| s == "opencode") {
    let path = args.opencode_db.clone().or_else(OpenCodeSource::default_path);
    if let Some(p) = path {
      let src = OpenCodeSource::new(p);
      let progress = ProcessingProgress::new(args.format);
      let result = if let Some(c) = cache.as_ref() {
        collect_opencode_with_cache(c, &src, progress)
      } else {
        if src.db_path.exists() {
          progress.show("opencode", &src.db_path);
        }
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

  if want.iter().any(|s| s == "pi-agent") {
    let path = args.pi_agent_dir.clone().or_else(PiAgentSource::default_path);
    if let Some(p) = path {
      let src = PiAgentSource::new(p);
      let progress = ProcessingProgress::new(args.format);
      let result = if let Some(c) = cache.as_ref() {
        collect_one_record_source_with_cache(c, "pi-agent", src.discover_files(), PiAgentSource::parse_file, progress)
      } else {
        collect_one_record_source_direct("pi-agent", src.discover_files(), PiAgentSource::parse_file, progress)
      };
      match result {
        Ok((mut v, stats)) => {
          if args.verbose {
            eprintln!("{}", format_cache_stats("pi-agent", "files", &stats));
          }
          all.append(&mut v);
        }
        Err(e) if args.verbose => eprintln!("pi-agent: error: {e:#}"),
        Err(_) => {}
      }
    }
  }

  if want.iter().any(|s| s == "claude") {
    let path = args.claude_dir.clone().or_else(ClaudeSource::default_path);
    if let Some(p) = path {
      let src = ClaudeSource::new(p);
      let progress = ProcessingProgress::new(args.format);
      let result = if let Some(c) = cache.as_ref() {
        collect_one_record_source_with_cache(c, "claude", src.discover_files(), ClaudeSource::parse_file, progress)
      } else {
        collect_one_record_source_direct("claude", src.discover_files(), ClaudeSource::parse_file, progress)
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
      let progress = ProcessingProgress::new(args.format);
      let result = if let Some(c) = cache.as_ref() {
        collect_one_record_source_with_cache(c, "copilot", src.discover_files(), CopilotSource::parse_file, progress)
      } else {
        collect_one_record_source_direct("copilot", src.discover_files(), CopilotSource::parse_file, progress)
      };
      match result {
        Ok((mut v, stats)) => {
          CopilotSource::dedupe_exact_sessions(&mut v);
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
  let period_since = period_since(&args).transpose().context("parsing --period")?;

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
  let pricing = if let Some(p) = &args.pricing {
    PricingTable::load_file(p)?
  } else {
    PricingTable::load_default()?
  };

  // Group dims.
  let dims: Vec<GroupDim> = args.group_by.iter().filter_map(|s| GroupDim::parse(s)).collect();
  let dims = if dims.is_empty() {
    vec![GroupDim::Source, GroupDim::Model]
  } else {
    dims
  };

  let cost_per = args
    .cost_per
    .as_deref()
    .map(|s| GroupDim::parse(s).with_context(|| format!("parsing --cost-per dimension '{s}'")))
    .transpose()?;

  let mut aggs = aggregate(
    &all,
    &dims,
    args.date_bucket.as_str(),
    &filters,
    &pricing,
    cost_per,
    args.cost,
  );

  let unit = output_unit(&args);

  let sort_key = SortKey::parse(&args.sort).unwrap_or(SortKey::Total);
  sort_aggs(&mut aggs, sort_key, !args.asc, unit);

  if let Some(n) = args.limit {
    aggs.truncate(n);
  }

  let show_cost = !args.no_cost;

  match args.format {
    Format::Table => {
      if aggs.is_empty() {
        println!("(no records found)");
      } else {
        let opts = table_opts(&args, show_cost, use_color, unit, table_fit_width(&args));
        println!("{}", render_table(&aggs, &dims, &opts));
      }
    }
    Format::Json => {
      println!("{}", render_json(&aggs, &dims, unit));
    }
    Format::Svg => {
      let text = if aggs.is_empty() {
        "(no records found)\n".to_string()
      } else {
        let opts = table_opts(&args, show_cost, !args.no_color, unit, args.table_width);
        render_table(&aggs, &dims, &opts)
      };
      print!("{}", render_svg_terminal(&display_command(), &text));
    }
  }

  Ok(())
}

fn display_command() -> String {
  let mut args = std::env::args().collect::<Vec<_>>();
  if let Some(bin) = args.first_mut() {
    *bin = Path::new(bin)
      .file_name()
      .and_then(|name| name.to_str())
      .unwrap_or("llm-tokei")
      .to_string();
  }
  args.iter().map(|arg| shell_quote(arg)).collect::<Vec<_>>().join(" ")
}

fn shell_quote(arg: &str) -> String {
  if !arg.is_empty()
    && arg
      .chars()
      .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '/' | ':' | '=' | ','))
  {
    return arg.to_string();
  }
  let mut out = String::from("'");
  for ch in arg.chars() {
    if ch == '\'' {
      out.push_str("'\\''");
    } else {
      out.push(ch);
    }
  }
  out.push('\'');
  out
}

fn output_unit(args: &Args) -> Unit {
  if args.bytes {
    Unit::Bytes
  } else {
    args.unit.unwrap_or(Unit::Tokens)
  }
}

fn table_opts(args: &Args, show_cost: bool, use_color: bool, unit: Unit, fit_width: Option<usize>) -> TableOpts {
  TableOpts {
    show_cost,
    use_color,
    split_input: args.split_input,
    avg: args.avg,
    unit,
    human: args.human,
    fit_width,
  }
}

fn table_fit_width(args: &Args) -> Option<usize> {
  if args.no_fit {
    return None;
  }
  if let Some(width) = args.table_width {
    return Some(width);
  }
  if !std::io::stdout().is_terminal() {
    return None;
  }
  terminal_width().or_else(columns_env_width)
}

fn columns_env_width() -> Option<usize> {
  std::env::var("COLUMNS")
    .ok()
    .and_then(|v| v.parse::<usize>().ok())
    .filter(|w| *w > 0)
}

fn terminal_width() -> Option<usize> {
  terminal_size::terminal_size().map(|(terminal_size::Width(width), _)| width as usize)
}

#[derive(Clone, Copy)]
struct ProcessingProgress {
  enabled: bool,
}

impl ProcessingProgress {
  fn new(format: Format) -> Self {
    Self {
      enabled: format != Format::Json,
    }
  }

  fn show(self, source: &str, file: &Path) {
    if self.enabled {
      eprintln!("processing {source}: {}", file.display());
    }
  }
}

fn collect_one_record_source_with_cache<F>(
  cache: &CacheDb,
  source: &str,
  files: Vec<PathBuf>,
  parse_file: F,
  progress: ProcessingProgress,
) -> Result<(Vec<UsageRecord>, CacheStats)>
where
  F: Fn(&Path) -> Result<Option<Vec<UsageRecord>>>,
{
  let mut out = Vec::new();
  let mut stats = CacheStats::new();
  stats.scanned = files.len();

  let known = cache.file_mtimes_for(source)?;
  let mut seen: HashSet<PathBuf> = HashSet::new();

  for file in files {
    debug!(source, file = %file.display(), "processing file");
    progress.show(source, &file);
    seen.insert(file.clone());
    let mtime = file_mtime_secs(&file).unwrap_or(0);
    let was_known = known.get(&file).copied();

    if was_known == Some(mtime) {
      let mut cached = cache.load_active_for_file(source, &file)?;
      if cached.is_empty() {
        let parsed = parse_file(&file)?.unwrap_or_default();
        debug!(source, file = %file.display(), summary = %file_summary(&parsed), "file summary");
        if let Some(prev) = was_known {
          if prev == mtime {
            stats.updated += 1;
          }
        }
        cache.upsert_file(&file, mtime, source, &parsed)?;
        out.extend(parsed);
      } else {
        stats.cached += 1;
        debug!(source, file = %file.display(), summary = %file_summary(&cached), "file summary");
        out.append(&mut cached);
      }
      continue;
    }

    let parsed = parse_file(&file)?.unwrap_or_default();
    debug!(source, file = %file.display(), summary = %file_summary(&parsed), "file summary");
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

fn collect_one_record_source_direct<F>(
  source: &str,
  files: Vec<PathBuf>,
  parse_file: F,
  progress: ProcessingProgress,
) -> Result<(Vec<UsageRecord>, CacheStats)>
where
  F: Fn(&Path) -> Result<Option<Vec<UsageRecord>>>,
{
  let mut out = Vec::new();
  let mut stats = CacheStats::new();
  stats.scanned = files.len();

  for file in files {
    debug!(source, file = %file.display(), "processing file");
    progress.show(source, &file);
    let Ok(Some(records)) = parse_file(&file) else {
      continue;
    };
    debug!(source, file = %file.display(), summary = %file_summary(&records), "file summary");
    out.extend(records);
  }

  Ok((out, stats))
}

fn period_since(args: &Args) -> Option<anyhow::Result<chrono::DateTime<chrono::Utc>>> {
  let period = args
    .period
    .as_deref()
    .or_else(|| args.period_24h.then_some("24h"))
    .or_else(|| args.period_7d.then_some("7d"))
    .or_else(|| args.period_1m.then_some("1m"))
    .or_else(|| args.today.then_some("today"))
    .or_else(|| args.week.then_some("week"))
    .or_else(|| args.month.then_some("month"));

  period.map(time::parse_period)
}

fn collect_opencode_with_cache(
  cache: &CacheDb,
  src: &OpenCodeSource,
  progress: ProcessingProgress,
) -> Result<(Vec<UsageRecord>, CacheStats)> {
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
  debug!(source = "opencode", file = %file.display(), "processing file");
  progress.show("opencode", &file);
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
  debug!(source = "opencode", file = %file.display(), summary = %file_summary(&out), "file summary");
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

fn file_summary(records: &[UsageRecord]) -> String {
  let input: u64 = records.iter().map(UsageRecord::display_input).sum();
  let output: u64 = records.iter().map(UsageRecord::display_output).sum();
  let reasoning: u64 = records.iter().map(|r| r.reasoning).sum();
  let cache_read: u64 = records.iter().map(|r| r.cache_read).sum();
  let cache_write: u64 = records.iter().map(|r| r.cache_write).sum();
  let calls: u64 = records.iter().map(|r| r.calls).sum();
  let rounds: u64 = records.iter().map(|r| r.rounds).sum();
  let input_est = records.iter().any(|r| r.input_estimated);
  let output_est = records.iter().any(|r| r.output_estimated);
  format!(
    "records={}, input={}, output={}, reasoning={}, cache_r={}, cache_w={}, calls={}, rounds={}",
    records.len(),
    fmt_est(input, input_est),
    fmt_est(output, output_est),
    reasoning,
    cache_read,
    cache_write,
    calls,
    rounds
  )
}

fn fmt_est(v: u64, est: bool) -> String {
  if est {
    format!("~{v}")
  } else {
    v.to_string()
  }
}

fn init_tracing(verbose: bool) {
  let filter = match std::env::var("RUST_LOG") {
    Ok(value) => EnvFilter::new(value),
    Err(_) if verbose => EnvFilter::new("debug"),
    Err(_) => EnvFilter::new("warn"),
  };
  let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}

fn run_subcommand(cmd: &Cmd, args: &Args) -> Result<()> {
  match cmd {
    Cmd::Dump {
      copilot,
      copilot_cli,
      codex,
      files,
      out,
      ..
    } => run_dump(*copilot, *copilot_cli, *codex, files, out.as_deref(), args),
    Cmd::Update { .. } => run_update(),
    Cmd::Config { cmd } => run_config(cmd, args),
  }
}

fn run_update() -> Result<()> {
  let path = update_cached_prices()?;
  eprintln!("updated pricing cache: {}", path.display());
  Ok(())
}

fn run_config(cmd: &ConfigCmd, args: &Args) -> Result<()> {
  let path = args
    .config
    .clone()
    .or_else(config::default_config_path)
    .context("cannot determine config path")?;
  match cmd {
    ConfigCmd::Args { args, reset, .. } => {
      if *reset {
        config::reset_defaults(&path)?;
        eprintln!("reset config defaults: {}", path.display());
      } else if let Some(arg_string) = args {
        config::save_default_arg_string(&path, arg_string)?;
        eprintln!("saved config defaults: {}", path.display());
      } else {
        anyhow::bail!("config args: provide an argument string or --reset");
      }
    }
    ConfigCmd::List { .. } => {
      println!("# {}", path.display());
      print!("{}", config::list_config(&path)?);
    }
  }
  Ok(())
}

#[derive(Debug, Clone, Copy)]
enum DumpSource {
  Codex,
  Copilot,
  CopilotCli,
}

fn run_dump(
  copilot: bool,
  copilot_cli: bool,
  codex: bool,
  files: &[PathBuf],
  out: Option<&Path>,
  args: &Args,
) -> Result<()> {
  let selected = [copilot, copilot_cli, codex].into_iter().filter(|v| *v).count();
  let source = match selected {
    0 => anyhow::bail!("dump: select a source with `--copilot`, `--copilot-cli`, or `--codex`"),
    1 if copilot => DumpSource::Copilot,
    1 if copilot_cli => DumpSource::CopilotCli,
    1 => DumpSource::Codex,
    _ => anyhow::bail!("dump: select only one source: `--copilot`, `--copilot-cli`, or `--codex`"),
  };

  if let Some(out) = out {
    std::fs::create_dir_all(out).with_context(|| format!("creating output dir {}", out.display()))?;
  }

  let discovered;
  let paths: &[PathBuf] = if files.is_empty() {
    discovered = discover_dump_files(source, args);
    &discovered
  } else {
    files
  };

  let mut written: usize = 0;
  let mut total_records: usize = 0;
  let stdout = std::io::stdout();
  let mut stdout = std::io::BufWriter::new(stdout.lock());
  use std::io::Write;

  for path in paths {
    let dumped = match dump_session_messages(source, path) {
      Ok(Some(d)) => d,
      Ok(None) => continue,
      Err(e) => {
        if args.verbose {
          eprintln!("dump: error reading {}: {e:#}", path.display());
        }
        continue;
      }
    };
    if dumped.records.is_empty() {
      continue;
    }

    if let Some(out) = out {
      let dest = out.join(format!("{}.jsonl", sanitize_filename(&dumped.session_id)));
      let f = std::fs::File::create(&dest).with_context(|| format!("writing {}", dest.display()))?;
      let mut writer = std::io::BufWriter::new(f);
      for rec in &dumped.records {
        serde_json::to_writer(&mut writer, rec)?;
        writeln!(writer)?;
      }
      writer.flush()?;
      written += 1;
    } else {
      writeln!(stdout, "# {}", path.display())?;
      for rec in &dumped.records {
        serde_json::to_writer(&mut stdout, rec)?;
        writeln!(stdout)?;
      }
      written += 1;
    }
    total_records += dumped.records.len();
  }
  stdout.flush()?;

  if let Some(out) = out {
    if args.verbose || written == 0 {
      eprintln!(
        "dump: wrote {written} session file(s), {total_records} record(s) to {}",
        out.display()
      );
    }
  } else if args.verbose || written == 0 {
    eprintln!("dump: wrote {written} session stream(s), {total_records} record(s) to stdout");
  }
  Ok(())
}

fn discover_dump_files(source: DumpSource, args: &Args) -> Vec<PathBuf> {
  match source {
    DumpSource::Codex => args
      .codex_dir
      .clone()
      .or_else(CodexSource::default_path)
      .map(|root| CodexSource::new(root).discover_files())
      .unwrap_or_default(),
    DumpSource::Copilot => {
      let roots = args.copilot_dir.clone().unwrap_or_else(CopilotSource::default_paths);
      CopilotSource::new(roots).discover_files()
    }
    DumpSource::CopilotCli => {
      let roots = args
        .copilot_cli_dir
        .clone()
        .unwrap_or_else(CopilotCliSource::default_paths);
      CopilotCliSource::new(roots).discover_files()
    }
  }
}

fn dump_session_messages(source: DumpSource, path: &Path) -> Result<Option<crate::sources::dump::DumpedSession>> {
  match source {
    DumpSource::Codex => CodexSource::dump_session_messages(path),
    DumpSource::Copilot => CopilotSource::dump_session_messages(path),
    DumpSource::CopilotCli => CopilotCliSource::dump_session_messages(path),
  }
}

fn sanitize_filename(name: &str) -> String {
  name
    .chars()
    .map(|c| match c {
      '/' | '\\' | '\0' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
      _ => c,
    })
    .collect()
}
