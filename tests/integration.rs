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

fn bin() -> std::path::PathBuf {
  let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
  p.push("target");
  p.push(if cfg!(debug_assertions) { "debug" } else { "release" });
  p.push("llm-tokei");
  p
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
  // total = input + output + reasoning.
  assert_eq!(row["total"], 770);
  assert_eq!(row["turns"], 4);
  assert_eq!(row["rounds"], 2);
  assert_eq!(row["sessions"], 1);
  assert_eq!(row["keys"]["model"], "gpt-5");
  assert_eq!(row["keys"]["source"], "codex");
  // gpt-5 base price: input 1.25 + output 10 + cache_read 0.125 (per 1M).
  // Billing uses uncached_input = 300.
  // 300*1.25 + 220*10 + 50*10 (reasoning falls back to output) + 200*0.125
  //   = 375 + 2200 + 500 + 25 = 3100 → / 1e6 = 0.003100
  let base = row["cost_base"].as_f64().unwrap();
  assert!((base - 0.003100).abs() < 1e-9, "got {base}");
  // openai provider has no multiplier override → defaults to 1.0.
  let mult = row["cost_multiplied"].as_f64().unwrap();
  assert!((mult - base).abs() < 1e-9);
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
  assert_eq!(row["total"], 770);
  assert_eq!(row["turns"], 4);
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
            input         INTEGER NOT NULL,
            output        INTEGER NOT NULL,
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
            turns         INTEGER NOT NULL,
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
  // Two assistant turns:
  //   #1: input=50, output=40, cache_read=100, cache_write=30
  //   #2: input=10, output=20, cache_read=150, cache_write=5+2=7
  // displayed input = (50+100+30) + (10+150+7) = 347
  // output = 40+20 = 60
  // cache_read  = 250
  // cache_write = 37
  // total = input + output + reasoning = 347+60+0 = 407
  assert_eq!(row["input"], 347);
  assert_eq!(row["output"], 60);
  assert_eq!(row["reasoning"], 0);
  assert_eq!(row["cache_read"], 250);
  assert_eq!(row["cache_write"], 37);
  assert_eq!(row["total"], 407);
  assert_eq!(row["turns"], 2);
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
  assert_eq!(row["output"], 13);
  assert_eq!(row["reasoning"], 17);
  assert_eq!(row["cache_read"], 0);
  assert_eq!(row["cache_write"], 0);
  assert_eq!(row["turns"], 3);
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
  assert_eq!(row["input"], 17);
  assert_eq!(row["output"], 20);
  assert_eq!(row["cache_read"], 3);
  assert_eq!(row["cache_write"], 4);
  assert_eq!(row["total"], 37);
  assert_eq!(row["turns"], 2);
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
  assert_eq!(row["output"], 20);
  assert_eq!(row["cache_read"], 5);
  assert_eq!(row["cache_write"], 2);
  assert_eq!(row["total"], 40);
  assert_eq!(row["turns"], 2);
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
  assert_eq!(token_row["output"], 13);
  assert_eq!(bytes_row["input"], 82);
  assert_eq!(bytes_row["output"], 49);
  assert_eq!(token_row["total"], bytes_row["total"]);
  assert_eq!(token_row["cost_base"], bytes_row["cost_base"]);
  assert_eq!(token_row["cost_multiplied"], bytes_row["cost_multiplied"]);
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
  assert_eq!(row["total"], 770);
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
  ]
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

  let base = row["cost_base"].as_f64().unwrap();
  assert!((base - 0.000015).abs() < 1e-9, "got {base}");
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
      "input": 1.0,
      "output": 0.0,
      "cache_read": 0.0,
      "cache_write": 0.0
    }
  ]
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

  let base = row["cost_base"].as_f64().unwrap();
  assert!((base - 0.000013).abs() < 1e-9, "got {base}");
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
