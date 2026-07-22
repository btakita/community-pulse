use serde_json::Value;
use std::process::Command;

#[test]
fn fixture_cli_prints_a_machine_readable_capped_digest() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("pulse.db");
    let output = Command::new(env!("CARGO_BIN_EXE_pulse"))
        .args([
            "--database",
            database.to_str().unwrap(),
            "--fixture",
            "top",
            "--json",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value.as_array().unwrap().len(), 5);
}

#[test]
fn app_help_exposes_the_phone_frame_modes() {
    let output = Command::new(env!("CARGO_BIN_EXE_pulse"))
        .args(["app", "--help"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("--mobile"));
    assert!(help.contains("--companion"));
    assert!(help.contains("--mcp-port"));
    assert!(help.contains("--no-mcp"));
    assert!(help.contains("--agent-terminal"));
    assert!(help.contains("--live"));
    assert!(help.contains("--ingest-interval"));
}

#[test]
fn setup_help_exposes_both_agents_and_the_default_endpoint_port() {
    let output = Command::new(env!("CARGO_BIN_EXE_pulse"))
        .args(["setup", "--help"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("claude"));
    assert!(help.contains("codex"));
    assert!(help.contains("--mcp-port"));
}

#[test]
fn snapshot_command_produces_a_reusable_database() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("pulse.db");
    let snapshot = directory.path().join("fallback.db");
    let output = Command::new(env!("CARGO_BIN_EXE_pulse"))
        .args([
            "--database",
            source.to_str().unwrap(),
            "--fixture",
            "snapshot",
            snapshot.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let restored = Command::new(env!("CARGO_BIN_EXE_pulse"))
        .args(["--database", snapshot.to_str().unwrap(), "top", "--json"])
        .output()
        .unwrap();
    assert!(restored.status.success());
    let value: Value = serde_json::from_slice(&restored.stdout).unwrap();
    assert_eq!(value.as_array().unwrap().len(), 5);
}
