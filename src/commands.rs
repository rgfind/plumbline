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

use crate::config::Config;
use crate::engine;
use crate::markers::{extract_generated, generated_ids, replace_generated};
use crate::registry::{check_claims, normalized};
use serde_json::Value;
use std::path::Path;
use std::process::Command;

fn read_json(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("parse {}: {e}", path.display()))
}

// ---- check -----------------------------------------------------------------

pub fn cmd_check(cfg: &Config) -> Result<(), String> {
    let n = check_docs_against_fixture(cfg)?;
    println!("plumbline: {n} registered claim(s) match the committed fixture");
    Ok(())
}

/// Assert every registered claim equals its fixture field and no surface carries
/// a generated block with an unknown id. Returns the number of claims checked.
fn check_docs_against_fixture(cfg: &Config) -> Result<usize, String> {
    let fixture = read_json(&cfg.root.join(&cfg.fixture))?;
    let mut failures = check_claims(&fixture, &cfg.claims);

    let known: Vec<&str> = cfg.generated.iter().map(|g| g.id.as_str()).collect();
    for surface in &cfg.surfaces {
        let p = cfg.root.join(surface);
        if !p.exists() {
            continue;
        }
        let text = std::fs::read_to_string(&p).map_err(|e| format!("read {surface}: {e}"))?;
        for id in generated_ids(&text) {
            if !known.contains(&id.as_str()) {
                failures.push(format!(
                    "[{surface}] GENERATED block `{id}` has no renderer; add it to the config's \
                     `generated` list, or remove the block"
                ));
            }
        }
    }

    if failures.is_empty() {
        Ok(cfg.claims.len())
    } else {
        Err(format!(
            "{} claim(s) failed:\n  {}",
            failures.len(),
            failures.join("\n  ")
        ))
    }
}

// ---- capture ---------------------------------------------------------------

pub fn cmd_capture(cfg: &Config, check_only: bool) -> Result<(), String> {
    if check_only {
        fixture_matches_binary(cfg)?;
        println!("plumbline: committed fixture is current (contract-equivalent to a fresh capture)");
        return Ok(());
    }

    let fixture_path = cfg.root.join(&cfg.fixture);
    let captured = engine::capture(&cfg.root, cfg)?;
    let mut text = serde_json::to_string_pretty(&captured).map_err(|e| format!("serialize: {e}"))?;
    text.push('\n');
    std::fs::write(&fixture_path, text).map_err(|e| format!("write {}: {e}", cfg.fixture))?;
    println!("plumbline: rewrote {} from a fresh capture", cfg.fixture);

    // The capture above already built the binary; render every block from it.
    for gen in &cfg.generated {
        let surface_path = cfg.root.join(&gen.surface);
        let doc = std::fs::read_to_string(&surface_path)
            .map_err(|e| format!("read {}: {e}", gen.surface))?;
        let block = engine::render_block(&cfg.root, gen)?;
        let updated = replace_generated(&doc, &gen.id, &block)?;
        if updated != doc {
            std::fs::write(&surface_path, updated)
                .map_err(|e| format!("write {}: {e}", gen.surface))?;
            println!("plumbline: regenerated `{}` in {}", gen.id, gen.surface);
        } else {
            println!("plumbline: `{}` in {} already current", gen.id, gen.surface);
        }
    }
    Ok(())
}

/// Assert the committed fixture is contract-equivalent to a fresh capture from
/// the binary being packaged. Shared by `capture --check` and `preflight`.
fn fixture_matches_binary(cfg: &Config) -> Result<(), String> {
    let captured = engine::capture(&cfg.root, cfg)?;
    let committed = read_json(&cfg.root.join(&cfg.fixture))?;
    if normalized(&captured, &cfg.normalize_meta) == normalized(&committed, &cfg.normalize_meta) {
        Ok(())
    } else {
        Err(format!(
            "committed fixture {} is STALE: a fresh capture differs. \
             Run `plumb capture` and reconcile the docs.",
            cfg.fixture
        ))
    }
}

// ---- preflight (the publish stop-sign) -------------------------------------

pub fn cmd_preflight(cfg: &Config) -> Result<(), String> {
    let name = cfg
        .capture
        .command
        .first()
        .cloned()
        .unwrap_or_else(|| "the crate".into());
    println!("preflight: publish gate for `{name}` (crates.io is write-once)");

    // Build the gate list. Gates 1-4 are fixed; gate 5 covers every generated
    // block, and is present only when the config declares blocks.
    type Gate<'a> = (&'a str, Box<dyn Fn() -> Result<String, String> + 'a>);
    let mut gates: Vec<Gate> = vec![
        (
            "worktree clean and committed",
            Box::new(|| gate_worktree_clean(cfg)),
        ),
        (
            "committed fixture matches the binary",
            Box::new(|| fixture_matches_binary(cfg).map(|()| "a fresh capture equals the committed fixture".into())),
        ),
        (
            "docs match the fixture (no stray blocks)",
            Box::new(|| check_docs_against_fixture(cfg).map(|n| format!("{n} claim(s) match; no stray GENERATED block"))),
        ),
        (
            "packaged files within the include allowlist",
            Box::new(|| engine::packaged_within_allowlist(&cfg.root, &cfg.package_allowlist)),
        ),
    ];
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
            Err(e) => {
                failures += 1;
                println!("  [{step}/{total}] FAIL  {label}");
                for line in e.lines() {
                    println!("            {line}");
                }
            }
        }
    }

    if failures == 0 {
        println!("preflight: OK — every gate passed; safe to `cargo publish`");
        Ok(())
    } else {
        Err(format!(
            "{failures} gate(s) failed; do NOT `cargo publish` until each is green"
        ))
    }
}

/// Gate: the git worktree has no uncommitted changes. `cargo publish` packages
/// the working tree, so an untidy tree could ship un-reviewed files.
fn gate_worktree_clean(cfg: &Config) -> Result<String, String> {
    let out = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&cfg.root)
        .output()
        .map_err(|e| format!("run git status: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git status exited {}: {}",
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let listing = String::from_utf8_lossy(&out.stdout);
    let dirty: Vec<&str> = listing.lines().filter(|l| !l.trim().is_empty()).collect();
    if dirty.is_empty() {
        Ok("no uncommitted changes".into())
    } else {
        Err(format!(
            "{} uncommitted path(s); commit or stash before publishing:\n{}",
            dirty.len(),
            dirty.join("\n")
        ))
    }
}

/// Gate: every generated block equals a fresh render from the binary. crates.io
/// ships the docs verbatim and write-once, so a block that no longer matches real
/// output would mislead every reader of the published page.
fn generated_blocks_fresh(cfg: &Config) -> Result<String, String> {
    // Build once up front so all blocks render from the same binary.
    engine::run_build(&cfg.root, &cfg.capture.build)?;
    let mut checked = 0usize;
    for gen in &cfg.generated {
        let doc = std::fs::read_to_string(cfg.root.join(&gen.surface))
            .map_err(|e| format!("read {}: {e}", gen.surface))?;
        let committed = extract_generated(&doc, &gen.id)
            .ok_or_else(|| format!("{} has no `{}` GENERATED block", gen.surface, gen.id))?;
        let fresh = engine::render_block(&cfg.root, gen)?;
        if committed != fresh {
            return Err(format!(
                "`{}` in {} is STALE; run `plumb capture` to regenerate it.\n\
                 --- committed ---\n{committed}\n--- fresh ---\n{fresh}",
                gen.id, gen.surface
            ));
        }
        checked += 1;
    }
    Ok(format!("{checked} block(s) equal a fresh run"))
}
