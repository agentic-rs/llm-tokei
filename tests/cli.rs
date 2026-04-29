use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn renders_json_summary_for_opencode_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    std::fs::write(
        path,
        r#"{"sessionID":"s1","model":"anthropic/claude-sonnet-4","usage":{"inputTokens":11,"outputTokens":22,"cacheCreationTokens":3,"cacheReadTokens":4}}"#,
    )
    .unwrap();

    let mut cmd = Command::cargo_bin("llm-tokei").unwrap();
    cmd.args([
        "--provider",
        "open-code",
        "--path",
        dir.path().to_str().unwrap(),
        "--format",
        "json",
    ]);

    cmd.assert()
        .success()
        .stdout(predicate::str::contains(r#""provider": "opencode""#))
        .stdout(predicate::str::contains(r#""total_tokens": 40"#));
}
