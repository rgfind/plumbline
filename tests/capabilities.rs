//! Contract test for `plumb capabilities`.
//!
//! The verb emits plumbline's own contract as JSON. These tests pin the promised
//! shape — the exact envelope paths a future self-contract claims against (see
//! CONTRACT.md "Self-claims") — so a change that would silently break those
//! claims fails here first. They assert structure and the stable facts, not the
//! prose `meaning` strings (which may be reworded).

use serde_json::Value;
use std::process::Command;

fn capabilities() -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_plumb"))
        .arg("capabilities")
        .output()
        .expect("run plumb capabilities");
    assert!(
        out.status.success(),
        "capabilities must exit 0, got {:?}:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("capabilities stdout is valid JSON")
}

#[test]
fn envelope_is_ok_with_one_data_row() {
    let env = capabilities();
    assert_eq!(env["ok"], Value::Bool(true));
    assert_eq!(env["data"].as_array().map(|a| a.len()), Some(1));
    // meta is present and volatile: a request_id string the normalize step drops.
    assert!(env["meta"]["request_id"].is_string());
}

#[test]
fn commands_and_verbs_agree() {
    let env = capabilities();
    let commands: Vec<&str> = env["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(commands, ["check", "capture", "preflight", "capabilities"]);

    // Every top-level command has a detailed entry in data[0].verbs.
    let verbs = env["data"][0]["verbs"].as_object().unwrap();
    for c in &commands {
        assert!(verbs.contains_key(*c), "verbs is missing `{c}`");
    }
    // capture is the one verb that needs a contract.
    assert_eq!(verbs["capture"]["needs_contract"], Value::Bool(true));
    assert_eq!(verbs["check"]["needs_contract"], Value::Bool(false));
}

#[test]
fn pins_the_self_claim_paths() {
    let env = capabilities();
    let data = &env["data"][0];

    assert_eq!(data["contract_version"], "1");

    // exit_codes keyed by the three exit values.
    let exit_codes = data["exit_codes"].as_object().unwrap();
    for k in ["0", "1", "2"] {
        assert!(exit_codes.contains_key(k), "exit_codes is missing `{k}`");
    }

    // STRAY_BLOCK is a GATE fault that exits 1: the per-code value claims in
    // CONTRACT.md pin exactly this.
    let stray = &data["error_codes"]["STRAY_BLOCK"];
    assert_eq!(stray["family"], "GATE");
    assert_eq!(stray["exit"], 1);

    // USAGE is the only family that exits 2.
    assert_eq!(data["error_codes"]["USAGE"]["exit"], 2);

    // No warnings surface yet.
    assert_eq!(data["warning_codes"], serde_json::json!([]));
}

#[test]
fn version_is_available_without_a_project_config() {
    let output = Command::new(env!("CARGO_BIN_EXE_plumb"))
        .arg("--version")
        .current_dir(std::env::temp_dir())
        .output()
        .expect("run plumb --version");

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).expect("UTF-8 version output"),
        format!("plumb {}\n", env!("CARGO_PKG_VERSION"))
    );

    let envelope = capabilities();
    let global_flags = envelope["data"][0]["global_flags"]
        .as_array()
        .expect("global_flags array");
    assert!(global_flags.iter().any(|flag| flag["name"] == "--version"));
}
