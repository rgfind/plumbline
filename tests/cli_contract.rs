//! Binary tests for the 0.0.3 Foundation command path.

use serde_json::Value;
use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_plumb"))
        .args(args)
        .output()
        .expect("run plumb")
}

fn json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("stdout has one JSON envelope")
}

#[test]
fn json_success_uses_the_universal_envelope() {
    let output = run(&["capabilities", "--json"]);
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let envelope = json(&output);
    assert_eq!(envelope["ok"], true);
    for key in [
        "ok",
        "tool_version",
        "data",
        "meta",
        "warnings",
        "commands",
        "errors",
    ] {
        assert!(envelope.get(key).is_some(), "missing `{key}`");
    }
    assert_eq!(envelope["meta"]["contract_version"], "0.1");
    assert_eq!(envelope["meta"]["schema_version"], 1);
    assert!(envelope["meta"]["ts_iso"].as_str().unwrap().contains('T'));
}

#[test]
fn json_input_failure_keeps_stdout_parseable_and_mirrors_stderr() {
    let output = run(&["--json", "--not-a-flag"]);
    assert_eq!(output.status.code(), Some(1));
    let envelope = json(&output);
    assert_eq!(envelope["ok"], false);
    assert_eq!(envelope["data"], Value::Null);
    assert_eq!(envelope["errors"][0]["code"], "UNKNOWN_FLAG");
    assert!(String::from_utf8_lossy(&output.stderr).contains("UNKNOWN_FLAG"));
}

#[test]
fn help_and_version_need_no_project_config() {
    let directory = std::env::temp_dir();
    for args in [["--help", "--json"], ["--version", "--json"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_plumb"))
            .args(args)
            .current_dir(&directory)
            .output()
            .expect("run config-free command");
        assert!(output.status.success());
        assert_eq!(json(&output)["ok"], true);
    }
}

#[test]
fn schema_and_robot_docs_are_registry_backed() {
    let schema = run(&["schema", "--command=check", "--json"]);
    assert!(schema.status.success());
    assert!(json(&schema)["data"]["schemas"].get("check").is_some());

    let guide = run(&["robot-docs", "guide", "--json"]);
    assert!(guide.status.success());
    let text = json(&guide)["data"]["guide"].as_str().unwrap().to_string();
    assert!(text.contains("capabilities"));
    assert!(text.contains("robot-docs"));
}

#[test]
fn mutation_modes_refuse_without_the_declared_consent() {
    let capture = run(&["capture", "--json"]);
    assert_eq!(capture.status.code(), Some(1));
    let capture_json = json(&capture);
    assert_eq!(capture_json["errors"][0]["code"], "MISSING_REQUIRED");
    let message = capture_json["errors"][0]["message"].as_str().unwrap();
    assert!(message.contains("capture --check"));
    assert!(message.contains("capture --yes"));

    let selector_conflict = run(&["capture", "--check", "--yes", "--json"]);
    assert_eq!(selector_conflict.status.code(), Some(1));
    assert_eq!(
        json(&selector_conflict)["errors"][0]["code"],
        "INVALID_INPUT"
    );

    let release = run(&["release", "--json"]);
    assert_eq!(release.status.code(), Some(1));
    assert_eq!(json(&release)["errors"][0]["code"], "MISSING_REQUIRED");
    assert!(json(&release)["errors"][0]["message"]
        .as_str()
        .unwrap()
        .contains("--yes"));
}
