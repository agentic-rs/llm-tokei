use std::process::Command;

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
  // `input` is the full prompt total (cached + uncached).
  assert_eq!(row["input"], 500);
  assert_eq!(row["output"], 220);
  assert_eq!(row["reasoning"], 50);
  assert_eq!(row["cache_read"], 200);
  // total = input + output + reasoning + cache_write (cache_read is already in input).
  assert_eq!(row["total"], 770);
  assert_eq!(row["turns"], 4);
  assert_eq!(row["rounds"], 2);
  assert_eq!(row["sessions"], 1);
  assert_eq!(row["keys"]["model"], "gpt-5");
  assert_eq!(row["keys"]["source"], "codex");
  // gpt-5 base price: input 1.25 + output 10 + cache_read 0.125 (per 1M).
  // Billing uses uncached_input = 500 - 200 = 300.
  // 300*1.25 + 220*10 + 50*10 (reasoning falls back to output) + 200*0.125
  //   = 375 + 2200 + 500 + 25 = 3100 → / 1e6 = 0.003100
  let base = row["cost_base"].as_f64().unwrap();
  assert!((base - 0.003100).abs() < 1e-9, "got {base}");
  // openai provider has no multiplier override → defaults to 1.0.
  let mult = row["cost_multiplied"].as_f64().unwrap();
  assert!((mult - base).abs() < 1e-9);
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
  // input  = 50+100+30 + 10+150+7 = 347
  // output = 40+20 = 60
  // cache_read  = 250
  // cache_write = 37
  // total = input + output + reasoning + cache_write = 347+60+0+37 = 444
  assert_eq!(row["input"], 347);
  assert_eq!(row["output"], 60);
  assert_eq!(row["reasoning"], 0);
  assert_eq!(row["cache_read"], 250);
  assert_eq!(row["cache_write"], 37);
  assert_eq!(row["total"], 444);
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
  assert_eq!(arr.len(), 1);
  let row = &arr[0];
  // Per-turn (per-request) ceil(chars/4) — request 1 input chars 31+18=49 → 13;
  //   request 2 input chars 7 → 2; total 15.
  // Output per request: 32+2+12=46 → 12; 5 → 2; total 14.
  // reasoning = 17 (exact, from thinking.tokens)
  assert_eq!(row["input"], 15);
  assert_eq!(row["output"], 14);
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
