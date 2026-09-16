//! The workflows are part of the release contract. Keep their policy calls in
//! source control rather than trusting a visual review of the YAML.

use std::fs;

#[test]
fn workflows_call_the_declared_policy_in_order() {
    let ci = fs::read_to_string(".github/workflows/ci.yml").unwrap();
    assert!(ci.contains("cargo install --path . --locked --force"));
    assert!(ci.contains("plumb verify --stage=ci --json"));
    assert!(!ci.contains("run: cargo fmt"));
    assert!(!ci.contains("run: cargo clippy"));
    assert!(!ci.contains("run: cargo test"));

    let release = fs::read_to_string(".github/workflows/release.yml").unwrap();
    let preflight = release.find("plumb preflight").unwrap();
    let publish = release.find("cargo publish --locked").unwrap();
    assert!(preflight < publish);
    assert!(release.contains("actions: read"));
    assert!(release.contains("GH_TOKEN: ${{ github.token }}"));
}
