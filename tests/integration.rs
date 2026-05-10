use std::process::Command;

fn temp_file_path(name: &str) -> std::path::PathBuf {
  let nanos = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .expect("system time")
    .as_nanos();
  std::env::temp_dir().join(format!("llm-tokei-{name}-{nanos}.json"))
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
  let out = Command::new(bin())
    .args([
      "--source",
      "codex",
      "--codex-dir",
      fixtures.to_str().unwrap(),
      "--opencode-db",
      "/nonexistent/opencode.db",
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
  let out = Command::new(bin())
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
  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let s = String::from_utf8_lossy(&out.stdout);
  let v: serde_json::Value = serde_json::from_str(&s).expect("valid json");
  let arr = v.as_array().unwrap();
  assert_eq!(arr.len(), 1);
  let row = &arr[0];
  assert_eq!(row["input"], 34);
  assert_eq!(row["output"], 34);
  assert_eq!(row["reasoning"], 50);
  assert_eq!(row["total"], 770);
  assert_eq!(row["turns"], 4);
  assert_eq!(row["rounds"], 2);
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
    ])
    .output()
    .expect("run llm-tokei table bytes mode");
  assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
  let table = String::from_utf8_lossy(&out.stdout);
  assert!(table.contains("input(B)"), "table output: {table}");
  assert!(table.contains("output(B)"), "table output: {table}");
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
fn copilot_dump_fixture_bytes_cover_schema_variants() {
  // Exercises schema-driven bytes paths: message.text fallback,
  // progressTaskSerialized.content.value, toolInvocationSerialized
  // invocationMessage as string and as { value }, and thinking value
  // (which must NOT count as output bytes).
  let fixtures =
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/copilot_dump/workspaceStorage");
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
  //   r3 renderedUserMessage "tool call please" = 16
  //   r4 renderedUserMessage "think" = 5
  // Total = 49.
  assert_eq!(row["input"], 49);
  // Outputs:
  //   r1 text "ack" = 3
  //   r2 progressTaskSerialized.content.value "progress text" (13) + tool args "{}" (2) = 15
  //   r3 invocationMessage "Reading files" (13) + pastTenseMessage.value "Read files" (10) = 23
  //   r4 thinking "secret reasoning" → 0 (must not leak into output) + text "done" (4) = 4
  // Total = 45.
  assert_eq!(row["output"], 45);
  // reasoning comes from toolCallRounds.thinking.tokens (exact, not bytes).
  assert_eq!(row["reasoning"], 7);
}

#[test]
fn copilot_dump_subcommand_writes_role_user_jsonl() {
  let fixtures =
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/copilot_dump/workspaceStorage");
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
      "--out",
      out_dir.to_str().unwrap(),
    ])
    .status()
    .expect("run llm-tokei dump");
  assert!(status.success());

  let dest = out_dir.join("sess-dump-1.jsonl");
  let body = std::fs::read_to_string(&dest).expect("dump file written");
  let lines: Vec<&str> = body.lines().collect();
  assert_eq!(lines.len(), 5);
  let parsed: Vec<serde_json::Value> = lines
    .iter()
    .map(|l| serde_json::from_str(l).expect("valid jsonl"))
    .collect();

  // Every record uses role:"user".
  for rec in &parsed {
    assert_eq!(rec["role"], "user");
  }
  // Order: prompt, prompt, tool result (with call_id), prompt, prompt.
  assert_eq!(parsed[0]["text"], "fallback prompt");
  assert!(parsed[0].get("call_id").is_none());
  assert_eq!(parsed[1]["text"], "hi");
  assert_eq!(parsed[2]["text"], "result body");
  assert_eq!(parsed[2]["call_id"], "c1");
  assert_eq!(parsed[3]["text"], "tool call please");
  assert_eq!(parsed[4]["text"], "think");

  let _ = std::fs::remove_dir_all(&out_dir);
}

#[test]
fn copilot_dump_subcommand_rejects_unsupported_source() {
  let nanos = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .unwrap()
    .as_nanos();
  let out_dir = std::env::temp_dir().join(format!("llm-tokei-dump-bad-{nanos}"));
  let out = Command::new(bin())
    .args([
      "dump",
      "--out",
      out_dir.to_str().unwrap(),
      "--source",
      "codex",
    ])
    .output()
    .expect("run llm-tokei dump");
  assert!(!out.status.success());
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert!(stderr.contains("only `copilot`"), "stderr: {stderr}");
  let _ = std::fs::remove_dir_all(&out_dir);
}
