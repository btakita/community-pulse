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
