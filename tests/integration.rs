use std::process::Command;

fn temp_file_path(name: &str) -> std::path::PathBuf {
  let nanos = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .expect("system time")
    .as_nanos();
  std::env::temp_dir().join(format!("llm-tokei-{name}-{nanos}.json"))
}

fn temp_cache_home(name: &str) -> std::path::PathBuf {
  let nanos = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .expect("system time")
    .as_nanos();
  std::env::temp_dir().join(format!("llm-tokei-cache-{name}-{nanos}"))
}

fn isolated_cmd(name: &str) -> (Command, std::path::PathBuf) {
  let cache_home = temp_cache_home(name);
  let mut cmd = Command::new(bin());
  cmd.env("XDG_CACHE_HOME", &cache_home);
  (cmd, cache_home)
}

fn temp_config_file(name: &str, contents: &str) -> (std::path::PathBuf, std::path::PathBuf) {
  let dir = temp_cache_home(name);
  let path = dir.join("config.toml");
  std::fs::create_dir_all(&dir).expect("create config dir");
  std::fs::write(&path, contents).expect("write config file");
  (path, dir)
}

fn bin() -> std::path::PathBuf {
  static TEST_CONFIG_HOME: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
  let config_home = TEST_CONFIG_HOME.get_or_init(|| {
    let dir = temp_cache_home("xdg-config-home");
    std::fs::create_dir_all(&dir).expect("create test config home");
    dir
  });
  std::env::set_var("XDG_CONFIG_HOME", config_home);

  let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
  p.push("target");
  p.push(if cfg!(debug_assertions) { "debug" } else { "release" });
  p.push("llm-tokei");
  p
}

#[test]
fn redirected_table_output_does_not_persist_processing_entries() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex/sessions");
  let (mut cmd, cache_home) = isolated_cmd("processing-table");
  let out = cmd
    .args([
      "--source",
      "codex",
      "--codex-dir",
      fixtures.to_str().unwrap(),
      "--no-cache",
    ])
    .output()
    .expect("run llm-tokei table");
  let _ = std::fs::remove_dir_all(cache_home);
  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert!(!stderr.contains("processing codex:"), "stderr: {stderr}");
}

#[test]
fn json_output_hides_current_processing_entry() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex/sessions");
  let (mut cmd, cache_home) = isolated_cmd("processing-json");
  let out = cmd
    .args([
      "--source",
      "codex",
      "--codex-dir",
      fixtures.to_str().unwrap(),
      "--format",
      "json",
      "--no-cache",
    ])
    .output()
    .expect("run llm-tokei json");
  let _ = std::fs::remove_dir_all(cache_home);
  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert!(!stderr.contains("processing codex:"), "stderr: {stderr}");
  let stdout = String::from_utf8_lossy(&out.stdout);
  serde_json::from_str::<serde_json::Value>(&stdout).expect("valid json");
}

#[test]
fn cached_files_are_skipped_without_processing_output() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/copilot/workspaceStorage");
  let cache_home = temp_cache_home("processing-cached");
  let args = [
    "--source",
    "copilot",
    "--copilot-dir",
    fixtures.to_str().unwrap(),
    "--no-color",
  ];

  let first = Command::new(bin())
    .env("XDG_CACHE_HOME", &cache_home)
    .args(args)
    .output()
    .expect("populate processing cache");
  assert!(
    first.status.success(),
    "stderr: {}",
    String::from_utf8_lossy(&first.stderr)
  );

  let second = Command::new(bin())
    .env("XDG_CACHE_HOME", &cache_home)
    .args(args)
    .arg("--verbose")
    .output()
    .expect("read processing cache");
  let _ = std::fs::remove_dir_all(cache_home);

  assert!(
    second.status.success(),
    "stderr: {}",
    String::from_utf8_lossy(&second.stderr)
  );
  let stderr = String::from_utf8_lossy(&second.stderr);
  assert!(stderr.contains("2 cached, 0 added, 0 updated"), "stderr: {stderr}");
  assert!(!stderr.contains("processing copilot:"), "stderr: {stderr}");
}

#[test]
fn codex_fixture_parses_last_total() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex/sessions");
  let (mut cmd, cache_home) = isolated_cmd("codex-total");
  let out = cmd
    .args([
      "--source",
      "codex",
      "--codex-dir",
      fixtures.to_str().unwrap(),
      "--opencode-db",
      "/nonexistent/opencode.db",
      "--format",
      "json",
      "--no-cache",
    ])
    .output()
    .expect("run llm-tokei");
  let _ = std::fs::remove_dir_all(cache_home);
  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let s = String::from_utf8_lossy(&out.stdout);
  let v: serde_json::Value = serde_json::from_str(&s).expect("valid json");
  let arr = v.as_array().unwrap();
  assert_eq!(arr.len(), 1);
  let row = &arr[0];
  // `input` display includes uncached input + cache_read + cache_write.
  assert_eq!(row["input"], 500);
  assert_eq!(row["output"], 220);
  assert_eq!(row["reasoning"], 50);
  assert_eq!(row["cache_read"], 200);
  // total = input + output, where output already includes reasoning.
  assert_eq!(row["total"], 720);
  assert_eq!(row["calls"], 4);
  assert_eq!(row["rounds"], 2);
  assert_eq!(row["sessions"], 1);
  assert_eq!(row["keys"]["model"], "gpt-5");
  assert_eq!(row["keys"]["source"], "codex");
  // gpt-5 base price: input 1.25 + output 10 + cache_read 0.125 (per 1M).
  // Billing uses prompt = 300, completion = 170, reasoning = 50, cache_read = 200.
  // 300*1.25 + 170*10 + 50*10 + 200*0.125 = 375 + 1700 + 500 + 25 = 2600 → / 1e6 = 0.002600
  let cost = row["cost"].as_f64().unwrap();
  assert!((cost - 0.002600).abs() < 1e-9, "got {cost}");
}

#[test]
fn codex_fixture_renders_svg() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex/sessions");
  let (mut cmd, cache_home) = isolated_cmd("codex-svg");
  let out = cmd
    .args([
      "--source",
      "codex",
      "--codex-dir",
      fixtures.to_str().unwrap(),
      "--opencode-db",
      "/nonexistent/opencode.db",
      "--format",
      "svg",
      "--no-cache",
    ])
    .output()
    .expect("run llm-tokei");
  let _ = std::fs::remove_dir_all(cache_home);
  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let svg = String::from_utf8_lossy(&out.stdout);
  assert!(svg.starts_with("<svg "), "svg: {svg}");
  assert!(svg.contains("llm-tokei terminal output"), "svg: {svg}");
  assert!(svg.contains("fill=\"#ff5f56\""), "svg: {svg}");
  assert!(svg.contains("fill=\"#39c5cf\""), "svg: {svg}");
  assert!(svg.contains("codex"), "svg: {svg}");
  assert!(svg.contains("gpt-5"), "svg: {svg}");
  assert!(svg.ends_with("</svg>\n"), "svg: {svg}");
}

#[test]
fn graph_renders_a_daily_terminal_plot_for_short_ranges() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex/sessions");
  let (mut cmd, cache_home) = isolated_cmd("graph-terminal-plot");
  let out = cmd
    .args([
      "graph",
      "--source",
      "codex",
      "--codex-dir",
      fixtures.to_str().unwrap(),
      "--no-cache",
      "--since",
      "2025-01-01",
      "--until",
      "2025-01-30",
      "--no-color",
      "--width",
      "80",
    ])
    .output()
    .expect("run terminal activity graph");
  let _ = std::fs::remove_dir_all(cache_home);

  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let graph = String::from_utf8_lossy(&out.stdout);
  assert!(graph.contains("Token activity · Jan 1–30, 2025"), "graph: {graph}");
  assert!(graph.contains("┼"), "graph: {graph}");
  assert!(graph.contains("Total 720 · Active 1/30 days"), "graph: {graph}");
  assert!(
    graph.contains("Best Jan 2: 720 · Longest streak 1 day"),
    "graph: {graph}"
  );
  assert!(!graph.contains("Less"), "graph: {graph}");
  assert!(!graph.contains("\x1b["), "graph: {graph}");
}

#[test]
fn graph_renders_an_hourly_terminal_plot_for_sub_30_hour_ranges() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex/sessions");
  let (mut cmd, cache_home) = isolated_cmd("graph-terminal-hourly");
  let out = cmd
    .args([
      "graph",
      "--source",
      "codex",
      "--codex-dir",
      fixtures.to_str().unwrap(),
      "--no-cache",
      "--since",
      "2025-01-02T00:00:00Z",
      "--until",
      "2025-01-02T12:00:00Z",
      "--no-color",
      "--width",
      "80",
    ])
    .output()
    .expect("run hourly terminal activity graph");
  let _ = std::fs::remove_dir_all(cache_home);

  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let graph = String::from_utf8_lossy(&out.stdout);
  assert!(graph.contains("Hourly token activity · "), "graph: {graph}");
  assert!(graph.contains("Total 720 · Active 1/13 hours"), "graph: {graph}");
  assert!(graph.contains("Longest streak 1 hour"), "graph: {graph}");
  assert!(!graph.contains("Less"), "graph: {graph}");
}

#[test]
fn graph_renders_exactly_24_hours_on_hourly_resolution() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex/sessions");
  let (mut cmd, cache_home) = isolated_cmd("graph-exactly-24h");
  let out = cmd
    .args([
      "graph",
      "--source",
      "codex",
      "--codex-dir",
      fixtures.to_str().unwrap(),
      "--no-cache",
      "--since",
      "2025-01-01T12:00:00Z",
      "--until",
      "2025-01-02T12:00:00Z",
      "--no-color",
    ])
    .output()
    .expect("run exactly 24-hour activity graph");
  let _ = std::fs::remove_dir_all(cache_home);

  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let graph = String::from_utf8_lossy(&out.stdout);
  assert!(graph.contains("Hourly token activity · "), "graph: {graph}");
  assert!(graph.contains("Total 720 · Active 1/25 hours"), "graph: {graph}");
}

#[test]
fn graph_keeps_exactly_30_hours_on_daily_resolution() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex/sessions");
  let (mut cmd, cache_home) = isolated_cmd("graph-exactly-30h");
  let out = cmd
    .args([
      "graph",
      "--source",
      "codex",
      "--codex-dir",
      fixtures.to_str().unwrap(),
      "--no-cache",
      "--since",
      "2025-01-01T12:00:00Z",
      "--until",
      "2025-01-02T18:00:00Z",
      "--no-color",
    ])
    .output()
    .expect("run exactly 30-hour activity graph");
  let _ = std::fs::remove_dir_all(cache_home);

  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let graph = String::from_utf8_lossy(&out.stdout);
  assert!(graph.contains("Token activity · "), "graph: {graph}");
  assert!(!graph.contains("Hourly token activity"), "graph: {graph}");
  assert!(graph.contains("Total 720 · Active 1/"), "graph: {graph}");
  assert!(graph.contains("Longest streak 1 day"), "graph: {graph}");
}

#[test]
fn graph_heatmap_override_forces_daily_resolution_below_30_hours() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex/sessions");
  let (mut cmd, cache_home) = isolated_cmd("graph-short-heatmap");
  let out = cmd
    .args([
      "graph",
      "--source",
      "codex",
      "--codex-dir",
      fixtures.to_str().unwrap(),
      "--no-cache",
      "--since",
      "2025-01-02T00:00:00Z",
      "--until",
      "2025-01-02T12:00:00Z",
      "--chart",
      "heatmap",
      "--no-color",
    ])
    .output()
    .expect("run forced daily heatmap for a short range");
  let _ = std::fs::remove_dir_all(cache_home);

  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let graph = String::from_utf8_lossy(&out.stdout);
  assert!(!graph.contains("Hourly token activity"), "graph: {graph}");
  assert!(graph.contains("Less ·░▒▓█ More"), "graph: {graph}");
}

#[test]
fn graph_renders_a_terminal_heatmap_for_long_ranges() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex/sessions");
  let (mut cmd, cache_home) = isolated_cmd("graph-terminal-heatmap");
  let out = cmd
    .args([
      "graph",
      "--source",
      "codex",
      "--codex-dir",
      fixtures.to_str().unwrap(),
      "--no-cache",
      "--since",
      "2025-01-01",
      "--until",
      "2025-12-31",
      "--no-color",
    ])
    .output()
    .expect("run terminal activity heatmap");
  let _ = std::fs::remove_dir_all(cache_home);

  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let graph = String::from_utf8_lossy(&out.stdout);
  assert!(graph.contains("Token activity · Jan 1–Dec 31, 2025"), "graph: {graph}");
  assert!(graph.contains("Mon"), "graph: {graph}");
  assert!(graph.contains("Less ·░▒▓█ More"), "graph: {graph}");
  assert!(graph.contains("Active 1/365 days"), "graph: {graph}");
}

#[test]
fn graph_renders_native_svg_for_short_ranges() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex/sessions");
  let (mut cmd, cache_home) = isolated_cmd("graph-svg-plot");
  let out = cmd
    .args([
      "graph",
      "--source",
      "codex",
      "--codex-dir",
      fixtures.to_str().unwrap(),
      "--no-cache",
      "--since",
      "2025-01-01",
      "--until",
      "2025-01-30",
      "--format",
      "svg",
    ])
    .output()
    .expect("run SVG activity graph");
  let _ = std::fs::remove_dir_all(cache_home);

  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let svg = String::from_utf8_lossy(&out.stdout);
  assert!(svg.starts_with("<svg "), "svg: {svg}");
  assert!(svg.contains("data-chart=\"plot\""), "svg: {svg}");
  assert!(svg.contains("class=\"activity-bar\""), "svg: {svg}");
  assert!(svg.contains("<title>Jan 2, 2025: 720</title>"), "svg: {svg}");
  assert!(!svg.contains("terminal-content"), "svg: {svg}");
}

#[test]
fn graph_renders_native_hourly_svg_for_sub_30_hour_ranges() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex/sessions");
  let (mut cmd, cache_home) = isolated_cmd("graph-svg-hourly");
  let out = cmd
    .args([
      "graph",
      "--source",
      "codex",
      "--codex-dir",
      fixtures.to_str().unwrap(),
      "--no-cache",
      "--since",
      "2025-01-02T00:00:00Z",
      "--until",
      "2025-01-02T12:00:00Z",
      "--format",
      "svg",
    ])
    .output()
    .expect("run hourly SVG activity graph");
  let _ = std::fs::remove_dir_all(cache_home);

  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let svg = String::from_utf8_lossy(&out.stdout);
  assert!(svg.contains("data-chart=\"plot\""), "svg: {svg}");
  assert!(svg.contains("data-resolution=\"hour\""), "svg: {svg}");
  assert!(svg.contains("Hourly token activity graph"), "svg: {svg}");
  assert_eq!(svg.matches("class=\"activity-hit-target\"").count(), 13);
}

#[cfg(unix)]
#[test]
fn graph_hour_buckets_align_to_local_hours_in_fractional_offset_timezones() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex/sessions");
  let (mut cmd, cache_home) = isolated_cmd("graph-hourly-fractional-timezone");
  let out = cmd
    .env("TZ", "Asia/Kolkata")
    .args([
      "graph",
      "--source",
      "codex",
      "--codex-dir",
      fixtures.to_str().unwrap(),
      "--no-cache",
      "--since",
      "2025-01-02T00:00:00Z",
      "--until",
      "2025-01-02T12:00:00Z",
      "--format",
      "svg",
    ])
    .output()
    .expect("run hourly graph in a fractional-offset timezone");
  let _ = std::fs::remove_dir_all(cache_home);

  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let svg = String::from_utf8_lossy(&out.stdout);
  assert!(
    svg.contains("Hourly token activity · Jan 2, 05:30–17:30, 2025"),
    "svg: {svg}"
  );
  assert!(svg.contains("Jan 2, 2025 05:00 +05:30: 0"), "svg: {svg}");
}

#[cfg(unix)]
#[test]
fn graph_hour_buckets_keep_repeated_dst_hours_distinct() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex/sessions");
  let (mut cmd, cache_home) = isolated_cmd("graph-hourly-dst-repeat");
  let out = cmd
    .env("TZ", "America/Los_Angeles")
    .args([
      "graph",
      "--source",
      "codex",
      "--codex-dir",
      fixtures.to_str().unwrap(),
      "--no-cache",
      "--since",
      "2025-11-02T08:00:00Z",
      "--until",
      "2025-11-02T10:00:00Z",
      "--format",
      "svg",
    ])
    .output()
    .expect("run hourly graph across a repeated DST hour");
  let _ = std::fs::remove_dir_all(cache_home);

  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let svg = String::from_utf8_lossy(&out.stdout);
  assert_eq!(svg.matches("class=\"activity-hit-target\"").count(), 3);
  assert!(
    svg.contains("Hourly token activity · Nov 2, 01:00 -07:00–02:00 -08:00, 2025"),
    "svg: {svg}"
  );
  assert!(svg.contains("Nov 2, 2025 01:00 -07:00: 0"), "svg: {svg}");
  assert!(svg.contains("Nov 2, 2025 01:00 -08:00: 0"), "svg: {svg}");
}

#[cfg(unix)]
#[test]
fn graph_hour_buckets_survive_half_hour_dst_shifts() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex/sessions");
  let (mut cmd, cache_home) = isolated_cmd("graph-hourly-half-hour-dst");
  let out = cmd
    .env("TZ", "Australia/Lord_Howe")
    .args([
      "graph",
      "--source",
      "codex",
      "--codex-dir",
      fixtures.to_str().unwrap(),
      "--no-cache",
      "--since",
      "2025-04-05T14:15:00Z",
      "--until",
      "2025-04-05T16:15:00Z",
      "--format",
      "svg",
    ])
    .output()
    .expect("run hourly graph across a half-hour DST shift");
  let _ = std::fs::remove_dir_all(cache_home);

  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let svg = String::from_utf8_lossy(&out.stdout);
  assert_eq!(svg.matches("class=\"activity-hit-target\"").count(), 3);
  assert!(svg.contains("Apr 6, 2025 01:00 +11:00: 0"), "svg: {svg}");
  assert!(svg.contains("Apr 6, 2025 01:30 +10:30: 0"), "svg: {svg}");
  assert!(svg.contains("Apr 6, 2025 02:30 +10:30: 0"), "svg: {svg}");
}

#[test]
fn graph_bytes_unit_uses_daily_input_and_output_bytes() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex/sessions");
  let (mut cmd, cache_home) = isolated_cmd("graph-bytes");
  let out = cmd
    .args([
      "graph",
      "--source",
      "codex",
      "--codex-dir",
      fixtures.to_str().unwrap(),
      "--no-cache",
      "--since",
      "2025-01-01",
      "--until",
      "2025-01-07",
      "--unit",
      "bytes",
      "--no-color",
    ])
    .output()
    .expect("run byte activity graph");
  let _ = std::fs::remove_dir_all(cache_home);

  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let graph = String::from_utf8_lossy(&out.stdout);
  assert!(graph.contains("Byte activity · Jan 1–7, 2025"), "graph: {graph}");
  assert!(graph.contains("Total ~71"), "graph: {graph}");
}

#[test]
fn graph_rejects_json_output() {
  let (mut cmd, cache_home) = isolated_cmd("graph-json");
  let out = cmd
    .args(["graph", "--source", "codex", "--no-cache", "--format", "json"])
    .output()
    .expect("run unsupported JSON activity graph");
  let _ = std::fs::remove_dir_all(cache_home);

  assert!(!out.status.success());
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert!(stderr.contains("--format json is not supported"), "stderr: {stderr}");
  assert!(!stderr.contains("processing "), "stderr: {stderr}");
}

#[cfg(unix)]
#[test]
fn graph_date_only_bounds_include_the_complete_day() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex/sessions");
  let (mut cmd, cache_home) = isolated_cmd("graph-date-bound");
  let out = cmd
    .env("TZ", "America/Los_Angeles")
    .args([
      "graph",
      "--source",
      "codex",
      "--codex-dir",
      fixtures.to_str().unwrap(),
      "--no-cache",
      "--since",
      "2025-01-02",
      "--until",
      "2025-01-02",
      "--no-color",
    ])
    .output()
    .expect("run activity graph with local date bounds");
  let _ = std::fs::remove_dir_all(cache_home);

  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let graph = String::from_utf8_lossy(&out.stdout);
  assert!(
    graph.contains("Hourly token activity · Jan 2, 00:00–23:59, 2025"),
    "graph: {graph}"
  );
  assert!(graph.contains("Total 720 · Active 1/24 hours"), "graph: {graph}");
}

#[cfg(unix)]
#[test]
fn graph_date_only_resolution_follows_dst_day_length() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex/sessions");

  for (date, expected_title, expected_summary) in [
    (
      "2025-03-09",
      "Hourly token activity · Mar 9, 00:00 -08:00–23:59 -07:00, 2025",
      "Active 0/23 hours",
    ),
    (
      "2025-11-02",
      "Hourly token activity · Nov 2, 00:00 -07:00–23:59 -08:00, 2025",
      "Active 0/25 hours",
    ),
  ] {
    let (mut cmd, cache_home) = isolated_cmd(&format!("graph-date-dst-{date}"));
    let out = cmd
      .env("TZ", "America/Los_Angeles")
      .args([
        "graph",
        "--source",
        "codex",
        "--codex-dir",
        fixtures.to_str().unwrap(),
        "--no-cache",
        "--since",
        date,
        "--until",
        date,
        "--no-color",
      ])
      .output()
      .expect("run activity graph across a DST date");
    let _ = std::fs::remove_dir_all(cache_home);

    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let graph = String::from_utf8_lossy(&out.stdout);
    assert!(graph.contains(expected_title), "graph: {graph}");
    assert!(graph.contains(expected_summary), "graph: {graph}");
  }
}

#[test]
fn graph_marks_cost_from_estimated_tokens_as_estimated() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/copilot/workspaceStorage");
  let (mut cmd, cache_home) = isolated_cmd("graph-estimated-cost");
  let out = cmd
    .args([
      "graph",
      "--source",
      "copilot",
      "--copilot-dir",
      fixtures.to_str().unwrap(),
      "--no-cache",
      "--since",
      "2026-04-17",
      "--until",
      "2026-04-18",
      "--unit",
      "cost",
      "--no-color",
    ])
    .output()
    .expect("run estimated cost activity graph");
  let _ = std::fs::remove_dir_all(cache_home);

  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let graph = String::from_utf8_lossy(&out.stdout);
  assert!(graph.contains("Total ~$"), "graph: {graph}");
}

#[test]
fn graph_applies_config_defaults_with_options_after_subcommand() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex/sessions");
  let (config, config_dir) = temp_config_file(
    "graph-config",
    r#"
[table]
unit = "bytes"
no-color = true
"#,
  );
  let (mut cmd, cache_home) = isolated_cmd("graph-config");
  let out = cmd
    .args([
      "graph",
      "--config",
      config.to_str().unwrap(),
      "--source",
      "codex",
      "--codex-dir",
      fixtures.to_str().unwrap(),
      "--no-cache",
      "--since",
      "2025-01-01",
      "--until",
      "2025-01-07",
    ])
    .output()
    .expect("run activity graph with config defaults");
  let _ = std::fs::remove_dir_all(cache_home);
  let _ = std::fs::remove_dir_all(config_dir);

  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let graph = String::from_utf8_lossy(&out.stdout);
  assert!(graph.contains("Byte activity · Jan 1–7, 2025"), "graph: {graph}");
  assert!(!graph.contains("\x1b["), "graph: {graph}");
}

#[test]
fn pi_agent_fixture_parses_usage() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pi_agent/sessions");
  let (mut cmd, cache_home) = isolated_cmd("pi-agent");
  let out = cmd
    .args([
      "--source",
      "pi-agent",
      "--pi-agent-dir",
      fixtures.to_str().unwrap(),
      "--format",
      "json",
      "--no-cache",
    ])
    .output()
    .expect("run llm-tokei");
  let _ = std::fs::remove_dir_all(cache_home);
  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid json");
  let arr = v.as_array().unwrap();
  assert_eq!(arr.len(), 2);
  let row = arr
    .iter()
    .find(|row| row["keys"]["model"] == "deepseek-v4-pro")
    .expect("pro row");
  assert_eq!(row["keys"]["source"], "pi-agent");
  assert_eq!(row["keys"]["model"], "deepseek-v4-pro");
  assert_eq!(row["input"], 3210);
  assert_eq!(row["output"], 30);
  assert_eq!(row["cache_read"], 3000);
  assert_eq!(row["cache_write"], 50);
  assert_eq!(row["total"], 3240);
  assert_eq!(row["calls"], 2);
  assert_eq!(row["rounds"], 1);
  assert_eq!(row["sessions"], 1);
  assert!((row["cost_embedded"].as_f64().unwrap() - 0.03).abs() < 1e-9);

  let flash = arr
    .iter()
    .find(|row| row["keys"]["model"] == "deepseek-v4-flash")
    .expect("flash row");
  assert_eq!(flash["keys"]["source"], "pi-agent");
  assert_eq!(flash["input"], 143);
  assert_eq!(flash["output"], 4);
  assert_eq!(flash["input_estimated"], true);
  assert_eq!(flash["output_estimated"], true);
  assert_eq!(flash["calls"], 1);
  assert_eq!(flash["rounds"], 0);
}

#[test]
fn codex_fixture_reports_response_item_bytes() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex/sessions");
  let (mut cmd, cache_home) = isolated_cmd("codex-bytes");
  let out = cmd
    .args([
      "--source",
      "codex",
      "--codex-dir",
      fixtures.to_str().unwrap(),
      "--opencode-db",
      "/nonexistent/opencode.db",
      "--format",
      "json",
      "--bytes",
    ])
    .output()
    .expect("run llm-tokei bytes mode");
  let _ = std::fs::remove_dir_all(cache_home);
  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let s = String::from_utf8_lossy(&out.stdout);
  let v: serde_json::Value = serde_json::from_str(&s).expect("valid json");
  let arr = v.as_array().unwrap();
  assert_eq!(arr.len(), 1);
  let row = &arr[0];
  assert_eq!(row["input"], 37);
  assert_eq!(row["output"], 34);
  assert_eq!(row["reasoning"], 50);
  assert_eq!(row["total"], 720);
  assert_eq!(row["calls"], 4);
  assert_eq!(row["rounds"], 2);
}

#[test]
fn codex_bytes_mode_rebuilds_stale_zero_byte_cache() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex/sessions");
  let cache_home = temp_cache_home("stale-codex-bytes");
  std::fs::create_dir_all(&cache_home).expect("create cache home");

  let cache_path = cache_home.join("llm-tokei.db");
  {
    let conn = rusqlite::Connection::open(&cache_path).expect("open stale cache");
    conn
      .execute_batch(
        r#"
        PRAGMA user_version = 3;
        CREATE TABLE sessions (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            source        TEXT NOT NULL,
            session_id    TEXT NOT NULL,
            session_title TEXT,
            project_cwd   TEXT,
            project_name  TEXT,
            file_path     TEXT NOT NULL,
            first_ts      TEXT NOT NULL,
            last_ts       TEXT NOT NULL,
            file_mtime    INTEGER NOT NULL,
            pruned        INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE records (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            session_rowid INTEGER NOT NULL REFERENCES sessions(id),
            provider      TEXT,
            model         TEXT,
            ts            TEXT NOT NULL,
            prompt        INTEGER NOT NULL,
            completion    INTEGER NOT NULL,
            input_bytes   INTEGER NOT NULL,
            output_bytes  INTEGER NOT NULL,
            input_estimated INTEGER NOT NULL,
            output_estimated INTEGER NOT NULL,
            input_bytes_estimated INTEGER NOT NULL,
            output_bytes_estimated INTEGER NOT NULL,
            reasoning     INTEGER NOT NULL,
            cache_read    INTEGER NOT NULL,
            cache_write   INTEGER NOT NULL,
            mode          TEXT,
            agent         TEXT,
            is_compaction INTEGER NOT NULL,
            rounds        INTEGER NOT NULL,
            calls         INTEGER NOT NULL,
            cost_embedded REAL
        );
        "#,
      )
      .expect("create stale schema");
  }

  let out = Command::new(bin())
    .env("XDG_CACHE_HOME", &cache_home)
    .args([
      "--source",
      "codex",
      "--codex-dir",
      fixtures.to_str().unwrap(),
      "--opencode-db",
      "/nonexistent/opencode.db",
      "--format",
      "json",
      "--bytes",
    ])
    .output()
    .expect("run llm-tokei bytes mode with stale cache");

  let _ = std::fs::remove_dir_all(&cache_home);

  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid json");
  let row = &v.as_array().unwrap()[0];
  assert_eq!(row["input"], 37);
  assert_eq!(row["output"], 34);
}

#[test]
fn claude_fixture_parses_usage() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/claude/projects");
  let out = Command::new(bin())
    .args([
      "--source",
      "claude",
      "--claude-dir",
      fixtures.to_str().unwrap(),
      "--format",
      "json",
    ])
    .output()
    .expect("run llm-tokei");
  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let s = String::from_utf8_lossy(&out.stdout);
  let v: serde_json::Value = serde_json::from_str(&s).expect("valid json");
  let arr = v.as_array().unwrap();
  assert_eq!(arr.len(), 1);
  let row = &arr[0];
  // Two assistant calls:
  //   #1: input=50, output=40, cache_read=100, cache_write=30
  //   #2: input=10, output=20, cache_read=150, cache_write=5+2=7
  // displayed input = (50+100+30) + (10+150+7) = 347
  // output = 40+20 = 60
  // cache_read  = 250
  // cache_write = 37
  // total = input + output = 347+60 = 407
  assert_eq!(row["input"], 347);
  assert_eq!(row["output"], 60);
  assert_eq!(row["reasoning"], 0);
  assert_eq!(row["cache_read"], 250);
  assert_eq!(row["cache_write"], 37);
  assert_eq!(row["total"], 407);
  assert_eq!(row["calls"], 2);
  assert_eq!(row["rounds"], 1);
  assert_eq!(row["sessions"], 1);
  assert_eq!(row["keys"]["model"], "claude-sonnet-4.5");
  assert_eq!(row["keys"]["source"], "claude");
}

#[test]
fn copilot_fixture_estimates_and_thinking() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/copilot/workspaceStorage");
  let out = Command::new(bin())
    .args([
      "--source",
      "copilot",
      "--copilot-dir",
      fixtures.to_str().unwrap(),
      "--format",
      "json",
      "--group-by",
      "source,model,project",
      "--no-cache",
    ])
    .output()
    .expect("run llm-tokei");
  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let s = String::from_utf8_lossy(&out.stdout);
  let v: serde_json::Value = serde_json::from_str(&s).expect("valid json");
  let arr = v.as_array().unwrap();
  let row = arr
    .iter()
    .find(|row| row["keys"]["model"] == "claude-sonnet-4.5")
    .expect("fixture row for claude-sonnet-4.5");
  // Per-turn (per-request) ceil(chars/4) — request 1 input chars 31+18+26=75 → 19;
  //   request 2 input chars 7 → 2; total 21.
  // Output per request: 32+12=44 → 11; 5 → 2; total 13.
  // (`toolCallResults` is preferred over short `toolCallRounds.response` summaries.)
  // reasoning = 17 (exact, from thinking.tokens)
  assert_eq!(row["input"], 21);
  assert_eq!(row["output"], 30);
  assert_eq!(row["reasoning"], 17);
  assert_eq!(row["cache_read"], 0);
  assert_eq!(row["cache_write"], 0);
  assert_eq!(row["calls"], 3);
  assert_eq!(row["rounds"], 2);
  assert_eq!(row["sessions"], 1);
  assert_eq!(row["keys"]["source"], "copilot");
  assert_eq!(row["keys"]["model"], "claude-sonnet-4.5");
  let project = row["keys"]["project"].as_str().unwrap();
  assert!(project.ends_with("myrepo"), "got {project}");
}

#[test]
fn copilot_transcript_shutdown_dedupes_chat_session() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/copilot_exact/workspaceStorage");
  let out = Command::new(bin())
    .args([
      "--source",
      "copilot",
      "--copilot-dir",
      fixtures.to_str().unwrap(),
      "--format",
      "json",
      "--group-by",
      "source,model,project",
      "--no-cache",
    ])
    .output()
    .expect("run llm-tokei");
  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let s = String::from_utf8_lossy(&out.stdout);
  let v: serde_json::Value = serde_json::from_str(&s).expect("valid json");
  let arr = v.as_array().unwrap();
  assert_eq!(arr.len(), 1);
  let row = &arr[0];
  assert_eq!(row["input"], 10);
  assert_eq!(row["output"], 20);
  assert_eq!(row["cache_read"], 3);
  assert_eq!(row["cache_write"], 4);
  assert_eq!(row["total"], 30);
  assert_eq!(row["calls"], 2);
  assert_eq!(row["rounds"], 2);
  assert_eq!(row["sessions"], 1);
  assert_eq!(row["keys"]["source"], "copilot");
  assert_eq!(row["keys"]["model"], "gpt-5-mini");
  assert_eq!(row["keys"]["project"], "exactrepo");
}

#[test]
fn copilot_cli_fixture_parses_fallback_and_compaction() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/copilot_cli/session-state");
  let out = Command::new(bin())
    .args([
      "--source",
      "copilot-cli",
      "--copilot-cli-dir",
      fixtures.to_str().unwrap(),
      "--format",
      "json",
      "--no-cache",
    ])
    .output()
    .expect("run llm-tokei");
  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let s = String::from_utf8_lossy(&out.stdout);
  let v: serde_json::Value = serde_json::from_str(&s).expect("valid json");
  let arr = v.as_array().unwrap();
  assert_eq!(arr.len(), 1);
  let row = &arr[0];
  assert_eq!(row["input"], 20);
  assert_eq!(row["output"], 23);
  assert_eq!(row["reasoning"], 3);
  assert_eq!(row["cache_read"], 5);
  assert_eq!(row["cache_write"], 2);
  assert_eq!(row["total"], 43);
  assert_eq!(row["calls"], 2);
  assert_eq!(row["rounds"], 1);
  assert_eq!(row["sessions"], 1);
  assert_eq!(row["keys"]["source"], "copilot-cli");
  assert_eq!(row["keys"]["model"], "gpt-5-mini");
}

#[test]
fn copilot_cli_shutdown_merges_estimated_bytes() {
  let fixtures =
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/copilot_cli_shutdown/session-state");
  let out = Command::new(bin())
    .args([
      "--source",
      "copilot-cli",
      "--copilot-cli-dir",
      fixtures.to_str().unwrap(),
      "--format",
      "json",
      "--no-cache",
      "--no-config",
    ])
    .output()
    .expect("run llm-tokei");
  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let s = String::from_utf8_lossy(&out.stdout);
  let v: serde_json::Value = serde_json::from_str(&s).expect("valid json");
  let row = &v.as_array().unwrap()[0];
  // Shutdown provides exact tokens; display_input = input + cache_read + cache_write.
  assert_eq!(row["input"], 50);
  assert_eq!(row["output"], 30);
  assert_eq!(row["reasoning"], 5);
  assert_eq!(row["cache_read"], 10);
  assert_eq!(row["cache_write"], 3);
  assert_eq!(row["total"], 80);
  assert_eq!(row["calls"], 2);
  assert_eq!(row["rounds"], 2);
  assert_eq!(row["sessions"], 1);
  assert_eq!(row["keys"]["source"], "copilot-cli");
  assert_eq!(row["keys"]["model"], "gpt-5-mini");
}

#[test]
fn bytes_mode_switches_input_output_units_only() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/copilot/workspaceStorage");
  let base_args = [
    "--source",
    "copilot",
    "--copilot-dir",
    fixtures.to_str().unwrap(),
    "--format",
    "json",
    "--group-by",
    "source,model,project",
    "--no-cache",
  ];

  let token_out = Command::new(bin())
    .args(base_args)
    .output()
    .expect("run llm-tokei token mode");
  assert!(
    token_out.status.success(),
    "stderr: {}",
    String::from_utf8_lossy(&token_out.stderr)
  );
  let token_v: serde_json::Value = serde_json::from_slice(&token_out.stdout).expect("valid json in token mode");
  let token_row = token_v
    .as_array()
    .unwrap()
    .iter()
    .find(|row| row["keys"]["model"] == "claude-sonnet-4.5")
    .expect("token row for claude-sonnet-4.5");

  let bytes_out = Command::new(bin())
    .args(base_args)
    .arg("--bytes")
    .output()
    .expect("run llm-tokei bytes mode");
  assert!(
    bytes_out.status.success(),
    "stderr: {}",
    String::from_utf8_lossy(&bytes_out.stderr)
  );
  let bytes_v: serde_json::Value = serde_json::from_slice(&bytes_out.stdout).expect("valid json in bytes mode");
  let bytes_row = bytes_v
    .as_array()
    .unwrap()
    .iter()
    .find(|row| row["keys"]["model"] == "claude-sonnet-4.5")
    .expect("bytes row for claude-sonnet-4.5");

  assert_eq!(token_row["input"], 21);
  assert_eq!(token_row["output"], 30);
  assert_eq!(bytes_row["input"], 82);
  assert_eq!(bytes_row["output"], 49);
  assert_eq!(token_row["total"], bytes_row["total"]);
  assert_eq!(token_row["cost"], bytes_row["cost"]);
}

#[test]
fn unit_bytes_matches_legacy_bytes_flag() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/copilot/workspaceStorage");
  let base_args = [
    "--source",
    "copilot",
    "--copilot-dir",
    fixtures.to_str().unwrap(),
    "--format",
    "json",
    "--group-by",
    "source,model,project",
    "--no-cache",
  ];

  let legacy = Command::new(bin())
    .args(base_args)
    .arg("--bytes")
    .output()
    .expect("run llm-tokei legacy bytes mode");
  assert!(
    legacy.status.success(),
    "stderr: {}",
    String::from_utf8_lossy(&legacy.stderr)
  );

  let unit = Command::new(bin())
    .args(base_args)
    .args(["--unit", "bytes"])
    .output()
    .expect("run llm-tokei unit bytes mode");
  assert!(
    unit.status.success(),
    "stderr: {}",
    String::from_utf8_lossy(&unit.stderr)
  );

  assert_eq!(legacy.stdout, unit.stdout);
}

#[test]
fn bytes_mode_table_header_uses_byte_suffix() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/copilot/workspaceStorage");
  let out = Command::new(bin())
    .args([
      "--source",
      "copilot",
      "--copilot-dir",
      fixtures.to_str().unwrap(),
      "--group-by",
      "source,model",
      "--bytes",
      "--no-cache",
    ])
    .output()
    .expect("run llm-tokei table bytes mode");
  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let table = String::from_utf8_lossy(&out.stdout);
  assert!(table.contains("input(B)"), "table output: {table}");
  assert!(table.contains("output(B)"), "table output: {table}");
}

#[test]
fn unit_cost_switches_usage_columns_to_costs() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/copilot_cli/session-state");
  let out = Command::new(bin())
    .args([
      "--source",
      "copilot-cli",
      "--copilot-cli-dir",
      fixtures.to_str().unwrap(),
      "--cost",
      "mixed",
      "--unit",
      "cost",
      "--format",
      "json",
      "--no-cache",
    ])
    .output()
    .expect("run llm-tokei cost unit mode");

  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid json");
  let row = &v.as_array().unwrap()[0];

  let input = row["input"].as_f64().unwrap();
  let output = row["output"].as_f64().unwrap();
  let reasoning = row["reasoning"].as_f64().unwrap();
  let total = row["total"].as_f64().unwrap();
  let cost = row["cost"].as_f64().unwrap();

  assert!(input > 0.0, "row: {row}");
  assert!(output > 0.0, "row: {row}");
  assert!(reasoning >= 0.0, "row: {row}");
  assert!((total - cost).abs() < 1e-12, "row: {row}");
}

#[test]
fn table_width_fits_table_output() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/copilot/workspaceStorage");
  let out = Command::new(bin())
    .args([
      "--source",
      "copilot",
      "--copilot-dir",
      fixtures.to_str().unwrap(),
      "--group-by",
      "source,model",
      "--no-cache",
      "--table-width",
      "50",
    ])
    .output()
    .expect("run llm-tokei fitted table");
  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let table = String::from_utf8_lossy(&out.stdout);
  let header = table.lines().next().unwrap_or_default();
  assert!(header.contains("source"), "table output: {table}");
  assert!(header.contains("model"), "table output: {table}");
  assert!(header.contains("total"), "table output: {table}");
  assert!(table.contains("hidden columns:"), "table output: {table}");
}

#[test]
fn table_width_does_not_affect_json_output() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex/sessions");
  let out = Command::new(bin())
    .args([
      "--source",
      "codex",
      "--codex-dir",
      fixtures.to_str().unwrap(),
      "--format",
      "json",
      "--no-cache",
      "--table-width",
      "20",
    ])
    .output()
    .expect("run llm-tokei json with table width");
  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid json");
  let row = &v.as_array().unwrap()[0];
  assert_eq!(row["input"], 500);
  assert_eq!(row["output"], 220);
  assert_eq!(row["total"], 720);
}

#[test]
fn no_fit_conflicts_with_table_width() {
  let out = Command::new(bin())
    .args(["--no-fit", "--table-width", "80"])
    .output()
    .expect("run llm-tokei with conflicting fit args");
  assert!(!out.status.success());
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert!(stderr.contains("--no-fit"), "stderr: {stderr}");
  assert!(stderr.contains("--table-width"), "stderr: {stderr}");
}

#[test]
fn config_file_sets_main_flag_defaults() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex/sessions");
  let (config_path, config_dir) = temp_config_file(
    "config-defaults",
    r#"
format = "json"
source = ["codex"]
group-by = ["provider"]
cost = "official"
no-cache = true
"#,
  );

  let out = Command::new(bin())
    .args([
      "--config",
      config_path.to_str().unwrap(),
      "--codex-dir",
      fixtures.to_str().unwrap(),
      "--opencode-db",
      "/nonexistent/opencode.db",
    ])
    .output()
    .expect("run llm-tokei with config");
  let _ = std::fs::remove_dir_all(config_dir);

  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid json");
  let row = &v.as_array().unwrap()[0];
  assert_eq!(row["keys"]["provider"], "openai");
  assert!(row["keys"].get("model").is_none());
}

#[test]
fn cli_flags_override_config_file_defaults() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex/sessions");
  let (config_path, config_dir) = temp_config_file(
    "config-cli-override",
    r#"
format = "json"
source = ["codex"]
group-by = ["provider"]
no-cache = true
"#,
  );

  let out = Command::new(bin())
    .args([
      "--config",
      config_path.to_str().unwrap(),
      "--group-by",
      "source,model",
      "--codex-dir",
      fixtures.to_str().unwrap(),
      "--opencode-db",
      "/nonexistent/opencode.db",
    ])
    .output()
    .expect("run llm-tokei with config override");
  let _ = std::fs::remove_dir_all(config_dir);

  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid json");
  let row = &v.as_array().unwrap()[0];
  assert_eq!(row["keys"]["source"], "codex");
  assert_eq!(row["keys"]["model"], "gpt-5");
  assert!(row["keys"].get("provider").is_none());
}

#[test]
fn config_args_saves_structured_defaults() {
  let (config_path, config_dir) = temp_config_file("config-args-save", "");
  let save = Command::new(bin())
    .args([
      "--config",
      config_path.to_str().unwrap(),
      "config",
      "args",
      "--",
      "--cost official --group-by provider --human --source codex",
    ])
    .output()
    .expect("run config args");
  assert!(
    save.status.success(),
    "stderr: {}",
    String::from_utf8_lossy(&save.stderr)
  );

  let contents = std::fs::read_to_string(&config_path).expect("read saved config");
  let _ = std::fs::remove_dir_all(config_dir);
  assert!(contents.contains("cost = \"official\""), "config: {contents}");
  assert!(contents.contains("group-by = [\"provider\"]"), "config: {contents}");
  assert!(contents.contains("human = true"), "config: {contents}");
  assert!(contents.contains("source = [\"codex\"]"), "config: {contents}");
}

#[test]
fn save_default_saves_current_main_flags_and_runs() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex/sessions");
  let (config_path, config_dir) = temp_config_file("config-save-default", "");
  let out = Command::new(bin())
    .args([
      "--config",
      config_path.to_str().unwrap(),
      "--save-default",
      "--format",
      "json",
      "--source",
      "codex",
      "--group-by",
      "provider",
      "--codex-dir",
      fixtures.to_str().unwrap(),
      "--opencode-db",
      "/nonexistent/opencode.db",
      "--no-cache",
    ])
    .output()
    .expect("run save-default");
  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid json");
  assert_eq!(v.as_array().unwrap()[0]["keys"]["provider"], "openai");

  let contents = std::fs::read_to_string(&config_path).expect("read saved config");
  let _ = std::fs::remove_dir_all(config_dir);
  assert!(contents.contains("format = \"json\""), "config: {contents}");
  assert!(contents.contains("group-by = [\"provider\"]"), "config: {contents}");
  assert!(!contents.contains("save-default"), "config: {contents}");
}

#[test]
fn no_default_skips_saved_config_defaults() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex/sessions");
  let (config_path, config_dir) = temp_config_file(
    "config-no-default",
    r#"
format = "json"
source = ["codex"]
group-by = ["provider"]
no-cache = true
"#,
  );
  let out = Command::new(bin())
    .args([
      "--config",
      config_path.to_str().unwrap(),
      "--no-default",
      "--source",
      "codex",
      "--codex-dir",
      fixtures.to_str().unwrap(),
      "--opencode-db",
      "/nonexistent/opencode.db",
      "--format",
      "json",
      "--no-cache",
    ])
    .output()
    .expect("run no-default");
  let _ = std::fs::remove_dir_all(config_dir);
  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid json");
  assert_eq!(v.as_array().unwrap()[0]["keys"]["model"], "gpt-5");
}

#[test]
fn config_args_reset_clears_defaults() {
  let (config_path, config_dir) = temp_config_file(
    "config-args-reset",
    r#"
format = "json"
group-by = ["provider"]
"#,
  );
  let out = Command::new(bin())
    .args(["--config", config_path.to_str().unwrap(), "config", "args", "--reset"])
    .output()
    .expect("run config args reset");
  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let contents = std::fs::read_to_string(&config_path).expect("read reset config");
  let _ = std::fs::remove_dir_all(config_dir);
  assert!(contents.trim().is_empty(), "config: {contents}");
}

#[test]
fn xdg_default_config_path_is_flat_file() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex/sessions");
  let config_home = temp_cache_home("flat-xdg-config");
  std::fs::create_dir_all(&config_home).expect("create xdg config home");
  let config_path = config_home.join("llm-tokei.toml");
  std::fs::write(
    &config_path,
    r#"
format = "json"
source = ["codex"]
group-by = ["provider"]
no-cache = true
"#,
  )
  .expect("write default config");

  let out = Command::new(bin())
    .env("XDG_CONFIG_HOME", &config_home)
    .args([
      "--codex-dir",
      fixtures.to_str().unwrap(),
      "--opencode-db",
      "/nonexistent/opencode.db",
    ])
    .output()
    .expect("run with xdg config");
  let _ = std::fs::remove_dir_all(config_home);
  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid json");
  assert_eq!(v.as_array().unwrap()[0]["keys"]["provider"], "openai");
}

#[test]
fn config_list_prints_current_config() {
  let (config_path, config_dir) = temp_config_file(
    "config-list",
    r#"
cost = "official"
human = true
"#,
  );
  let out = Command::new(bin())
    .args(["--config", config_path.to_str().unwrap(), "config", "list"])
    .output()
    .expect("run config list");
  let _ = std::fs::remove_dir_all(config_dir);
  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let stdout = String::from_utf8_lossy(&out.stdout);
  assert!(stdout.contains(config_path.to_str().unwrap()), "stdout: {stdout}");
  assert!(stdout.contains("cost = \"official\""), "stdout: {stdout}");
  assert!(stdout.contains("human = true"), "stdout: {stdout}");
}

#[test]
fn config_save_canonicalizes_alias_flags() {
  let (config_path, config_dir) = temp_config_file("config-canonical-alias", "");
  let out = Command::new(bin())
    .args([
      "--config",
      config_path.to_str().unwrap(),
      "config",
      "args",
      "--",
      "--24h -h -v",
    ])
    .output()
    .expect("run config args aliases");
  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let contents = std::fs::read_to_string(&config_path).expect("read saved config");
  let _ = std::fs::remove_dir_all(config_dir);
  assert!(contents.contains("period = \"24h\""), "config: {contents}");
  assert!(!contents.contains("24h = true"), "config: {contents}");
  assert!(contents.contains("human = true"), "config: {contents}");
  assert!(contents.contains("verbose = true"), "config: {contents}");
}

#[test]
fn cli_period_alias_overrides_config_period_without_conflict() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex/sessions");
  let (config_path, config_dir) = temp_config_file(
    "config-period-override",
    r#"
format = "json"
source = ["codex"]
period = "month"
group-by = ["source", "model"]
no-cache = true
"#,
  );
  let out = Command::new(bin())
    .args([
      "--config",
      config_path.to_str().unwrap(),
      "--24h",
      "--codex-dir",
      fixtures.to_str().unwrap(),
      "--opencode-db",
      "/nonexistent/opencode.db",
    ])
    .output()
    .expect("run period override");
  let _ = std::fs::remove_dir_all(config_dir);
  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn missing_cache_write_price_falls_back_to_input_price() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/copilot_cli/session-state");
  let pricing_path = temp_file_path("pricing-override");
  std::fs::write(
    &pricing_path,
    r#"{
  "prices": [
    {
      "provider": "github-copilot",
      "model": "gpt-5-mini",
      "input": 1.0,
      "output": 0.0,
      "cache_read": 0.0,
      "cache_write": null
    }
  ],
  "models": {
    "gpt-5-mini": { "provider": "github-copilot" }
  },
  "providers": {
    "github-copilot": {
      "included": false,
      "models": {
        "gpt-5-mini": { "included": false, "multiplier": 1.0 }
      }
    }
  }
}
"#,
  )
  .expect("write pricing override");

  let out = Command::new(bin())
    .args([
      "--source",
      "copilot-cli",
      "--copilot-cli-dir",
      fixtures.to_str().unwrap(),
      "--pricing",
      pricing_path.to_str().unwrap(),
      "--cost",
      "actual",
      "--format",
      "json",
      "--no-cache",
    ])
    .output()
    .expect("run llm-tokei");

  let _ = std::fs::remove_file(&pricing_path);

  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let s = String::from_utf8_lossy(&out.stdout);
  let v: serde_json::Value = serde_json::from_str(&s).expect("valid json");
  let arr = v.as_array().unwrap();
  assert_eq!(arr.len(), 1);
  let row = &arr[0];

  let cost = row["cost"].as_f64().unwrap();
  assert!((cost - 0.000015).abs() < 1e-9, "got {cost}");
}

#[test]
fn explicit_zero_cache_write_price_stays_zero() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/copilot_cli/session-state");
  let pricing_path = temp_file_path("pricing-override-zero");
  std::fs::write(
    &pricing_path,
    r#"{
  "prices": [
    {
      "provider": "github-copilot",
      "model": "gpt-5-mini",
      "input": 0.0,
      "output": 0.0,
      "cache_read": 0.0,
      "cache_write": 0.0
    }
  ],
  "models": {
    "gpt-5-mini": { "provider": "github-copilot" }
  },
  "providers": {
    "github-copilot": {
      "included": false,
      "models": {
        "gpt-5-mini": { "included": false, "multiplier": 1.0 }
      }
    }
  }
}
"#,
  )
  .expect("write pricing override");

  let out = Command::new(bin())
    .args([
      "--source",
      "copilot-cli",
      "--copilot-cli-dir",
      fixtures.to_str().unwrap(),
      "--pricing",
      pricing_path.to_str().unwrap(),
      "--cost",
      "actual",
      "--format",
      "json",
      "--no-cache",
    ])
    .output()
    .expect("run llm-tokei");

  let _ = std::fs::remove_file(&pricing_path);

  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let s = String::from_utf8_lossy(&out.stdout);
  let v: serde_json::Value = serde_json::from_str(&s).expect("valid json");
  let arr = v.as_array().unwrap();
  assert_eq!(arr.len(), 1);
  let row = &arr[0];

  let cost = row["cost"].as_f64().unwrap();
  assert!(cost.abs() < 1e-12, "got {cost}");
}

#[test]
fn cost_mode_mixed_uses_official_price_for_included_provider() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/copilot_cli/session-state");
  let out = Command::new(bin())
    .args([
      "--source",
      "copilot-cli",
      "--copilot-cli-dir",
      fixtures.to_str().unwrap(),
      "--cost",
      "mixed",
      "--format",
      "json",
      "--no-cache",
    ])
    .output()
    .expect("run llm-tokei mixed cost");

  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid json");
  let cost = v.as_array().unwrap()[0]["cost"].as_f64().unwrap();
  assert!(cost > 0.0, "got {cost}");
}

#[test]
fn cost_per_provider_adds_top_provider_columns_and_json_object() {
  let codex_fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex/sessions");
  let copilot_fixtures =
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/copilot/workspaceStorage");
  let pricing_path = temp_file_path("pricing-cost-per");
  std::fs::write(
    &pricing_path,
    r#"{
  "prices": [
    { "provider": "openai", "model": "gpt-5", "input": 1000.0, "output": 0.0, "cache_read": 0.0 },
    { "provider": "github-copilot", "model": "gpt-5.3-codex", "input": 1.0, "output": 0.0, "cache_read": 0.0 }
  ],
  "providers": {
    "github-copilot": { "included": false }
  }
}
"#,
  )
  .expect("write pricing override");

  let table_out = Command::new(bin())
    .args([
      "--source",
      "codex,copilot",
      "--codex-dir",
      codex_fixtures.to_str().unwrap(),
      "--copilot-dir",
      copilot_fixtures.to_str().unwrap(),
      "--pricing",
      pricing_path.to_str().unwrap(),
      "--group-by",
      "source",
      "--cost-per",
      "provider",
      "--no-cache",
      "--no-color",
    ])
    .output()
    .expect("run llm-tokei cost-per table");
  assert!(
    table_out.status.success(),
    "stderr: {}",
    String::from_utf8_lossy(&table_out.stderr)
  );
  let table = String::from_utf8_lossy(&table_out.stdout);
  let header = table.lines().next().unwrap_or_default();
  assert!(header.contains("openai"), "table output: {table}");
  assert!(header.contains("github-cop"), "table output: {table}");
  assert!(!header.contains("provider:"), "table output: {table}");

  let json_out = Command::new(bin())
    .args([
      "--source",
      "codex,copilot",
      "--codex-dir",
      codex_fixtures.to_str().unwrap(),
      "--copilot-dir",
      copilot_fixtures.to_str().unwrap(),
      "--pricing",
      pricing_path.to_str().unwrap(),
      "--group-by",
      "source",
      "--cost-per",
      "provider",
      "--format",
      "json",
      "--no-cache",
    ])
    .output()
    .expect("run llm-tokei cost-per json");
  let _ = std::fs::remove_file(&pricing_path);

  assert!(
    json_out.status.success(),
    "stderr: {}",
    String::from_utf8_lossy(&json_out.stderr)
  );
  let v: serde_json::Value = serde_json::from_slice(&json_out.stdout).expect("valid json");
  let rows = v.as_array().unwrap();
  assert!(rows
    .iter()
    .any(|row| row["cost_per"]["openai"].as_f64().unwrap_or(0.0) > 0.0));
  assert!(rows
    .iter()
    .any(|row| row["cost_per"]["github-copilot"].as_f64().unwrap_or(0.0) > 0.0));
}

#[test]
fn copilot_dump_fixture_bytes_cover_schema_variants() {
  // Exercises schema-driven bytes paths: message.text fallback,
  // progressTaskSerialized.content.value, toolInvocationSerialized
  // invocationMessage as string and as { value }, and thinking value
  // (which must NOT count as output bytes).
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/copilot_dump/workspaceStorage");
  let out = Command::new(bin())
    .args([
      "--source",
      "copilot",
      "--copilot-dir",
      fixtures.to_str().unwrap(),
      "--format",
      "json",
      "--bytes",
      "--group-by",
      "source,model,project",
      "--no-cache",
    ])
    .output()
    .expect("run llm-tokei");
  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid json");
  let arr = v.as_array().unwrap();
  assert_eq!(arr.len(), 1);
  let row = &arr[0];
  // Inputs:
  //   r1 message.text fallback "fallback prompt" = 15
  //   r2 renderedUserMessage "hi" (2) + tool result "result body" (11) = 13
  //   r3 renderedUserMessage "tool call please" (16) + tool result "file body" (9) = 25
  //   r4 renderedUserMessage "think" = 5
  // Total = 58.
  assert_eq!(row["input"], 58);
  // Outputs:
  //   r1 text "ack" = 3
  //   r2 progressTaskSerialized.content.value "progress text" (13) + tool args "{}" (2) = 15
  //   r3 invocationMessage "Reading files" (13) + pastTenseMessage.value "Read files" (10)
  //     + tool args "{\"path\":\"file\"}" (15) = 38
  //   r4 thinking "secret reasoning" → 0 (must not leak into output) + text "done" (4) = 4
  // Total = 60.
  assert_eq!(row["output"], 60);
  // reasoning comes from toolCallRounds.thinking.tokens (exact, not bytes).
  assert_eq!(row["reasoning"], 7);
}

#[test]
fn copilot_dump_subcommand_writes_role_user_jsonl() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/copilot_dump/workspaceStorage");
  let nanos = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .unwrap()
    .as_nanos();
  let out_dir = std::env::temp_dir().join(format!("llm-tokei-dump-{nanos}"));
  let _ = std::fs::remove_dir_all(&out_dir);

  let status = Command::new(bin())
    .args([
      "--copilot-dir",
      fixtures.to_str().unwrap(),
      "dump",
      "--copilot",
      "--out",
      out_dir.to_str().unwrap(),
    ])
    .status()
    .expect("run llm-tokei dump");
  assert!(status.success());

  let dest = out_dir.join("sess-dump-1.jsonl");
  let body = std::fs::read_to_string(&dest).expect("dump file written");
  let lines: Vec<&str> = body.lines().collect();
  assert_eq!(lines.len(), 11);
  let parsed: Vec<serde_json::Value> = lines
    .iter()
    .map(|l| serde_json::from_str(l).expect("valid jsonl"))
    .collect();

  // Order: prompt/assistant response pairs, with tool calls/results preserving call_id.
  assert_eq!(
    parsed
      .iter()
      .map(|rec| rec["role"].as_str().unwrap())
      .collect::<Vec<_>>(),
    vec![
      "user",
      "assistant",
      "user",
      "assistant",
      "tool_call",
      "tool_call_result",
      "user",
      "tool_call",
      "tool_call_result",
      "user",
      "assistant",
    ]
  );
  assert_eq!(parsed[0]["text"], "fallback prompt");
  assert!(parsed[0].get("call_id").is_none());
  assert_eq!(parsed[1]["text"], "ack");
  assert_eq!(parsed[2]["text"], "hi");
  assert_eq!(parsed[3]["text"], "progress text");
  assert_eq!(parsed[4]["role"], "tool_call");
  assert_eq!(parsed[4]["text"], "read: {}");
  assert_eq!(parsed[4]["call_id"], "c1");
  assert_eq!(parsed[5]["role"], "tool_call_result");
  assert_eq!(parsed[5]["text"], "result body");
  assert_eq!(parsed[5]["display"], "ok");
  assert_eq!(parsed[5]["call_id"], "c1");
  assert_eq!(parsed[6]["text"], "tool call please");
  assert_eq!(parsed[7]["role"], "tool_call");
  assert_eq!(parsed[7]["text"], "read");
  assert_eq!(parsed[7]["display"], "Reading files\nRead files");
  assert_eq!(parsed[7]["call_id"], "c2");
  assert_eq!(parsed[8]["role"], "tool_call_result");
  assert_eq!(parsed[8]["text"], "file body");
  assert_eq!(parsed[8]["display"], "Reading files\nRead files");
  assert_eq!(parsed[8]["call_id"], "c2");
  assert_eq!(parsed[9]["text"], "think");
  assert_eq!(parsed[10]["text"], "done");

  let _ = std::fs::remove_dir_all(&out_dir);
}

#[test]
fn copilot_dump_subcommand_writes_positional_file_to_stdout() {
  let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
    .join("tests/fixtures/copilot_dump/workspaceStorage/ws/chatSessions/sess-dump-1.jsonl");
  let out = Command::new(bin())
    .args(["dump", "--copilot", fixture.to_str().unwrap()])
    .output()
    .expect("run llm-tokei dump");
  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));

  let body = String::from_utf8_lossy(&out.stdout);
  let lines: Vec<&str> = body.lines().collect();
  assert_eq!(lines.len(), 12);
  assert_eq!(lines[0], format!("# {}", fixture.display()));
  let parsed: Vec<serde_json::Value> = lines[1..]
    .iter()
    .map(|line| serde_json::from_str(line).expect("valid jsonl"))
    .collect();
  assert_eq!(parsed[0]["text"], "fallback prompt");
  assert_eq!(parsed[4]["role"], "tool_call");
  assert_eq!(parsed[4]["text"], "read: {}");
  assert_eq!(parsed[4]["call_id"], "c1");
  assert_eq!(parsed[5]["role"], "tool_call_result");
  assert_eq!(parsed[5]["text"], "result body");
  assert_eq!(parsed[5]["display"], "ok");
  assert_eq!(parsed[5]["call_id"], "c1");
  assert_eq!(parsed[7]["role"], "tool_call");
  assert_eq!(parsed[7]["text"], "read");
  assert_eq!(parsed[7]["display"], "Reading files\nRead files");
  assert_eq!(parsed[8]["role"], "tool_call_result");
  assert_eq!(parsed[8]["display"], "Reading files\nRead files");
  assert_eq!(parsed[10]["text"], "done");
}

#[test]
fn copilot_cli_dump_subcommand_writes_positional_file_to_stdout() {
  let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
    .join("tests/fixtures/copilot_cli_dump/session-state/session1/events.jsonl");
  let out = Command::new(bin())
    .args(["dump", "--copilot-cli", fixture.to_str().unwrap()])
    .output()
    .expect("run llm-tokei dump");
  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));

  let body = String::from_utf8_lossy(&out.stdout);
  let lines: Vec<&str> = body.lines().collect();
  assert_eq!(lines.len(), 6);
  assert_eq!(lines[0], format!("# {}", fixture.display()));
  let parsed: Vec<serde_json::Value> = lines[1..]
    .iter()
    .map(|line| serde_json::from_str(line).expect("valid jsonl"))
    .collect();
  assert_eq!(
    parsed
      .iter()
      .map(|rec| rec["role"].as_str().unwrap())
      .collect::<Vec<_>>(),
    vec!["system", "user", "assistant", "tool_call", "tool_call_result"]
  );
  assert_eq!(parsed[0]["text"], "system prompt");
  assert_eq!(parsed[1]["text"], "hello cli");
  assert_eq!(parsed[2]["text"], "I'll read it");
  assert_eq!(parsed[3]["text"], "read_file: {\"path\":\"Cargo.toml\"}");
  assert_eq!(parsed[3]["call_id"], "tc1");
  assert_eq!(parsed[4]["text"], "full result");
  assert_eq!(parsed[4]["call_id"], "tc1");
}

#[test]
fn copilot_cli_dump_subcommand_discovers_sessions_and_writes_out_dir() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/copilot_cli_dump/session-state");
  let nanos = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .unwrap()
    .as_nanos();
  let out_dir = std::env::temp_dir().join(format!("llm-tokei-copilot-cli-dump-{nanos}"));
  let _ = std::fs::remove_dir_all(&out_dir);

  let status = Command::new(bin())
    .args([
      "--copilot-cli-dir",
      fixtures.to_str().unwrap(),
      "dump",
      "--copilot-cli",
      "--out",
      out_dir.to_str().unwrap(),
    ])
    .status()
    .expect("run llm-tokei dump");
  assert!(status.success());

  let dest = out_dir.join("cli-dump-session.jsonl");
  let body = std::fs::read_to_string(&dest).expect("dump file written");
  assert_eq!(body.lines().count(), 5);
  assert!(body.contains("hello cli"));

  let _ = std::fs::remove_dir_all(&out_dir);
}

#[test]
fn copilot_dump_subcommand_requires_copilot_flag() {
  let nanos = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .unwrap()
    .as_nanos();
  let out_dir = std::env::temp_dir().join(format!("llm-tokei-dump-bad-{nanos}"));
  let out = Command::new(bin())
    .args(["dump", "--out", out_dir.to_str().unwrap()])
    .output()
    .expect("run llm-tokei dump");
  assert!(!out.status.success());
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert!(stderr.contains("select a source with `--copilot`"), "stderr: {stderr}");
  let _ = std::fs::remove_dir_all(&out_dir);
}

#[test]
fn codex_dump_subcommand_writes_positional_file_to_stdout() {
  let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
    .join("tests/fixtures/codex/sessions/2025/01/02/rollout-2025-01-02T10-00-00-test.jsonl");
  let out = Command::new(bin())
    .args(["dump", "--codex", fixture.to_str().unwrap()])
    .output()
    .expect("run llm-tokei dump");
  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));

  let body = String::from_utf8_lossy(&out.stdout);
  let lines: Vec<&str> = body.lines().collect();
  assert_eq!(lines.len(), 17);
  assert_eq!(lines[0], format!("# {}", fixture.display()));
  let parsed: Vec<serde_json::Value> = lines[1..]
    .iter()
    .map(|line| serde_json::from_str(line).expect("valid jsonl"))
    .collect();

  assert_eq!(
    parsed
      .iter()
      .map(|rec| rec["role"].as_str().unwrap())
      .collect::<Vec<_>>(),
    vec![
      "developer",
      "system",
      "user",
      "assistant",
      "tool_call",
      "tool_call_result",
      "tool_call",
      "tool_call_result",
      "reasoning",
      "user",
      "assistant",
      "user",
      "tool_call",
      "tool_call_result",
      "developer",
      "assistant",
    ]
  );
  assert_eq!(parsed[0]["text"], "dev");
  assert_eq!(parsed[1]["text"], "sys");
  assert_eq!(parsed[2]["text"], "hello");
  assert_eq!(parsed[3]["text"], "ok");
  assert_eq!(parsed[4]["text"], "tool: args");
  assert_eq!(parsed[4]["call_id"], "call_1");
  assert_eq!(parsed[5]["text"], "result");
  assert_eq!(parsed[5]["call_id"], "call_1");
  assert_eq!(parsed[6]["text"], "shell: patch");
  assert_eq!(parsed[6]["call_id"], "call_custom_1");
  assert_eq!(parsed[7]["text"], "tool");
  assert_eq!(parsed[7]["call_id"], "call_custom_1");
  assert_eq!(parsed[8]["encrypted_text"], "think");
  assert_eq!(parsed[8]["text"], "");
  assert_eq!(parsed[9]["text"], "next");
  assert_eq!(parsed[10]["text"], "done");
  assert_eq!(parsed[11]["text"], "more");
  assert_eq!(parsed[12]["text"], "run: {}");
  assert_eq!(parsed[12]["call_id"], "call_2");
  assert_eq!(parsed[13]["text"], "abc");
  assert_eq!(parsed[13]["call_id"], "call_2");
  assert_eq!(parsed[14]["text"], "rules");
  assert_eq!(parsed[15]["text"], "final");
}

#[test]
fn codex_dump_subcommand_discovers_sessions_and_writes_out_dir() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex/sessions");
  let nanos = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .unwrap()
    .as_nanos();
  let out_dir = std::env::temp_dir().join(format!("llm-tokei-codex-dump-{nanos}"));
  let _ = std::fs::remove_dir_all(&out_dir);

  let status = Command::new(bin())
    .args([
      "--codex-dir",
      fixtures.to_str().unwrap(),
      "dump",
      "--codex",
      "--out",
      out_dir.to_str().unwrap(),
    ])
    .status()
    .expect("run llm-tokei dump");
  assert!(status.success());

  let dest = out_dir.join("sess-test-1.jsonl");
  let body = std::fs::read_to_string(&dest).expect("dump file written");
  let parsed: Vec<serde_json::Value> = body
    .lines()
    .map(|line| serde_json::from_str(line).expect("valid jsonl"))
    .collect();
  assert_eq!(parsed.len(), 16);
  assert_eq!(parsed[0]["role"], "developer");
  assert_eq!(parsed[0]["text"], "dev");
  assert_eq!(parsed[1]["role"], "system");
  assert_eq!(parsed[1]["text"], "sys");
  assert_eq!(parsed[8]["role"], "reasoning");
  assert_eq!(parsed[8]["encrypted_text"], "think");
  assert_eq!(parsed[8]["text"], "");
  assert_eq!(parsed[14]["role"], "developer");
  assert_eq!(parsed[14]["text"], "rules");
  assert_eq!(parsed[15]["role"], "assistant");
  assert_eq!(parsed[15]["text"], "final");

  let _ = std::fs::remove_dir_all(&out_dir);
}

#[test]
fn dump_subcommand_rejects_multiple_sources() {
  let out = Command::new(bin())
    .args(["dump", "--copilot", "--codex"])
    .output()
    .expect("run llm-tokei dump");
  assert!(!out.status.success());
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert!(stderr.contains("select only one source"), "stderr: {stderr}");
}

#[test]
fn cli_period_freeform_relative() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex/sessions");
  let (mut cmd, cache_home) = isolated_cmd("period-freeform");
  let out = cmd
    .args([
      "--format",
      "json",
      "--source",
      "codex",
      "--period",
      "3d",
      "--codex-dir",
      fixtures.to_str().unwrap(),
      "--opencode-db",
      "/nonexistent/opencode.db",
    ])
    .output()
    .expect("run period freeform");
  let _ = std::fs::remove_dir_all(cache_home);
  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn cli_period_freeform_calendar_today() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex/sessions");
  let (mut cmd, cache_home) = isolated_cmd("period-today");
  let out = cmd
    .args([
      "--format",
      "json",
      "--source",
      "codex",
      "--period",
      "today",
      "--codex-dir",
      fixtures.to_str().unwrap(),
      "--opencode-db",
      "/nonexistent/opencode.db",
    ])
    .output()
    .expect("run period today");
  let _ = std::fs::remove_dir_all(cache_home);
  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn cli_period_freeform_absolute_date() {
  let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex/sessions");
  let (mut cmd, cache_home) = isolated_cmd("period-absolute");
  let out = cmd
    .args([
      "--format",
      "json",
      "--source",
      "codex",
      "--period",
      "2020-01-01",
      "--codex-dir",
      fixtures.to_str().unwrap(),
      "--opencode-db",
      "/nonexistent/opencode.db",
    ])
    .output()
    .expect("run period absolute");
  let _ = std::fs::remove_dir_all(cache_home);
  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn cli_period_freeform_invalid_rejected() {
  let (mut cmd, cache_home) = isolated_cmd("period-invalid");
  let out = cmd.args(["--period", "foobar"]).output().expect("run period invalid");
  let _ = std::fs::remove_dir_all(cache_home);
  assert!(!out.status.success());
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert!(stderr.contains("parsing --period"), "stderr: {stderr}");
}
