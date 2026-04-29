use std::process::Command;

fn bin() -> std::path::PathBuf {
    let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("target");
    p.push(if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    });
    p.push("llm-tokei");
    p
}

#[test]
fn codex_fixture_parses_last_total() {
    let fixtures =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex/sessions");
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
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let s = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&s).expect("valid json");
    let arr = v.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    let row = &arr[0];
    // `input` is the full prompt total (cached + uncached).
    assert_eq!(row["input"], 200);
    assert_eq!(row["output"], 90);
    assert_eq!(row["reasoning"], 20);
    assert_eq!(row["cache_read"], 80);
    // total = input + output + reasoning + cache_write (cache_read is already in input).
    assert_eq!(row["total"], 310);
    assert_eq!(row["turns"], 1);
    assert_eq!(row["keys"]["model"], "gpt-5");
    assert_eq!(row["keys"]["source"], "codex");
    // gpt-5 base price: input 1.25 + output 10 + cache_read 0.125 (per 1M).
    // Billing uses uncached_input = 200 - 80 = 120.
    // 120*1.25 + 90*10 + 20*10 (reasoning falls back to output) + 80*0.125
    //   = 150 + 900 + 200 + 10 = 1260 → / 1e6 = 0.00126
    let base = row["cost_base"].as_f64().unwrap();
    assert!((base - 0.00126).abs() < 1e-9, "got {base}");
    // openai provider has no multiplier override → defaults to 1.0.
    let mult = row["cost_multiplied"].as_f64().unwrap();
    assert!((mult - base).abs() < 1e-9);
}
