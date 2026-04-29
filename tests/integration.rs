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
    let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/codex/sessions");
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
    assert_eq!(row["input"], 200);
    assert_eq!(row["output"], 90);
    assert_eq!(row["reasoning"], 20);
    assert_eq!(row["cache_read"], 80);
    assert_eq!(row["turns"], 1);
    assert_eq!(row["keys"]["model"], "gpt-5");
    assert_eq!(row["keys"]["source"], "codex");
}
