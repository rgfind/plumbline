//! Black-box tests for selected configuration inspection and guarded writes.

use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn temp_tree(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "plumbline-config-command-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

fn document(branch: &str) -> String {
    format!(
        r#"{{"config_version":1,"release":{{"branch":"{branch}","remote":"origin","verification":{{"ci":[{{"id":"format","argv":["cargo","fmt","--check"],"timeout_seconds":120}}],"release":[{{"id":"publish","argv":["cargo","publish","--dry-run"],"timeout_seconds":600}}],"github_actions":{{"repository":"owner/repo","workflow_path":".github/workflows/ci.yml"}}}}}},"surfaces":[]}}"#
    )
}

fn write_config(dir: &Path, name: &str, branch: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, document(branch)).unwrap();
    path
}

fn command(dir: &Path, arguments: &[&str]) -> Command {
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_plumb"))
        .canonicalize()
        .unwrap();
    let mut command = Command::new(binary);
    command
        .args(arguments)
        .current_dir(dir)
        .env_remove("PLUMB_CONFIG");
    command
}

fn run(dir: &Path, arguments: &[&str]) -> Output {
    command(dir, arguments).output().unwrap()
}

fn json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap()
}

fn config_hash(dir: &Path) -> String {
    json(&run(dir, &["config", "show", "--json"]))["data"]["config_hash"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn selection_uses_explicit_then_environment_then_default() {
    let dir = temp_tree("selection");
    let environment = write_config(&dir, "environment.json", "environment");
    let explicit = write_config(&dir, "explicit.json", "explicit");
    write_config(&dir, "plumbline.json", "default");

    let default = json(&run(&dir, &["config", "get", "/release/branch", "--json"]));
    assert_eq!(default["data"]["selection_source"], "default");
    assert_eq!(default["data"]["data"]["value"], "default");

    let environment_output = command(&dir, &["config", "get", "/release/branch", "--json"])
        .env("PLUMB_CONFIG", &environment)
        .output()
        .unwrap();
    let environment_json = json(&environment_output);
    assert_eq!(environment_json["data"]["selection_source"], "environment");
    assert_eq!(environment_json["data"]["data"]["value"], "environment");

    let explicit_output = command(
        &dir,
        &[
            "--config",
            explicit.to_str().unwrap(),
            "config",
            "get",
            "/release/branch",
            "--json",
        ],
    )
    .env("PLUMB_CONFIG", &environment)
    .output()
    .unwrap();
    let explicit_json = json(&explicit_output);
    assert_eq!(explicit_json["data"]["selection_source"], "command_line");
    assert_eq!(explicit_json["data"]["data"]["value"], "explicit");
}

#[test]
fn get_set_patch_and_hash_conflicts_are_guarded() {
    let dir = temp_tree("writes");
    let config = write_config(&dir, "plumbline.json", "main");
    let first_hash = config_hash(&dir);

    let refusal = json(&run(
        &dir,
        &["config", "set", "/release/branch", "\"next\"", "--json"],
    ));
    assert_eq!(refusal["errors"][0]["code"], "MISSING_REQUIRED");

    let changed = run(
        &dir,
        &[
            "config",
            "set",
            "/release/branch",
            "\"next\"",
            "--yes",
            "--if-match",
            &first_hash,
            "--json",
        ],
    );
    assert!(changed.status.success());
    let changed_json = json(&changed);
    assert_eq!(changed_json["data"]["data"]["already_complete"], false);
    assert_eq!(
        serde_json::from_str::<Value>(&fs::read_to_string(&config).unwrap()).unwrap()["release"]
            ["branch"],
        "next"
    );
    assert!(fs::read_to_string(&config).unwrap().ends_with('\n'));

    let current_hash = config_hash(&dir);
    let no_op = json(&run(
        &dir,
        &[
            "config",
            "set",
            "/release/branch",
            "\"next\"",
            "--yes",
            "--if-match",
            &current_hash,
            "--json",
        ],
    ));
    assert_eq!(no_op["data"]["data"]["already_complete"], true);

    let stale = json(&run(
        &dir,
        &[
            "config",
            "set",
            "/release/branch",
            "\"other\"",
            "--yes",
            "--if-match",
            &first_hash,
            "--json",
        ],
    ));
    assert_eq!(stale["errors"][0]["code"], "CONFIG_WRITE_CONFLICT");

    let before_patch = config_hash(&dir);
    let mut patch = command(
        &dir,
        &[
            "config",
            "patch",
            "--from-stdin",
            "--yes",
            "--if-match",
            &before_patch,
            "--json",
        ],
    );
    patch.stdin(Stdio::piped());
    patch.stdout(Stdio::piped());
    patch.stderr(Stdio::piped());
    let mut child = patch.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"release":{"branch":"patched"}}"#)
        .unwrap();
    let patched = child.wait_with_output().unwrap();
    assert!(patched.status.success());
    assert_eq!(json(&patched)["data"]["data"]["already_complete"], false);

    let missing = json(&run(&dir, &["config", "get", "/missing", "--json"]));
    assert_eq!(missing["errors"][0]["code"], "NOT_FOUND");
    let invalid = json(&run(&dir, &["config", "get", "missing", "--json"]));
    assert_eq!(invalid["errors"][0]["code"], "INVALID_INPUT");
}

#[test]
fn writer_lock_and_schema_validation_fail_without_writing() {
    let dir = temp_tree("lock");
    let config = write_config(&dir, "plumbline.json", "main");
    let hash = config_hash(&dir);
    fs::write(dir.join(".plumbline.json.plumb.lock"), "held\n").unwrap();
    let locked = json(&run(
        &dir,
        &[
            "config",
            "set",
            "/release/branch",
            "\"next\"",
            "--yes",
            "--if-match",
            &hash,
            "--json",
        ],
    ));
    assert_eq!(locked["errors"][0]["code"], "LOCKED");
    fs::remove_file(dir.join(".plumbline.json.plumb.lock")).unwrap();

    let hash = config_hash(&dir);
    let invalid = json(&run(
        &dir,
        &[
            "config",
            "set",
            "/unknown",
            "true",
            "--yes",
            "--if-match",
            &hash,
            "--json",
        ],
    ));
    assert_eq!(invalid["errors"][0]["code"], "CONFIG_SCHEMA");
    assert!(!fs::read_to_string(&config).unwrap().contains("unknown"));
}
