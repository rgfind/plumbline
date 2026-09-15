//! Input grammar tests for the release command.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn config_file(name: &str, text: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "plumbline-release-cli-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("plumbline.json");
    fs::write(&path, text).unwrap();
    path
}

fn release_with_config(path: &PathBuf, config_after_verb: bool) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_plumb"));
    if config_after_verb {
        command.args(["release", "--config", path.to_str().unwrap()]);
    } else {
        command.args(["--config", path.to_str().unwrap(), "release"]);
    }
    command.output().expect("run plumb release")
}

#[test]
fn release_accepts_config_before_or_after_the_verb() {
    let path = config_file("missing", r#"{"surfaces":[]}"#);
    for config_after_verb in [false, true] {
        let output = release_with_config(&path, config_after_verb);
        assert_eq!(output.status.code(), Some(3));
        assert!(String::from_utf8_lossy(&output.stderr).contains("RELEASE_CONFIG_MISSING"));
    }
}

#[test]
fn release_rejects_nonobject_and_empty_release_settings() {
    for (name, config) in [
        ("scalar", r#"{"release":"origin"}"#),
        ("empty", r#"{"release":{"branch":"","remote":"origin"}}"#),
    ] {
        let path = config_file(name, config);
        let output = release_with_config(&path, false);
        assert_eq!(output.status.code(), Some(3));
        assert!(String::from_utf8_lossy(&output.stderr).contains("CONFIG_SCHEMA"));
    }
}
