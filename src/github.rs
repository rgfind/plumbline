//! Exact-commit GitHub Actions evidence, behind a small adapter seam.

use crate::config::GitHubActions;
use serde_json::{json, Value};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

pub trait GitHubAdapter {
    fn prove(&mut self, proof: &GitHubActions, branch: &str, head_sha: &str) -> Value;
}

pub struct GhAdapter;

impl GitHubAdapter for GhAdapter {
    fn prove(&mut self, proof: &GitHubActions, branch: &str, head_sha: &str) -> Value {
        let workflows = match request(&format!("repos/{}/actions/workflows", proof.repository)) {
            Ok(value) => value,
            Err(code) => return failure(proof, head_sha, code, None),
        };
        let Some(workflow_id) = workflows["workflows"].as_array().and_then(|items| {
            items
                .iter()
                .find(|item| item["path"] == proof.workflow_path)
                .and_then(|item| item["id"].as_u64())
        }) else {
            return failure(proof, head_sha, "CI_PROOF_UNAVAILABLE", None);
        };
        let endpoint = format!(
            "repos/{}/actions/workflows/{workflow_id}/runs?event=push&branch={branch}&per_page=100",
            proof.repository
        );
        let runs = match request(&endpoint) {
            Ok(value) => value,
            Err(code) => return failure(proof, head_sha, code, None),
        };
        let empty = Vec::new();
        evaluate_runs(
            proof,
            head_sha,
            runs["workflow_runs"].as_array().unwrap_or(&empty),
        )
    }
}

fn request(endpoint: &str) -> Result<Value, &'static str> {
    // Retry only failures that can safely be temporary. No request text is a
    // shell fragment; each value is one argv element to `gh`.
    for attempt in 0..3 {
        match run_gh(endpoint) {
            Ok(text) => return serde_json::from_str(&text).map_err(|_| "CI_PROOF_UNAVAILABLE"),
            Err("CI_PROOF_TRANSIENT_FAILURE") if attempt < 2 => {
                std::thread::sleep(Duration::from_secs(if attempt == 0 { 1 } else { 2 }));
            }
            Err(code) => return Err(code),
        }
    }
    Err("CI_PROOF_TRANSIENT_FAILURE")
}

fn run_gh(endpoint: &str) -> Result<String, &'static str> {
    let output = Command::new("gh")
        .args(["api", endpoint])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(|_| "GITHUB_CLI_UNAVAILABLE")?;
    if !output.status.success() {
        return Err("CI_PROOF_UNAVAILABLE");
    }
    String::from_utf8(output.stdout).map_err(|_| "CI_PROOF_UNAVAILABLE")
}

pub fn head_sha(root: &Path) -> Result<String, ()> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()
        .map_err(|_| ())?;
    if !output.status.success() {
        return Err(());
    }
    String::from_utf8(output.stdout)
        .map(|text| text.trim().to_string())
        .map_err(|_| ())
}

pub fn evaluate_runs(proof: &GitHubActions, head_sha: &str, runs: &[Value]) -> Value {
    let run = runs
        .iter()
        .filter(|run| run["head_sha"] == head_sha)
        .max_by_key(|run| {
            run["run_attempt"].as_u64().unwrap_or(0) * 1_000_000_000
                + run["id"].as_u64().unwrap_or(0)
        });
    match run {
        None => failure(proof, head_sha, "CI_PROOF_MISSING", None),
        Some(run) if run["status"] != "completed" => {
            failure(proof, head_sha, "CI_PROOF_PENDING", Some(run))
        }
        Some(run) if run["conclusion"] != "success" => {
            failure(proof, head_sha, "CI_PROOF_FAILED", Some(run))
        }
        Some(run) => json!({
            "id": "github-actions-proof", "kind": "github-actions", "status": "passed",
            "repository": proof.repository, "workflow_path": proof.workflow_path, "head_sha": head_sha,
            "run_id": run["id"], "run_url": run["html_url"], "conclusion": "success",
        }),
    }
}

fn failure(proof: &GitHubActions, head_sha: &str, code: &str, run: Option<&Value>) -> Value {
    let command = match code {
        "CI_PROOF_PENDING" | "CI_PROOF_FAILED" => run
            .and_then(|item| item["html_url"].as_str())
            .unwrap_or("gh run list"),
        "CI_PROOF_MISSING" => "git push origin HEAD",
        "GITHUB_CLI_UNAVAILABLE" | "CI_PROOF_UNAVAILABLE" => "gh auth status",
        _ => "plumb preflight --wait --json",
    };
    json!({
        "id": "github-actions-proof", "kind": "github-actions", "status": "failed",
        "repository": proof.repository, "workflow_path": proof.workflow_path, "head_sha": head_sha,
        "diagnostic_code": code,
        "run_id": run.and_then(|item| item.get("id")).cloned().unwrap_or(Value::Null),
        "run_url": run.and_then(|item| item.get("html_url")).cloned().unwrap_or(Value::Null),
        "recommended_action": {"command": command, "rationale": "the configured exact-commit CI proof did not pass", "alternatives": []},
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proof() -> GitHubActions {
        GitHubActions {
            repository: "owner/repo".into(),
            workflow_path: ".github/workflows/ci.yml".into(),
        }
    }

    #[test]
    fn exact_commit_success_is_required() {
        let wrong = json!({"id":1,"head_sha":"other","status":"completed","conclusion":"success"});
        assert_eq!(
            evaluate_runs(&proof(), "head", &[wrong])["diagnostic_code"],
            "CI_PROOF_MISSING"
        );
        let pending = json!({"id":2,"head_sha":"head","status":"in_progress","html_url":"https://example.test/run"});
        assert_eq!(
            evaluate_runs(&proof(), "head", &[pending])["diagnostic_code"],
            "CI_PROOF_PENDING"
        );
        let success = json!({"id":3,"head_sha":"head","status":"completed","conclusion":"success","html_url":"https://example.test/run"});
        assert_eq!(
            evaluate_runs(&proof(), "head", &[success])["status"],
            "passed"
        );
    }
}
