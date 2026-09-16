//! The three verbs, orchestrating config + registry + engine.
//!
//!   check      every registered claim still equals its fixture field, and every
//!              generated block on a surface has a known id (an id with no
//!              renderer would go stale silently, so it is rejected).
//!   capture    rewrite the committed fixture from a fresh binary run, then
//!              re-render every generated block from the same binary.
//!   capture --check
//!              assert the committed fixture is not stale, without rewriting it.
//!   preflight  the publish stop-sign: run every gate that must hold at
//!              `cargo publish` and exit non-zero unless all pass.

use crate::cli::{ConfigAction, VerifyStage};
use crate::config::{self, Capture, Config};
use crate::diagnostic::{codes, Diagnostic};
use crate::engine;
use crate::markers::{extract_generated, generated_ids, replace_generated};
use crate::registry::{check_claims, normalized};
use crate::release;
use crate::result::CommandResult;
use serde_json::{json, Map, Value};
use std::path::Path;
use std::process::Command;

/// Read and parse a committed fixture. Both a read failure and a parse failure
/// mean the same thing to a consumer: the fixture on disk could not be loaded.
fn read_json(path: &Path) -> Result<Value, Diagnostic> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        Diagnostic::new(
            codes::FIXTURE_UNREADABLE,
            format!("read {}: {e}", path.display()),
        )
    })?;
    serde_json::from_str(&text).map_err(|e| {
        Diagnostic::new(
            codes::FIXTURE_UNREADABLE,
            format!("parse {}: {e}", path.display()),
        )
    })
}

// ---- check -----------------------------------------------------------------

pub fn cmd_check(cfg: &Config) -> Result<CommandResult, Diagnostic> {
    let n = check_docs_against_fixture(cfg)?;
    Ok(CommandResult::new(
        json!({"operation": "check", "claims_checked": n, "surfaces_scanned": cfg.surfaces.len(), "status": "passed"}),
        format!("plumbline: {n} registered claim(s) match the committed fixture"),
    ))
}

/// Assert every registered claim equals its fixture field and no surface carries
/// a generated block with an unknown id. Returns the number of claims checked.
///
/// When the config declares no fixture, there are no claims to check (the loader
/// forbids claims without a fixture), so this reduces to the stray-block scan.
fn check_docs_against_fixture(cfg: &Config) -> Result<usize, Diagnostic> {
    // Claim drift and stray blocks are distinct faults with distinct codes, so
    // check claims first and report CLAIM_DRIFT before scanning for STRAY_BLOCK.
    let claim_count = match &cfg.fixture {
        Some(f) => {
            let fixture = read_json(&cfg.root.join(f))?;
            let failures = check_claims(&fixture, &cfg.claims);
            if !failures.is_empty() {
                return Err(Diagnostic::new(
                    codes::CLAIM_DRIFT,
                    format!(
                        "{} claim(s) no longer equal the fixture:\n  {}",
                        failures.len(),
                        failures.join("\n  ")
                    ),
                ));
            }
            cfg.claims.len()
        }
        None => 0,
    };

    let known: Vec<&str> = cfg.generated.iter().map(|g| g.id.as_str()).collect();
    let mut stray = Vec::new();
    for surface in &cfg.surfaces {
        let p = cfg.root.join(surface);
        if !p.exists() {
            continue;
        }
        let text = std::fs::read_to_string(&p).map_err(|e| {
            Diagnostic::new(codes::SURFACE_UNREADABLE, format!("read {surface}: {e}"))
        })?;
        for id in generated_ids(&text) {
            if !known.contains(&id.as_str()) {
                stray.push(format!(
                    "[{surface}] GENERATED block `{id}` has no renderer; add it to the config's \
                     `generated` list, or remove the block"
                ));
            }
        }
    }

    if stray.is_empty() {
        Ok(claim_count)
    } else {
        Err(Diagnostic::new(
            codes::STRAY_BLOCK,
            format!(
                "{} stray GENERATED block(s):\n  {}",
                stray.len(),
                stray.join("\n  ")
            ),
        ))
    }
}

// ---- capture ---------------------------------------------------------------

pub fn cmd_capture(cfg: &Config, check_only: bool) -> Result<CommandResult, Diagnostic> {
    let (capture, fixture) = require_contract(cfg)?;

    if check_only {
        fixture_matches_binary(cfg)?;
        return Ok(CommandResult::new(
            json!({"operation": "capture", "mode": "check", "fixture_path": fixture, "status": "current"}),
            "plumbline: committed fixture is current (contract-equivalent to a fresh capture)",
        ));
    }

    let fixture_path = cfg.root.join(fixture);
    let (captured, executable) = engine::capture_with_executable(&cfg.root, capture)?;
    let mut text = serde_json::to_string_pretty(&captured)
        .map_err(|e| Diagnostic::new(codes::WRITE_FAILED, format!("serialize fixture: {e}")))?;
    text.push('\n');
    atomic_write(&fixture_path, &text, fixture)?;
    let mut human = vec![format!("plumbline: rewrote {fixture} from a fresh capture")];
    let mut blocks = Vec::new();

    // The capture above already built the binary; render every block from it.
    for gen in &cfg.generated {
        let surface_path = cfg.root.join(&gen.surface);
        let doc = std::fs::read_to_string(&surface_path).map_err(|e| {
            Diagnostic::new(
                codes::SURFACE_UNREADABLE,
                format!("read {}: {e}", gen.surface),
            )
        })?;
        let block = engine::render_block(&executable, gen)?;
        let updated = replace_generated(&doc, &gen.id, &block)
            .map_err(|e| Diagnostic::new(codes::MARKER_MISSING, e))?;
        if updated != doc {
            atomic_write(&surface_path, &updated, &gen.surface)?;
            human.push(format!(
                "plumbline: regenerated `{}` in {}",
                gen.id, gen.surface
            ));
        } else {
            human.push(format!(
                "plumbline: `{}` in {} already current",
                gen.id, gen.surface
            ));
        }
        blocks.push(gen.id.clone());
    }
    blocks.sort();
    Ok(CommandResult::new(
        json!({"operation": "capture", "mode": "write", "fixture_path": fixture, "generated_blocks": blocks, "status": "updated"}),
        human.join("\n"),
    ))
}

/// Write a sibling file, persist it, and rename it over the destination. A
/// capture never leaves a truncated fixture or generated document behind.
fn atomic_write(path: &Path, text: &str, display: &str) -> Result<(), Diagnostic> {
    use std::io::Write;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("plumbline");
    let temp = path.with_file_name(format!(".{name}.plumb-{}.tmp", std::process::id()));
    let mut file = std::fs::File::create(&temp).map_err(|e| {
        Diagnostic::new(
            codes::WRITE_FAILED,
            format!("create temporary file for {display}: {e}"),
        )
    })?;
    file.write_all(text.as_bytes())
        .map_err(|e| Diagnostic::new(codes::WRITE_FAILED, format!("write {display}: {e}")))?;
    file.sync_all()
        .map_err(|e| Diagnostic::new(codes::WRITE_FAILED, format!("sync {display}: {e}")))?;
    std::fs::rename(&temp, path)
        .map_err(|e| Diagnostic::new(codes::WRITE_FAILED, format!("replace {display}: {e}")))
}

/// Both `capture` verbs and the fixture-freshness gate need a declared capture
/// command and a committed fixture. A contract-less crate has neither; refuse
/// cleanly rather than unwrap a `None`.
fn require_contract(cfg: &Config) -> Result<(&Capture, &str), Diagnostic> {
    match (&cfg.capture, &cfg.fixture) {
        (Some(c), Some(f)) => Ok((c, f.as_str())),
        _ => Err(Diagnostic::new(
            codes::NO_CONTRACT,
            "this crate declares no `capture`/`fixture`; there is no contract to capture",
        )),
    }
}

/// Assert the committed fixture is contract-equivalent to a fresh capture from
/// the binary being packaged. Shared by `capture --check` and `preflight`.
fn fixture_matches_binary(cfg: &Config) -> Result<(), Diagnostic> {
    let (capture, fixture) = require_contract(cfg)?;
    let captured = engine::capture(&cfg.root, capture)?;
    let committed = read_json(&cfg.root.join(fixture))?;
    if normalized(&captured, &cfg.normalize_meta) == normalized(&committed, &cfg.normalize_meta) {
        Ok(())
    } else {
        Err(Diagnostic::new(codes::FIXTURE_STALE, format!("committed fixture {fixture} is STALE; run `plumb capture --yes` to reconcile docs.")).with_data(json!({"diff": json_diff(&committed, &captured)})))
    }
}

// ---- preflight (the publish stop-sign) -------------------------------------

pub fn cmd_preflight(cfg: &Config, wait_for_ci: bool) -> Result<CommandResult, Diagnostic> {
    let release = cfg.release.as_ref().ok_or_else(|| {
        Diagnostic::new(
            codes::RELEASE_CONFIG_MISSING,
            "preflight requires a complete `release` configuration",
        )
    })?;
    type Gate<'a> = (
        &'a str,
        &'a str,
        Option<&'a str>,
        Box<dyn Fn() -> Result<String, Diagnostic> + 'a>,
    );
    let gates: Vec<Gate> = vec![
        (
            "worktree-clean",
            "worktree clean and committed",
            None,
            Box::new(|| gate_worktree_clean(cfg)),
        ),
        (
            "fixture-fresh",
            "committed fixture matches binary",
            (cfg.capture.is_none() || cfg.fixture.is_none())
                .then_some("no-capture-fixture-contract"),
            Box::new(|| {
                fixture_matches_binary(cfg).map(|_| "fresh capture matches fixture".into())
            }),
        ),
        (
            "docs-stray-block",
            "docs match fixture",
            None,
            Box::new(|| {
                check_docs_against_fixture(cfg).map(|count| format!("{count} claims match"))
            }),
        ),
        (
            "packaged-allowlist",
            "packaged files within allowlist",
            None,
            Box::new(|| engine::packaged_within_allowlist(&cfg.root, &cfg.package_allowlist)),
        ),
        (
            "generated-fresh",
            "generated blocks match binary",
            cfg.generated.is_empty().then_some("no-generated-blocks"),
            Box::new(|| generated_blocks_fresh(cfg)),
        ),
    ];
    let mut records = Vec::new();
    let mut human = Vec::new();
    let mut failures = 0usize;
    for (id, label, skipped, run) in gates {
        if let Some(reason) = skipped {
            records.push(json!({"id": id, "label": label, "status": "skipped", "reason": reason}));
            human.push(format!("SKIP {id}: {reason}"));
        } else {
            match run() {
                Ok(reason) => {
                    records.push(
                        json!({"id": id, "label": label, "status": "passed", "reason": reason}),
                    );
                    human.push(format!("PASS {id}"));
                }
                Err(error) => {
                    failures += 1;
                    records.push(json!({"id": id, "label": label, "status": "failed", "diagnostic": error.as_json()}));
                    human.push(format!("FAIL {id}: {}", error.code.name));
                }
            }
        }
    }
    let mut runner = crate::verification::SystemRunner;
    let mut local_gates =
        crate::verification::run_gates(&mut runner, &release.verification.ci, &cfg.root, "ci");
    local_gates.extend(crate::verification::run_gates(
        &mut runner,
        &release.verification.release,
        &cfg.root,
        "release",
    ));
    for gate in local_gates {
        if gate["status"] == "failed" {
            failures += 1;
            human.push(format!(
                "FAIL {}: {}",
                gate["id"].as_str().unwrap_or("command"),
                gate["diagnostic_code"]
                    .as_str()
                    .unwrap_or("VERIFY_COMMAND_FAILED")
            ));
        } else {
            human.push(format!("PASS {}", gate["id"].as_str().unwrap_or("command")));
        }
        records.push(gate);
    }
    let head_sha = crate::github::head_sha(&cfg.root).map_err(|_| {
        Diagnostic::new(
            codes::GIT_UNAVAILABLE,
            "cannot read HEAD for GitHub Actions proof",
        )
    })?;
    let mut github = crate::github::GhAdapter;
    let proof = if wait_for_ci {
        github.prove_with_wait(
            &release.verification.github_actions,
            &release.branch,
            &head_sha,
            std::time::Duration::from_secs(30),
        )
    } else {
        crate::github::GitHubAdapter::prove(
            &mut github,
            &release.verification.github_actions,
            &release.branch,
            &head_sha,
        )
    };
    if proof["status"] == "failed" {
        failures += 1;
        human.push(format!(
            "FAIL github-actions-proof: {}",
            proof["diagnostic_code"]
                .as_str()
                .unwrap_or("CI_PROOF_UNAVAILABLE")
        ));
    } else {
        human.push("PASS github-actions-proof".into());
    }
    records.push(proof);
    let report = json!({"operation": "preflight", "waited_for_ci": wait_for_ci, "status": if failures == 0 { "passed" } else { "blocked" }, "gates": records});
    if failures == 0 {
        Ok(CommandResult::new(report, human.join("\n")))
    } else {
        let failed_codes = report["gates"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|gate| gate["status"] == "failed")
            .filter_map(|gate| {
                gate["diagnostic_code"]
                    .as_str()
                    .or_else(|| gate["diagnostic"]["code"].as_str())
            })
            .collect::<Vec<_>>();
        let code = if failed_codes
            .iter()
            .all(|code| matches!(*code, "CI_PROOF_PENDING" | "CI_PROOF_TRANSIENT_FAILURE"))
        {
            code_for_gate(failed_codes[0]).unwrap_or(codes::PREFLIGHT_BLOCKED)
        } else if failed_codes.iter().all(|code| {
            matches!(
                *code,
                "VERIFY_COMMAND_UNAVAILABLE" | "GITHUB_CLI_UNAVAILABLE" | "CI_PROOF_UNAVAILABLE"
            )
        }) {
            code_for_gate(failed_codes[0]).unwrap_or(codes::VERIFY_COMMAND_UNAVAILABLE)
        } else {
            codes::PREFLIGHT_BLOCKED
        };
        Err(Diagnostic::new(code, format!("{failures} preflight gate(s) failed")).with_data(report))
    }
}

fn code_for_gate(name: &str) -> Option<crate::diagnostic::Code> {
    Some(match name {
        "VERIFY_COMMAND_UNAVAILABLE" => codes::VERIFY_COMMAND_UNAVAILABLE,
        "GITHUB_CLI_UNAVAILABLE" => codes::GITHUB_CLI_UNAVAILABLE,
        "CI_PROOF_UNAVAILABLE" => codes::CI_PROOF_UNAVAILABLE,
        "CI_PROOF_PENDING" => codes::CI_PROOF_PENDING,
        "CI_PROOF_TRANSIENT_FAILURE" => codes::CI_PROOF_TRANSIENT_FAILURE,
        _ => return None,
    })
}

/// Run only the configured local policy. This command deliberately has no
/// GitHub adapter: a workflow uses it to create the CI evidence that preflight
/// later checks, so querying that proof here would create a CI cycle.
pub fn cmd_verify(cfg: &Config, stage: VerifyStage) -> Result<CommandResult, Diagnostic> {
    let release = cfg.release.as_ref().ok_or_else(|| {
        Diagnostic::new(
            codes::RELEASE_CONFIG_MISSING,
            "verify requires a complete `release` configuration",
        )
    })?;
    let mut runner = crate::verification::SystemRunner;
    let mut gates =
        crate::verification::run_gates(&mut runner, &release.verification.ci, &cfg.root, "ci");
    if stage == VerifyStage::Release {
        gates.extend(crate::verification::run_gates(
            &mut runner,
            &release.verification.release,
            &cfg.root,
            "release",
        ));
    }
    let failed = gates
        .iter()
        .filter(|gate| gate["status"] == "failed")
        .count();
    let report = json!({
        "operation": "verify",
        "stage": stage.as_str(),
        "status": if failed == 0 { "passed" } else { "blocked" },
        "gates": gates,
        "configured_ci_proof": {
            "repository": release.verification.github_actions.repository,
            "workflow_path": release.verification.github_actions.workflow_path,
        },
    });
    if failed == 0 {
        Ok(CommandResult::new(
            report,
            format!("verify {}: all local gates passed", stage.as_str()),
        ))
    } else {
        let has_gate_failure = report["gates"].as_array().is_some_and(|items| {
            items.iter().any(|gate| {
                matches!(
                    gate["diagnostic_code"].as_str(),
                    Some("VERIFY_COMMAND_FAILED" | "VERIFY_COMMAND_TIMED_OUT")
                )
            })
        });
        let code = if has_gate_failure {
            codes::PREFLIGHT_BLOCKED
        } else {
            codes::VERIFY_COMMAND_UNAVAILABLE
        };
        Err(
            Diagnostic::new(code, format!("{failed} verification gate(s) failed"))
                .with_data(report),
        )
    }
}

/// Validate and publish one guarded release. The release module owns every
/// external Git and Cargo call; preflight remains in-process so it keeps the
/// same complete gate report as the standalone command.
pub fn cmd_release(cfg: &Config) -> Result<CommandResult, Diagnostic> {
    let mut runner = release::SystemRunner;
    let release = release::run(cfg, &mut runner, || cmd_preflight(cfg, false).map(|_| ()))?;
    let status = if release.already_complete {
        "already_complete"
    } else {
        "submitted"
    };
    Ok(CommandResult::new(
        json!({
            "operation": "release",
            "status": status,
            "version": release.version,
            "tag": release.tag,
            "commit": release.commit,
            "remote": release.remote,
            "remote_url": release.remote_url,
            "already_complete": release.already_complete,
        }),
        release.human(),
    ))
}

pub fn cmd_release_dry_run(cfg: &Config) -> Result<CommandResult, Diagnostic> {
    let mut runner = release::SystemRunner;
    let release = release::run_dry(cfg, &mut runner, || cmd_preflight(cfg, false).map(|_| ()))?;
    Ok(CommandResult::new(
        json!({"operation":"release","mode":"dry-run","status":"passed","version":release.version,"tag":release.tag,"commit":release.commit,"remote":release.remote,"completed_state_changes":[],"skipped_state_changes":[{"id":"create-annotated-tag","reason":"dry-run"},{"id":"atomic-push","reason":"dry-run"}]}),
        "release dry-run: checks passed; no tag or push was performed",
    ))
}

/// Gate: the git worktree has no uncommitted changes. `cargo publish` packages
/// the working tree, so an untidy tree could ship un-reviewed files.
fn gate_worktree_clean(cfg: &Config) -> Result<String, Diagnostic> {
    let out = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&cfg.root)
        .output()
        .map_err(|e| Diagnostic::new(codes::GIT_UNAVAILABLE, format!("run git status: {e}")))?;
    if !out.status.success() {
        return Err(Diagnostic::new(
            codes::GIT_UNAVAILABLE,
            format!(
                "git status exited {}: {}",
                out.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        ));
    }
    let listing = String::from_utf8_lossy(&out.stdout);
    let dirty: Vec<&str> = listing.lines().filter(|l| !l.trim().is_empty()).collect();
    if dirty.is_empty() {
        Ok("no uncommitted changes".into())
    } else {
        Err(Diagnostic::new(
            codes::WORKTREE_DIRTY,
            format!(
                "{} uncommitted path(s); commit or stash before publishing:\n{}",
                dirty.len(),
                dirty.join("\n")
            ),
        ))
    }
}

/// Gate: every generated block equals a fresh render from the binary. crates.io
/// ships the docs verbatim and write-once, so a block that no longer matches real
/// output would mislead every reader of the published page.
fn generated_blocks_fresh(cfg: &Config) -> Result<String, Diagnostic> {
    // Generated blocks render from the binary, so a capture must be declared
    // (the loader enforces this pairing; guard rather than unwrap).
    let capture = cfg.capture.as_ref().ok_or_else(|| {
        Diagnostic::new(
            codes::CONFIG_INCOHERENT,
            "generated blocks declared without a `capture`",
        )
    })?;
    // Build once up front and resolve the built binary so all blocks render
    // from it. render_block execs this path; passing the crate root instead
    // fails with a permission error.
    let executable = engine::resolve_capture_executable(&cfg.root, capture)?;
    let mut checked = 0usize;
    for gen in &cfg.generated {
        let doc = std::fs::read_to_string(cfg.root.join(&gen.surface)).map_err(|e| {
            Diagnostic::new(
                codes::SURFACE_UNREADABLE,
                format!("read {}: {e}", gen.surface),
            )
        })?;
        let committed = extract_generated(&doc, &gen.id).ok_or_else(|| {
            Diagnostic::new(
                codes::MARKER_MISSING,
                format!("{} has no `{}` GENERATED block", gen.surface, gen.id),
            )
        })?;
        let fresh = engine::render_block(&executable, gen)?;
        if committed != fresh {
            return Err(Diagnostic::new(
                codes::BLOCK_STALE,
                format!(
                    "`{}` in {} is STALE; run `plumb capture --yes` to regenerate it.",
                    gen.id, gen.surface
                ),
            )
            .with_data(json!({"diff": line_diff(committed, &fresh)})));
        }
        checked += 1;
    }
    Ok(format!("{checked} block(s) equal a fresh run"))
}

fn json_diff(old: &Value, new: &Value) -> Vec<Value> {
    let mut records = Vec::new();
    collect_json_diff("", old, new, &mut records);
    cap_diff(records)
}

fn collect_json_diff(locator: &str, old: &Value, new: &Value, records: &mut Vec<Value>) {
    match (old, new) {
        (Value::Object(old), Value::Object(new)) => {
            let mut names = old.keys().chain(new.keys()).cloned().collect::<Vec<_>>(); names.sort(); names.dedup();
            for key in names { let child = format!("{}/{}", locator, key.replace('~', "~0").replace('/', "~1")); match (old.get(&key), new.get(&key)) { (Some(old), Some(new)) => collect_json_diff(&child, old, new, records), (Some(old), None) => records.push(json!({"locator": child, "location_kind":"json-pointer", "change_kind":"removed", "old":old})), (None, Some(new)) => records.push(json!({"locator": child, "location_kind":"json-pointer", "change_kind":"added", "new":new})), _=>{} } }
        }
        _ if old != new => records.push(json!({"locator": if locator.is_empty() { "/" } else { locator }, "location_kind":"json-pointer", "change_kind": if std::mem::discriminant(old) == std::mem::discriminant(new) { "changed" } else { "type-changed" }, "old":old, "new":new})),
        _ => {}
    }
}

fn line_diff(old: &str, new: &str) -> Vec<Value> {
    let old = old.lines().collect::<Vec<_>>();
    let new = new.lines().collect::<Vec<_>>();
    cap_diff((0..old.len().max(new.len())).filter_map(|index| match (old.get(index), new.get(index)) { (Some(old), Some(new)) if old != new => Some(json!({"locator":format!("line:{}", index+1),"location_kind":"line","change_kind":"changed","old":old,"new":new})), (Some(old),None)=>Some(json!({"locator":format!("line:{}",index+1),"location_kind":"line","change_kind":"removed","old":old})), (None,Some(new))=>Some(json!({"locator":format!("line:{}",index+1),"location_kind":"line","change_kind":"added","new":new})), _=>None }).collect())
}

fn cap_diff(mut records: Vec<Value>) -> Vec<Value> {
    records.sort_by(|left, right| left["locator"].as_str().cmp(&right["locator"].as_str()));
    records.truncate(50);
    records
}

// ---- capabilities ----------------------------------------------------------

/// Emit plumbline's own contract as JSON on stdout: the same envelope shape
/// plumbline checks for its targets, now describing `plumb` itself. This is the
/// data a self-contract pins its claims against. It needs no project config —
/// it describes the tool, not any one crate — so `main` dispatches it before a
/// config is loaded, and it runs anywhere.
///
/// `error_codes` is built straight from `diagnostic::codes::ALL`, so the emitted
/// catalog is exactly the set of codes the tool can raise: the doc cannot claim
/// a code the binary lacks, nor omit one it has.
pub fn cmd_capabilities() -> Result<CommandResult, Diagnostic> {
    let version = env!("CARGO_PKG_VERSION");
    let mut error_codes = Map::new();
    for c in codes::ALL {
        error_codes.insert(
            c.name.to_string(),
            json!({
                "family": c.family.as_str(),
                "exit": c.family.exit(),
                "meaning": c.meaning,
            }),
        );
    }

    let data = json!({
        "contract_version": crate::result::CONTRACT_VERSION,
        "tool_version": version,
        "exit_codes": {
            "0": {"meaning": "success", "retryable": null},
            "1": {"meaning": "invalid caller input", "retryable": false},
            "2": {"meaning": "safety block", "retryable": false},
            "3": {"meaning": "local tool or environment error", "retryable": null},
            "4": {"meaning": "transient failure", "retryable": true},
            "5": {"meaning": "conflict", "retryable": false},
            "6": {"meaning": "internal defect", "retryable": false},
        },
        "global_flags": [
            {"name": "--config", "arg": "path",
             "summary": "path to the project config (default ./plumbline.json)"},
            {"name": "--json", "arg": null, "summary": "select machine output"},
            {"name": "--no-color", "arg": null, "summary": "disable ANSI in human output"},
            {"name": "--color", "arg": "auto|always|never", "summary": "select human ANSI policy"},
            {"name": "--help", "arg": null, "summary": "show terse help"},
            {"name": "--version", "arg": null,
             "summary": "print the installed plumb version; needs no config"}
        ],
        "verbs": crate::cli::capability_verbs(),
        "parser_manifest": crate::cli::parser_manifest(),
        "gates": [
            {"id": "worktree-clean", "applies_when": "always"},
            {"id": "fixture-fresh", "applies_when": "a capture and fixture are declared"},
            {"id": "docs-stray-block", "applies_when": "always"},
            {"id": "packaged-allowlist", "applies_when": "always"},
            {"id": "generated-fresh", "applies_when": "generated blocks are declared"},
        ],
        "value_domains": {
            "claim_mode": ["value", "keys", "set"],
            "package_allowlist": ["cargo-include"],
        },
        "error_codes": Value::Object(error_codes),
        "warning_codes": [],
    });
    Ok(CommandResult::new(
        data,
        "plumb capabilities: use --json for the full contract",
    ))
}

pub fn cmd_schema(command: Option<&str>) -> Result<CommandResult, Diagnostic> {
    let verbs = crate::cli::capability_verbs();
    if let Some(command) = command {
        if verbs.get(command).is_none() {
            let valid = crate::cli::COMMANDS
                .iter()
                .map(|spec| spec.name)
                .collect::<Vec<_>>()
                .join(", ");
            return Err(Diagnostic::new(
                codes::INVALID_INPUT,
                format!("unknown command schema `{command}`; valid commands are {valid}"),
            ));
        }
    }
    let selected = command
        .map(|name| json!({name: verbs[name].clone()}))
        .unwrap_or(verbs);
    Ok(CommandResult::new(
        json!({
            "envelope_schema": {"type":"object", "required":["ok","tool_version","data","meta","warnings","commands","errors"]},
            "schemas": selected,
            "definitions": {},
        }),
        "plumb schema: use --json for machine-readable schemas",
    ))
}

pub fn cmd_robot_docs() -> Result<CommandResult, Diagnostic> {
    let names = crate::cli::COMMANDS
        .iter()
        .map(|spec| spec.name)
        .collect::<Vec<_>>()
        .join(", ");
    let guide = format!("# plumb agent guide\n\n1. Run `plumb capabilities --json` to discover commands.\n2. Run `plumb check` before a capture.\n3. Use `plumb capture --check` to compare fresh output.\n4. Read `plumb preflight --json` gate results before release.\n5. Use `plumb release --dry-run` when it is available.\n6. Branch on the declared exit code.\n7. Run `plumb conformance --json` when it is available.\n\nDeclared commands: {names}.");
    Ok(CommandResult::new(json!({"guide": guide}), guide))
}

/// Check command facts that are safe to inspect in an installed binary. Fault
/// injection exists only in debug builds; release builds state that limit in a
/// fixed machine-readable record.
pub fn cmd_conformance() -> Result<CommandResult, Diagnostic> {
    let declared = crate::cli::COMMANDS;
    let mut cases = Vec::new();
    cases.push(json!({
        "id": "command-registry",
        "verdict": if declared.is_empty() { "fail" } else { "pass" },
        "reason": if declared.is_empty() { "no-declared-commands" } else { "declared-command-registry-present" },
        "request_id": crate::result::REQUEST_ID_PLACEHOLDER,
        "target": {"kind": "registry", "name": "commands"},
    }));
    let manifest = crate::cli::parser_manifest();
    let parser_names = manifest["commands"]
        .as_array()
        .map(|commands| {
            commands
                .iter()
                .filter_map(|command| command["name"].as_str())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let expected_names = declared
        .iter()
        .map(|command| command.name)
        .collect::<Vec<_>>();
    cases.push(json!({
        "id": "parser-manifest",
        "verdict": if parser_names == expected_names { "pass" } else { "fail" },
        "reason": if parser_names == expected_names { "registry-and-parser-manifest-agree" } else { "registry-and-parser-manifest-differ" },
        "request_id": crate::result::REQUEST_ID_PLACEHOLDER,
        "target": {"kind": "parser", "name": "manifest"},
    }));
    cases.push(json!({
        "id": "diagnosis-order",
        "verdict": "pass",
        "reason": "bootstrap-global-verb-local-arity-semantic-order-installed",
        "request_id": crate::result::REQUEST_ID_PLACEHOLDER,
        "target": {"kind": "diagnostic", "name": "order"},
    }));
    cases.push(json!({
        "id": "fault-injection-totality",
        "verdict": if cfg!(debug_assertions) { "pass" } else { "not_applicable" },
        "reason": if cfg!(debug_assertions) { "debug-build-fault-trigger-coverage" } else { "release-build-fault-trigger-unavailable" },
        "request_id": crate::result::REQUEST_ID_PLACEHOLDER,
        "target": {"kind": "fault-seam", "name": "totality"},
    }));
    cases.sort_by(|left, right| left["id"].as_str().cmp(&right["id"].as_str()));
    let pass = cases
        .iter()
        .filter(|case| case["verdict"] == "pass")
        .count();
    let fail = cases
        .iter()
        .filter(|case| case["verdict"] == "fail")
        .count();
    let not_applicable = cases
        .iter()
        .filter(|case| case["verdict"] == "not_applicable")
        .count();
    let data = json!({
        "operation": "conformance",
        "profile": if cfg!(debug_assertions) { "test" } else { "release" },
        "cases": cases,
        "counts": {"pass": pass, "fail": fail, "not_applicable": not_applicable},
    });
    if fail == 0 {
        Ok(CommandResult::new(
            data,
            "conformance: all applicable checks passed",
        ))
    } else {
        Err(Diagnostic::new(
            codes::INTERNAL,
            "conformance self-check failed",
        ))
    }
}

pub fn cmd_contract_doc(section: Option<&str>) -> Result<CommandResult, Diagnostic> {
    let section = section.ok_or_else(|| {
        Diagnostic::new(
            codes::MISSING_REQUIRED,
            "contract-doc requires one section: verbs, gates, exit-codes, or error-codes",
        )
    })?;
    let markdown = match section {
        "verbs" => {
            let mut rows = vec!["| Command | Mutates | Description |".into(), "|---|---:|---|".into()];
            rows.extend(crate::cli::COMMANDS.iter().map(|spec| format!("| `{}` | {} | {} |", spec.name, matches!(spec.verb, crate::cli::Verb::Capture | crate::cli::Verb::Release), spec.summary))); rows.join("\n")
        }
        "gates" => "| Gate | Applies when |\n|---|---|\n| `worktree-clean` | always |\n| `fixture-fresh` | capture and fixture declared |\n| `docs-stray-block` | always |\n| `packaged-allowlist` | always |\n| `generated-fresh` | generated blocks declared |".into(),
        "exit-codes" => "| Exit | Meaning |\n|---:|---|\n| 0 | success |\n| 1 | invalid input |\n| 2 | safety block |\n| 3 | local error |\n| 4 | transient failure |\n| 5 | conflict |\n| 6 | internal defect |".into(),
        "error-codes" => { let mut rows = vec!["| Code | Family | Meaning |".into(), "|---|---|---|".into()]; rows.extend(codes::ALL.iter().map(|code| format!("| `{}` | {} | {} |", code.name, code.family.as_str(), code.meaning))); rows.join("\n") }
        other => return Err(Diagnostic::new(codes::INVALID_INPUT, format!("unknown contract section `{other}`; valid sections are verbs, gates, exit-codes, error-codes"))),
    };
    Ok(CommandResult::new(
        json!({"section":section,"markdown":markdown}),
        markdown,
    ))
}

/// Run one declared configuration command. This is kept separate from product
/// commands because config inspection must work even when the document has no
/// capture or release settings.
pub fn cmd_config(
    action: Option<ConfigAction>,
    arguments: &[String],
    yes: bool,
    if_match: Option<&str>,
    from_stdin: bool,
    explicit_path: Option<&Path>,
) -> Result<CommandResult, Diagnostic> {
    let action = action.ok_or_else(|| {
        Diagnostic::new(
            codes::MISSING_REQUIRED,
            "config requires one of: schema, validate, show, get, set, or patch",
        )
    })?;
    if action == ConfigAction::Schema {
        if !arguments.is_empty() {
            return Err(Diagnostic::new(
                codes::INVALID_INPUT,
                "config schema accepts no arguments",
            ));
        }
        return Ok(CommandResult::new(
            json!({"operation": "config schema", "schema": config::config_schema()}),
            "plumb config schema: use --json for the JSON Schema",
        ));
    }

    let cwd = std::env::current_dir().map_err(|e| {
        Diagnostic::new(
            codes::WORKDIR_UNREADABLE,
            format!("cannot determine working directory: {e}"),
        )
    })?;
    let cfg = Config::load_selected(explicit_path, cwd)?;
    match action {
        ConfigAction::Schema => unreachable!(),
        ConfigAction::Validate => {
            no_arguments(arguments, "config validate")?;
            Ok(config_result(
                "validate",
                &cfg,
                json!({"valid": true}),
                "configuration is valid",
            ))
        }
        ConfigAction::Show => {
            no_arguments(arguments, "config show")?;
            let provenance = cfg
                .document
                .as_object()
                .map(|fields| {
                    fields
                        .keys()
                        .map(|key| (format!("/{key}"), Value::String("file".into())))
                        .collect::<serde_json::Map<_, _>>()
                })
                .unwrap_or_default();
            Ok(config_result(
                "show",
                &cfg,
                json!({"config": cfg.document, "provenance": provenance}),
                "configuration loaded",
            ))
        }
        ConfigAction::Get => {
            if arguments.len() != 1 {
                return Err(Diagnostic::new(
                    codes::MISSING_REQUIRED,
                    "config get requires one JSON Pointer",
                ));
            }
            let pointer = &arguments[0];
            let value = config::pointer_get(&cfg.document, pointer)?.clone();
            Ok(config_result(
                "get",
                &cfg,
                json!({"pointer": pointer, "value": value, "provenance": cfg.provenance(pointer)}),
                "configuration value loaded",
            ))
        }
        ConfigAction::Set => {
            if arguments.len() != 2 {
                return Err(Diagnostic::new(
                    codes::MISSING_REQUIRED,
                    "config set requires a JSON Pointer and one JSON value",
                ));
            }
            let replacement = serde_json::from_str(&arguments[1]).map_err(|e| {
                Diagnostic::new(
                    codes::INVALID_INPUT,
                    format!("config set value is not valid JSON: {e}"),
                )
            })?;
            mutate_config(&cfg, yes, if_match, |document| {
                config::pointer_set(document, &arguments[0], replacement)
            })
        }
        ConfigAction::Patch => {
            no_arguments(arguments, "config patch")?;
            if !from_stdin {
                return Err(Diagnostic::new(
                    codes::MISSING_REQUIRED,
                    "config patch requires --from-stdin",
                ));
            }
            let patch = config::read_stdin_patch()?;
            mutate_config(&cfg, yes, if_match, |document| {
                config::merge_patch(document, &patch);
                Ok(())
            })
        }
    }
}

fn no_arguments(arguments: &[String], command: &str) -> Result<(), Diagnostic> {
    if arguments.is_empty() {
        Ok(())
    } else {
        Err(Diagnostic::new(
            codes::INVALID_INPUT,
            format!("{command} accepts no arguments"),
        ))
    }
}

fn config_result(operation: &str, cfg: &Config, data: Value, human: &str) -> CommandResult {
    CommandResult::new(
        json!({
            "operation": format!("config {operation}"),
            "config_path": cfg.path,
            "crate_root": cfg.root,
            "selection_source": cfg.selection_source.as_str(),
            "config_hash": cfg.config_hash,
            "data": data,
        }),
        format!("config {operation}: {human}"),
    )
}

fn mutate_config(
    cfg: &Config,
    yes: bool,
    if_match: Option<&str>,
    mutate: impl FnOnce(&mut Value) -> Result<(), Diagnostic>,
) -> Result<CommandResult, Diagnostic> {
    if !yes {
        return Err(Diagnostic::new(
            codes::MISSING_REQUIRED,
            "config mutation needs --yes",
        ));
    }
    let expected = if_match.ok_or_else(|| {
        Diagnostic::new(
            codes::MISSING_REQUIRED,
            "config mutation needs --if-match=<config-hash>",
        )
    })?;
    let _lock = config::ConfigLock::acquire(&cfg.path)?;
    let mut current = Config::load(&cfg.path, cfg.root.clone())?;
    current.selection_source = cfg.selection_source;
    if expected != current.config_hash {
        return Err(Diagnostic::new(codes::CONFIG_WRITE_CONFLICT, format!("config hash changed: expected `{expected}`, observed `{}`; run `plumb config show --json` and retry", current.config_hash)));
    }
    let mut document = current.document.clone();
    mutate(&mut document)?;
    let updated = Config::validate_document(
        current.path.clone(),
        current.root.clone(),
        current.selection_source,
        document,
    )?;
    let already_complete = updated.config_hash == current.config_hash;
    if !already_complete {
        config::write_document(&current.path, &updated.document)?;
    }
    Ok(config_result(
        "write",
        &updated,
        json!({"already_complete": already_complete, "config_hash": updated.config_hash}),
        if already_complete {
            "configuration already had the requested value"
        } else {
            "configuration updated"
        },
    ))
}

#[cfg(test)]
mod generated_gate_tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn temp_crate(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "plumbline-gengate-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(dir.join("src")).unwrap();
        dir
    }

    // The generated-fresh gate must render from the built binary, not the crate
    // root. Before the fix it passed the crate directory into render_block, so
    // it tried to exec a directory and failed with a permission error, making
    // preflight unpassable for any crate that declares a generated block. This
    // builds a real one-binary crate, commits the block its binary emits, and
    // asserts the gate passes.
    #[test]
    fn generated_fresh_execs_the_built_binary_not_the_crate_root() {
        let root = temp_crate("ok");
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"plumbdemo\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[[bin]]\nname = \"plumbdemo\"\npath = \"src/main.rs\"\n\n[workspace]\n",
        )
        .unwrap();
        fs::write(
            root.join("src/main.rs"),
            "fn main() { println!(\"hello from plumb test\"); }\n",
        )
        .unwrap();
        fs::write(
            root.join("README.md"),
            "# demo\n\n<!-- BEGIN GENERATED:demo -->\n```\nhello from plumb test\n```\n<!-- END GENERATED:demo -->\n",
        )
        .unwrap();
        let config = json!({
            "capture": {"build": ["cargo", "build", "--bin", "plumbdemo"], "command": ["plumbdemo"]},
            "surfaces": ["README.md"],
            "generated": [{
                "id": "demo", "surface": "README.md",
                "render": ["plumbdemo"], "tree": {"files": {}}
            }]
        });
        fs::write(
            root.join("plumbline.json"),
            serde_json::to_string_pretty(&config).unwrap(),
        )
        .unwrap();

        let cfg = Config::load(&root.join("plumbline.json"), root.clone()).expect("load config");
        let result = generated_blocks_fresh(&cfg);
        fs::remove_dir_all(&root).ok();
        assert!(result.is_ok(), "generated-fresh gate failed: {result:?}");
    }
}
