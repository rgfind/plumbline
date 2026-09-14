//! Seeded-drift acceptance tests for the guard.
//!
//! Each test lays down a self-contained temp tree (a plumbline.json config, a
//! synthetic contract fixture, and empty doc surfaces), runs `plumb check` with
//! that tree as the working directory, and asserts the guard passes on a
//! reconciled tree and fails — naming the drifted claim — on a seeded drift.
//!
//! The fixture is synthetic on purpose: these test plumbline's engine (the claim
//! checker and the stray-block scan), not any one project's data. `check` never
//! runs a target binary, so no build is needed here; the capture/render paths are
//! covered by the unit tests in `engine`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A synthetic contract envelope shaped like a real one: nested verbs, an
/// object of exit codes, and an array of warning codes, so value/keys/set modes
/// all have something to bite on.
fn fixture_json() -> serde_json::Value {
    serde_json::json!({
        "tool_version": "9.9.9",
        "data": [{
            "contract_version": "2",
            "exit_codes": {"0": {}, "1": {}, "5": {"retryable": true}},
            "warning_codes": ["ALPHA", "BETA", "GAMMA"],
            "verbs": {"find": {"stage": "enum[a,b,c]"}}
        }]
    })
}

/// The config that pins the fixture. One claim per mode (value/keys/set), plus a
/// declared generated block so the known-id case is exercised. `capture` is
/// present but never used by `check`; it is required by the loader.
fn config_json() -> serde_json::Value {
    serde_json::json!({
        "capture": {"build": ["cargo", "build"], "command": ["dummy", "--json"]},
        "fixture": "fixture.json",
        "normalize_meta": ["request_id"],
        "surfaces": ["README.md", "CHANGELOG.md"],
        "generated": [{
            "id": "readme-example", "surface": "README.md",
            "render": ["dummy"], "tree": {"files": {}}
        }],
        "claims": [
            {"id": "contract_version", "fixture_path": "data.0.contract_version",
             "mode": "value", "expected": "2"},
            {"id": "stage_enum", "fixture_path": "data.0.verbs.find.stage",
             "mode": "value", "expected": "enum[a,b,c]"},
            {"id": "exit_code_set", "fixture_path": "data.0.exit_codes",
             "mode": "keys", "expected": ["0", "1", "5"]},
            {"id": "warning_code_set", "fixture_path": "data.0.warning_codes",
             "mode": "set", "expected": ["ALPHA", "BETA", "GAMMA"]}
        ]
    })
}

/// Lay down a self-contained tree the guard can check: config + fixture at the
/// paths the config names, plus prose-only surfaces with no generated blocks.
fn scaffold(dir: &Path) {
    fs::write(
        dir.join("plumbline.json"),
        serde_json::to_string_pretty(&config_json()).unwrap(),
    )
    .unwrap();
    fs::write(
        dir.join("fixture.json"),
        serde_json::to_string_pretty(&fixture_json()).unwrap(),
    )
    .unwrap();
    fs::write(dir.join("README.md"), "# demo\n\nprose only, no tables.\n").unwrap();
    fs::write(dir.join("CHANGELOG.md"), "# changelog\n").unwrap();
}

/// Run `plumb check` with `root` as the working directory (so the config's
/// relative fixture path resolves there), using the default `./plumbline.json`.
fn run_check(root: &Path) -> Output {
    // Cargo can provide a relative test-binary path. Make it absolute before
    // changing into the synthetic project tree.
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_plumb"))
        .canonicalize()
        .expect("resolve plumb test binary");
    Command::new(binary)
        .arg("check")
        .current_dir(root)
        .output()
        .expect("run plumb check")
}

/// A unique temp dir (no external tempfile dependency).
fn temp_tree(tag: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!(
        "plumbline-drift-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&base).unwrap();
    base
}

fn mutate_fixture(root: &Path, f: impl FnOnce(&mut serde_json::Value)) {
    let p = root.join("fixture.json");
    let mut v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
    f(&mut v);
    fs::write(&p, serde_json::to_string_pretty(&v).unwrap()).unwrap();
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn passes_on_reconciled_tree() {
    let dir = temp_tree("clean");
    scaffold(&dir);
    let out = run_check(&dir);
    assert!(
        out.status.success(),
        "expected pass on reconciled tree, got:\n{}",
        stderr(&out)
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn fails_on_drifted_value() {
    let dir = temp_tree("value");
    scaffold(&dir);
    // Shrink the find.stage enum -> the value-mode claim must no longer match.
    mutate_fixture(&dir, |v| {
        v["data"][0]["verbs"]["find"]["stage"] = serde_json::json!("enum[a,b]");
    });
    let out = run_check(&dir);
    assert!(!out.status.success(), "expected failure on drifted value");
    assert!(
        stderr(&out).contains("stage_enum"),
        "diagnostic names the claim: {}",
        stderr(&out)
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn fails_on_dropped_key() {
    let dir = temp_tree("keys");
    scaffold(&dir);
    // Drop an exit code -> the keys-mode claim must fail.
    mutate_fixture(&dir, |v| {
        v["data"][0]["exit_codes"]
            .as_object_mut()
            .unwrap()
            .remove("5");
    });
    let out = run_check(&dir);
    assert!(!out.status.success(), "expected failure on dropped key");
    assert!(
        stderr(&out).contains("exit_code_set"),
        "diagnostic names the claim: {}",
        stderr(&out)
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn fails_on_changed_set() {
    let dir = temp_tree("set");
    scaffold(&dir);
    // Add an undocumented warning code -> the set-mode claim must fail.
    mutate_fixture(&dir, |v| {
        v["data"][0]["warning_codes"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!("DELTA_UNDOCUMENTED"));
    });
    let out = run_check(&dir);
    assert!(!out.status.success(), "expected failure on changed set");
    assert!(
        stderr(&out).contains("warning_code_set"),
        "diagnostic names the claim: {}",
        stderr(&out)
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn passes_with_a_known_generated_block() {
    let dir = temp_tree("known");
    scaffold(&dir);
    // A block whose id is declared in the config is known; check must not object.
    fs::write(
        dir.join("README.md"),
        "# demo\n\n<!-- BEGIN GENERATED:readme-example -->\nx\n<!-- END GENERATED:readme-example -->\n",
    )
    .unwrap();
    let out = run_check(&dir);
    assert!(
        out.status.success(),
        "a declared block id must be accepted, got:\n{}",
        stderr(&out)
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn fails_on_stray_generated_block() {
    let dir = temp_tree("stray");
    scaffold(&dir);
    // A block whose id has no renderer in the config would go stale silently and
    // must be rejected.
    fs::write(
        dir.join("README.md"),
        "# demo\n\n<!-- BEGIN GENERATED:exit-codes-table -->\nstale\n<!-- END GENERATED:exit-codes-table -->\n",
    )
    .unwrap();
    let out = run_check(&dir);
    assert!(!out.status.success(), "expected failure on stray block");
    assert!(
        stderr(&out).contains("exit-codes-table"),
        "diagnostic names the stray id: {}",
        stderr(&out)
    );
    fs::remove_dir_all(&dir).ok();
}
