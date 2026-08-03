mod activity;
mod aggregate;
mod cache;
mod cli;
mod config;
mod format;
mod model;
mod model_data;
mod model_name;
mod pricing;
mod sources;
mod text_count;
mod time;
mod tips;

use anyhow::{Context, Result};
use chrono::Utc;
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use std::collections::HashSet;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;
use tracing::debug;
use tracing_subscriber::EnvFilter;

use crate::activity::{render_activity, ActivityRenderOptions};
use crate::aggregate::{aggregate, sort_aggs, Filters, GroupDim, SortKey};
use crate::cache::{CacheDb, CacheStats, FileStamp};
use crate::cli::{Args, CacheCmd, Cmd, ConfigCmd, Format, GraphChart, Unit};
use crate::format::{
  json::render_json,
  svg::render_svg_terminal,
  table::{render_table, TableOpts},
};
use crate::model::{ParsedUsageFile, UsageRecord};
use crate::pricing::{update_cached_prices, PricingTable};
use crate::sources::{
  claude::ClaudeSource, codex::CodexSource, copilot::CopilotSource, copilot_cli::CopilotCliSource,
  opencode::OpenCodeSource, pi_agent::PiAgentSource, UsageSource,
};
use crate::tips::tip_for_hour;

#[derive(Clone, Copy)]
struct GraphOpts {
  chart: GraphChart,
  width: Option<usize>,
}

fn main() -> Result<()> {
  let args = config::parse_args()?;
  init_tracing(args.verbose);

  let graph_opts = match args.cmd.as_ref() {
    Some(Cmd::Graph { chart, width, .. }) => Some(GraphOpts {
      chart: *chart,
      width: *width,
    }),
    Some(cmd) => return run_subcommand(cmd, &args),
    None => None,
  };
  if graph_opts.is_some() && args.format == Format::Json {
    anyhow::bail!("graph: --format json is not supported; use table or svg");
  }
  let filters = build_filters(&args)?;

  let use_color = !args.no_color && std::env::var_os("NO_COLOR").is_none();
  let mut cache = if args.no_cache {
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
      let progress = ProcessingProgress::new(args.format, args.verbose);
      let result = if let Some(c) = cache.as_mut() {
        collect_one_record_source_with_cache(
          c,
          "codex",
          src.discover_files(),
          CodexSource::parse_cache_file,
          progress,
        )
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
      let progress = ProcessingProgress::new(args.format, args.verbose);
      let result = if let Some(c) = cache.as_mut() {
        collect_one_record_source_with_cache(
          c,
          "copilot-cli",
          src.discover_files(),
          CopilotCliSource::parse_cache_file,
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
      let mut progress = ProcessingProgress::new(args.format, args.verbose);
      let result = if let Some(c) = cache.as_mut() {
        collect_opencode_with_cache(c, &src, progress)
      } else {
        if src.db_path.exists() {
          progress.show("opencode", &src.db_path);
        }
        let collected = src.collect();
        collected.map(|records| {
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
      let progress = ProcessingProgress::new(args.format, args.verbose);
      let result = if let Some(c) = cache.as_mut() {
        collect_one_record_source_with_cache(
          c,
          "pi-agent",
          src.discover_files(),
          PiAgentSource::parse_cache_file,
          progress,
        )
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
      let progress = ProcessingProgress::new(args.format, args.verbose);
      let result = if let Some(c) = cache.as_mut() {
        collect_one_record_source_with_cache(
          c,
          "claude",
          src.discover_files(),
          ClaudeSource::parse_cache_file,
          progress,
        )
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
      let progress = ProcessingProgress::new(args.format, args.verbose);
      let result = if let Some(c) = cache.as_mut() {
        collect_one_record_source_with_cache(
          c,
          "copilot",
          src.discover_files(),
          CopilotSource::parse_cache_file,
          progress,
        )
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

  // Pricing.
  let pricing = if let Some(p) = &args.pricing {
    PricingTable::load_file(p)?
  } else {
    PricingTable::load_default()?
  };

  if let Some(opts) = graph_opts {
    return render_activity_graph(&all, &filters, &pricing, &args, opts);
  }
  let unit = output_unit(&args);

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

  let sort_key = SortKey::parse(&args.sort).unwrap_or(SortKey::Total);
  sort_aggs(&mut aggs, sort_key, !args.asc, unit);

  if let Some(n) = args.limit {
    aggs.truncate(n);
  }

  let show_cost = !args.no_cost;

  match args.format {
    Format::Table => {
      let rendered = if aggs.is_empty() {
        "(no records found)\n".to_string()
      } else {
        let opts = table_opts(&args, show_cost, use_color, unit, table_fit_width(&args));
        format!("{}\n", render_table(&aggs, &dims, &opts))
      };
      print!(
        "{}",
        append_hourly_tip_if_interactive(
          rendered,
          args.format,
          std::io::stdout().is_terminal(),
          tip_for_hour(&args, Utc::now()),
        )
      );
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
      print!("{}", render_svg_terminal(&display_command(), &text, args.svg_theme));
    }
  }

  Ok(())
}

fn render_activity_graph(
  records: &[UsageRecord],
  filters: &Filters,
  pricing: &PricingTable,
  args: &Args,
  opts: GraphOpts,
) -> Result<()> {
  let command = display_command();
  let unit = output_unit(args);
  let use_color = !args.no_color && std::env::var_os("NO_COLOR").is_none();
  let width = opts.width.or_else(|| {
    if std::io::stdout().is_terminal() {
      terminal_width().or_else(columns_env_width)
    } else {
      None
    }
  });
  let rendered = render_activity(
    records,
    filters,
    pricing,
    ActivityRenderOptions {
      chart: opts.chart,
      format: args.format,
      unit,
      cost_mode: args.cost,
      use_color,
      width,
      command: &command,
      svg_theme: args.svg_theme,
    },
  )?;
  print!(
    "{}",
    append_hourly_tip_if_interactive(
      rendered,
      args.format,
      std::io::stdout().is_terminal(),
      tip_for_hour(args, Utc::now()),
    )
  );
  Ok(())
}

fn append_hourly_tip_if_interactive(
  rendered: String,
  format: Format,
  stdout_is_terminal: bool,
  tip: Option<&str>,
) -> String {
  if format != Format::Table || !stdout_is_terminal || tip.is_none() {
    return rendered;
  }

  let rendered = rendered.trim_end_matches('\n');
  format!("{rendered}\n\nTip: {}\n", tip.expect("tip checked above"))
}

fn display_command() -> String {
  display_command_from(std::env::args().collect())
}

fn display_command_from(mut args: Vec<String>) -> String {
  if let Some(bin) = args.first_mut() {
    *bin = Path::new(bin)
      .file_name()
      .and_then(|name| name.to_str())
      .unwrap_or("llm-tokei")
      .to_string();
  }
  let mut visible = Vec::with_capacity(args.len());
  let mut args = args.into_iter().peekable();
  while let Some(arg) = args.next() {
    let omit_next = (arg == "--format" && args.peek().is_some_and(|format| format == "svg")) || arg == "--svg-theme";
    if omit_next {
      args.next();
    } else if arg != "--format=svg" && !arg.starts_with("--svg-theme=") {
      visible.push(arg);
    }
  }
  visible.iter().map(|arg| shell_quote(arg)).collect::<Vec<_>>().join(" ")
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

#[cfg(test)]
mod output_tests {
  use super::*;

  #[test]
  fn svg_rendering_options_are_omitted_from_the_decorated_command() {
    let command = display_command_from(
      [
        "/tmp/llm-tokei",
        "graph",
        "--24h",
        "--format",
        "svg",
        "--svg-theme",
        "light",
      ]
      .map(str::to_string)
      .to_vec(),
    );
    let equals_command = display_command_from(
      ["/tmp/llm-tokei", "graph", "--format=svg", "--svg-theme=dark"]
        .map(str::to_string)
        .to_vec(),
    );

    assert_eq!(command, "llm-tokei graph --24h");
    assert_eq!(equals_command, "llm-tokei graph");
  }

  #[test]
  fn tips_only_render_for_interactive_table_output() {
    let table = "source  total\ncodex   42\n".to_string();
    let tip = "Use `--7d` to focus on the last week.";
    let expected = format!("source  total\ncodex   42\n\nTip: {tip}\n");

    assert_eq!(
      append_hourly_tip_if_interactive(table.clone(), Format::Table, true, Some(tip)),
      expected
    );
    assert_eq!(
      append_hourly_tip_if_interactive(table.clone(), Format::Table, false, Some(tip)),
      table
    );
    assert_eq!(
      append_hourly_tip_if_interactive(table.clone(), Format::Json, true, Some(tip)),
      table
    );
    assert_eq!(
      append_hourly_tip_if_interactive(table.clone(), Format::Svg, true, Some(tip)),
      table
    );
    assert_eq!(
      append_hourly_tip_if_interactive(table.clone(), Format::Table, true, None),
      table
    );
  }

  #[test]
  fn tips_keep_empty_terminal_output_readable() {
    let tip = "Use `--7d` to focus on the last week.";
    let rendered = append_hourly_tip_if_interactive("(no records found)\n".to_string(), Format::Table, true, Some(tip));

    assert_eq!(rendered, format!("(no records found)\n\nTip: {tip}\n"));
  }
}

fn terminal_width() -> Option<usize> {
  terminal_size::terminal_size().map(|(terminal_size::Width(width), _)| width as usize)
}

struct ProcessingProgress {
  bar: ProgressBar,
  enabled: bool,
  gate: Option<DelayedProgressGate>,
}

const PROGRESS_DELAY: Duration = Duration::from_secs(1);
const PROGRESS_TICK_INTERVAL: Duration = Duration::from_millis(100);

impl ProcessingProgress {
  fn new(format: Format, verbose: bool) -> Self {
    Self::with_terminal(format, std::io::stderr().is_terminal(), verbose)
  }

  fn with_terminal(format: Format, is_terminal: bool, verbose: bool) -> Self {
    let enabled = format != Format::Json && is_terminal && !verbose;
    let bar = ProgressBar::new_spinner();
    bar.set_draw_target(ProgressDrawTarget::hidden());
    bar.set_style(
      ProgressStyle::with_template("{spinner} processing {msg}").expect("processing progress template is valid"),
    );
    Self {
      bar,
      enabled,
      gate: None,
    }
  }

  fn show(&mut self, source: &str, file: &Path) {
    if self.enabled {
      self.bar.set_message(format!("{source}: {}", file.display()));
      if self.gate.is_none() {
        let bar = self.bar.clone();
        self.gate = Some(DelayedProgressGate::start(PROGRESS_DELAY, move || {
          bar.set_draw_target(ProgressDrawTarget::stderr());
          bar.enable_steady_tick(PROGRESS_TICK_INTERVAL);
        }));
      }
    }
  }
}

impl Drop for ProcessingProgress {
  fn drop(&mut self) {
    if let Some(mut gate) = self.gate.take() {
      gate.cancel();
    }
    self.bar.finish_and_clear();
  }
}

struct DelayedProgressGate {
  cancelled: Arc<(Mutex<bool>, Condvar)>,
  handle: Option<JoinHandle<()>>,
}

impl DelayedProgressGate {
  fn start(delay: Duration, reveal: impl FnOnce() + Send + 'static) -> Self {
    let cancelled = Arc::new((Mutex::new(false), Condvar::new()));
    let worker_cancelled = Arc::clone(&cancelled);
    let handle = std::thread::spawn(move || {
      let (lock, wake) = &*worker_cancelled;
      let cancelled = lock.lock().expect("progress gate lock is not poisoned");
      let (cancelled, timeout) = wake
        .wait_timeout_while(cancelled, delay, |cancelled| !*cancelled)
        .expect("progress gate lock is not poisoned");
      if !*cancelled && timeout.timed_out() {
        drop(cancelled);
        reveal();
      }
    });
    Self {
      cancelled,
      handle: Some(handle),
    }
  }

  fn cancel(&mut self) {
    let (lock, wake) = &*self.cancelled;
    *lock.lock().expect("progress gate lock is not poisoned") = true;
    wake.notify_all();
    if let Some(handle) = self.handle.take() {
      let _ = handle.join();
    }
  }
}

impl Drop for DelayedProgressGate {
  fn drop(&mut self) {
    self.cancel();
  }
}

#[cfg(test)]
mod processing_progress_tests {
  use super::*;

  #[test]
  fn progress_is_only_enabled_for_interactive_non_json_output() {
    assert!(ProcessingProgress::with_terminal(Format::Table, true, false).enabled);
    assert!(ProcessingProgress::with_terminal(Format::Svg, true, false).enabled);
    assert!(!ProcessingProgress::with_terminal(Format::Json, true, false).enabled);
    assert!(!ProcessingProgress::with_terminal(Format::Table, false, false).enabled);
    assert!(!ProcessingProgress::with_terminal(Format::Table, true, true).enabled);
  }

  #[test]
  fn delayed_progress_gate_can_cancel_before_reveal() {
    let revealed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let worker_revealed = Arc::clone(&revealed);
    let mut gate = DelayedProgressGate::start(Duration::from_secs(1), move || {
      worker_revealed.store(true, std::sync::atomic::Ordering::SeqCst);
    });

    gate.cancel();

    assert!(!revealed.load(std::sync::atomic::Ordering::SeqCst));
  }

  #[test]
  fn unique_files_keeps_each_discovered_path_once() {
    assert_eq!(
      unique_files(vec![
        PathBuf::from("/cache/one.jsonl"),
        PathBuf::from("/cache/two.jsonl"),
        PathBuf::from("/cache/one.jsonl"),
      ]),
      vec![PathBuf::from("/cache/one.jsonl"), PathBuf::from("/cache/two.jsonl")]
    );
  }
}

fn collect_one_record_source_with_cache<F>(
  cache: &mut CacheDb,
  source: &str,
  files: Vec<PathBuf>,
  parse_file: F,
  mut progress: ProcessingProgress,
) -> Result<(Vec<UsageRecord>, CacheStats)>
where
  F: Fn(&Path) -> Result<ParsedUsageFile>,
{
  let mut out = Vec::new();
  let mut stats = CacheStats::new();
  let files = unique_files(files);
  stats.scanned = files.len();

  let known = cache.file_stamps_for(source)?;
  for file in files {
    let stamp_before = FileStamp::from_path(&file);
    let was_known = known.get(&file).copied();

    if stamp_before.is_some_and(|stamp| was_known == Some(stamp)) {
      let mut cached = cache.load_active_for_file(source, &file)?;
      stats.cached += 1;
      debug!(source, file = %file.display(), summary = %file_summary(&cached), "file summary");
      out.append(&mut cached);
      continue;
    }

    debug!(source, file = %file.display(), "processing file");
    progress.show(source, &file);
    let parsed = parse_file(&file)?;
    debug!(source, file = %file.display(), summary = %parsed_file_summary(&parsed), "file summary");

    let stamp_after = FileStamp::from_path(&file);
    match (parsed.complete, stamp_before, stamp_after) {
      (true, Some(stamp_before), Some(stamp_after)) if stamp_before == stamp_after => {
        match cache.upsert_file(&file, stamp_before, source, &parsed) {
          Ok(()) => {
            if was_known.is_some() {
              stats.updated += 1;
            } else {
              stats.added += 1;
            }
          }
          Err(error) => {
            debug!(source, file = %file.display(), error = %error, "cache reconciliation failed; reporting parsed file");
          }
        }
      }
      _ => {
        debug!(
          source,
          file = %file.display(),
          complete = parsed.complete,
          "not caching an incomplete or concurrently modified file"
        );
      }
    }
    out.extend(parsed.into_usage_records());
  }

  Ok((out, stats))
}

fn collect_one_record_source_direct<F>(
  source: &str,
  files: Vec<PathBuf>,
  parse_file: F,
  mut progress: ProcessingProgress,
) -> Result<(Vec<UsageRecord>, CacheStats)>
where
  F: Fn(&Path) -> Result<Option<Vec<UsageRecord>>>,
{
  let mut out = Vec::new();
  let mut stats = CacheStats::new();
  let files = unique_files(files);
  stats.scanned = files.len();

  for file in files {
    debug!(source, file = %file.display(), "processing file");
    progress.show(source, &file);
    let parsed = parse_file(&file);
    let Ok(Some(records)) = parsed else {
      continue;
    };
    debug!(source, file = %file.display(), summary = %file_summary(&records), "file summary");
    out.extend(records);
  }

  Ok((out, stats))
}

fn unique_files(files: Vec<PathBuf>) -> Vec<PathBuf> {
  let mut seen = HashSet::new();
  files.into_iter().filter(|file| seen.insert(file.clone())).collect()
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

fn build_filters(args: &Args) -> Result<Filters> {
  let period_since = period_since(args).transpose().context("parsing --period")?;
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
    .map(time::parse_until)
    .transpose()
    .context("parsing --until")?;
  Ok(Filters {
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
  })
}

fn collect_opencode_with_cache(
  cache: &mut CacheDb,
  src: &OpenCodeSource,
  mut progress: ProcessingProgress,
) -> Result<(Vec<UsageRecord>, CacheStats)> {
  let mut stats = CacheStats::new();
  let mut out = Vec::new();
  let file = src.db_path.clone();

  if !file.exists() {
    return Ok((out, stats));
  }

  stats.scanned = 1;
  let stamp_before = FileStamp::from_sqlite_database(&file);
  let known = cache.file_stamps_for("opencode")?;
  let was_known = known.get(&file).copied();

  if stamp_before.is_some_and(|stamp| was_known == Some(stamp)) {
    out = cache.load_active_for_file("opencode", &file)?;
    stats.cached = 1;
    return Ok((out, stats));
  }

  debug!(source = "opencode", file = %file.display(), "processing file");
  progress.show("opencode", &file);
  let parsed = OpenCodeSource::parse_cache_file(&file)?;
  debug!(source = "opencode", file = %file.display(), summary = %parsed_file_summary(&parsed), "file summary");

  let stamp_after = FileStamp::from_sqlite_database(&file);
  match (parsed.complete, stamp_before, stamp_after) {
    (true, Some(stamp_before), Some(stamp_after)) if stamp_before == stamp_after => {
      match cache.upsert_file(&file, stamp_before, "opencode", &parsed) {
        Ok(()) => {
          if was_known.is_some() {
            stats.updated = 1;
          } else {
            stats.added = 1;
          }
        }
        Err(error) => {
          debug!(source = "opencode", file = %file.display(), error = %error, "cache reconciliation failed; reporting parsed file");
        }
      }
    }
    _ => {
      debug!(
        source = "opencode",
        file = %file.display(),
        complete = parsed.complete,
        "not caching an incomplete or concurrently modified file"
      );
    }
  }
  out = parsed.into_usage_records();
  Ok((out, stats))
}

fn format_cache_stats(source: &str, unit: &str, stats: &CacheStats) -> String {
  if stats.scanned == 0 {
    return format!("{source}: 0 {unit}");
  }
  if stats.cached == 0 && stats.added == 0 && stats.updated == 0 {
    return format!("{source}: {} {unit}", stats.scanned);
  }
  format!(
    "{source}: {} {unit}, {} cached, {} added, {} updated",
    stats.scanned, stats.cached, stats.added, stats.updated
  )
}

fn file_summary(records: &[UsageRecord]) -> String {
  file_summary_iter(records.iter())
}

fn parsed_file_summary(parsed: &ParsedUsageFile) -> String {
  file_summary_iter(parsed.records.iter().map(|parsed| &parsed.record))
}

fn file_summary_iter<'a>(records: impl Iterator<Item = &'a UsageRecord>) -> String {
  let mut input = 0;
  let mut output = 0;
  let mut reasoning = 0;
  let mut cache_read = 0;
  let mut cache_write = 0;
  let mut calls = 0;
  let mut rounds = 0;
  let mut input_est = false;
  let mut output_est = false;
  let mut count = 0;
  for record in records {
    count += 1;
    input += record.display_input();
    output += record.display_output();
    reasoning += record.reasoning;
    cache_read += record.cache_read;
    cache_write += record.cache_write;
    calls += record.calls;
    rounds += record.rounds;
    input_est |= record.input_estimated;
    output_est |= record.output_estimated;
  }
  format!(
    "records={}, input={}, output={}, reasoning={}, cache_r={}, cache_w={}, calls={}, rounds={}",
    count,
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
    Cmd::Graph { .. } => unreachable!("graph is rendered after collecting usage records"),
    Cmd::Cache { cmd } => run_cache(cmd),
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

fn run_cache(cmd: &CacheCmd) -> Result<()> {
  match cmd {
    CacheCmd::Prune { .. } => {
      let mut cache = CacheDb::open()?;
      let stats = cache.prune()?;
      eprintln!("pruned cache: {} sessions, {} records", stats.sessions, stats.records);
    }
  }
  Ok(())
}

fn run_update() -> Result<()> {
  let path = update_cached_prices()?;
  eprintln!("updated model data cache: {}", path.display());
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
