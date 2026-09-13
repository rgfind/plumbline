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

use crate::config::{Capture, Config};
use crate::diagnostic::{codes, Diagnostic};
use crate::engine;
use crate::markers::{extract_generated, generated_ids, replace_generated};
use crate::registry::{check_claims, normalized};
use serde_json::Value;
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

pub fn cmd_check(cfg: &Config) -> Result<(), Diagnostic> {
    let n = check_docs_against_fixture(cfg)?;
    println!("plumbline: {n} registered claim(s) match the committed fixture");
    Ok(())
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

pub fn cmd_capture(cfg: &Config, check_only: bool) -> Result<(), Diagnostic> {
    let (capture, fixture) = require_contract(cfg)?;

    if check_only {
        fixture_matches_binary(cfg)?;
        println!(
            "plumbline: committed fixture is current (contract-equivalent to a fresh capture)"
        );
        return Ok(());
    }

    let fixture_path = cfg.root.join(fixture);
    let captured = engine::capture(&cfg.root, capture)?;
    let mut text = serde_json::to_string_pretty(&captured)
        .map_err(|e| Diagnostic::new(codes::WRITE_FAILED, format!("serialize fixture: {e}")))?;
    text.push('\n');
    std::fs::write(&fixture_path, text)
        .map_err(|e| Diagnostic::new(codes::WRITE_FAILED, format!("write {fixture}: {e}")))?;
    println!("plumbline: rewrote {fixture} from a fresh capture");

    // The capture above already built the binary; render every block from it.
    for gen in &cfg.generated {
        let surface_path = cfg.root.join(&gen.surface);
        let doc = std::fs::read_to_string(&surface_path).map_err(|e| {
            Diagnostic::new(
                codes::SURFACE_UNREADABLE,
                format!("read {}: {e}", gen.surface),
            )
        })?;
        let block = engine::render_block(&cfg.root, gen)?;
        let updated = replace_generated(&doc, &gen.id, &block)
            .map_err(|e| Diagnostic::new(codes::MARKER_MISSING, e))?;
        if updated != doc {
            std::fs::write(&surface_path, updated).map_err(|e| {
                Diagnostic::new(codes::WRITE_FAILED, format!("write {}: {e}", gen.surface))
            })?;
            println!("plumbline: regenerated `{}` in {}", gen.id, gen.surface);
        } else {
            println!("plumbline: `{}` in {} already current", gen.id, gen.surface);
        }
    }
    Ok(())
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
        Err(Diagnostic::new(
            codes::FIXTURE_STALE,
            format!(
                "committed fixture {fixture} is STALE: a fresh capture differs. \
                 Run `plumb capture` and reconcile the docs."
            ),
        ))
    }
}

// ---- preflight (the publish stop-sign) -------------------------------------

pub fn cmd_preflight(cfg: &Config) -> Result<(), Diagnostic> {
    let name = cfg
        .capture
        .as_ref()
        .and_then(|c| c.command.first())
        .cloned()
        .unwrap_or_else(|| "the crate".into());
    println!("preflight: publish gate for `{name}` (crates.io is write-once)");

    // Build the gate list. Gates 1, 3 and 4 apply to every crate. Gate 2
    // (fixture freshness) is present only when the crate declares a contract to
    // capture; gate 5 (generated blocks) only when it declares blocks. A
    // contract-less crate (like plumbline) runs the universal gates alone.
    type Gate<'a> = (&'a str, Box<dyn Fn() -> Result<String, Diagnostic> + 'a>);
    let mut gates: Vec<Gate> = vec![(
        "worktree clean and committed",
        Box::new(|| gate_worktree_clean(cfg)),
    )];
    if cfg.capture.is_some() && cfg.fixture.is_some() {
        gates.push((
            "committed fixture matches the binary",
            Box::new(|| {
                fixture_matches_binary(cfg)
                    .map(|()| "a fresh capture equals the committed fixture".into())
            }),
        ));
    }
    gates.push((
        "docs match the fixture (no stray blocks)",
        Box::new(|| {
            check_docs_against_fixture(cfg)
                .map(|n| format!("{n} claim(s) match; no stray GENERATED block"))
        }),
    ));
    gates.push((
        "packaged files within the include allowlist",
        Box::new(|| engine::packaged_within_allowlist(&cfg.root, &cfg.package_allowlist)),
    ));
    if !cfg.generated.is_empty() {
        gates.push((
            "generated blocks match the binary",
            Box::new(|| generated_blocks_fresh(cfg)),
        ));
    }

    let total = gates.len();
    let mut failures = 0usize;
    for (i, (label, run)) in gates.iter().enumerate() {
        let step = i + 1;
        match run() {
            Ok(note) => println!("  [{step}/{total}] PASS  {label} — {note}"),
            Err(d) => {
                failures += 1;
                println!("  [{step}/{total}] FAIL  {label}");
                // Print the failing gate's own diagnostic, code and all, so the
                // leaf code (for example `[STRAY_BLOCK]`) is visible per gate.
                for line in d.to_string().lines() {
                    println!("            {line}");
                }
            }
        }
    }

    if failures == 0 {
        println!("preflight: OK — every gate passed; safe to `cargo publish`");
        Ok(())
    } else {
        Err(Diagnostic::new(
            codes::PREFLIGHT_FAILED,
            format!("{failures} gate(s) failed; do NOT `cargo publish` until each is green"),
        ))
    }
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
    // Build once up front so all blocks render from the same binary.
    engine::run_build(&cfg.root, &capture.build)?;
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
        let fresh = engine::render_block(&cfg.root, gen)?;
        if committed != fresh {
            return Err(Diagnostic::new(
                codes::BLOCK_STALE,
                format!(
                    "`{}` in {} is STALE; run `plumb capture` to regenerate it.\n\
                     --- committed ---\n{committed}\n--- fresh ---\n{fresh}",
                    gen.id, gen.surface
                ),
            ));
        }
        checked += 1;
    }
    Ok(format!("{checked} block(s) equal a fresh run"))
}
