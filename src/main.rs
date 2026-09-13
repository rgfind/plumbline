//! plumbline (`plumb`): keep a Rust crate's shipped docs in lock-step with its
//! real binary output, and gate `cargo publish` on it. crates.io is write-once,
//! so the docs and the code must agree at the exact moment of publishing.
//!
//! Usage:
//!
//!   plumb check                assert every registered claim still equals its
//!                              fixture field, and every generated doc block has
//!                              a known renderer.
//!   plumb capture              rewrite the committed fixture from a fresh binary
//!                              run, then re-render every generated doc block.
//!   plumb capture --check      assert the committed fixture is not stale, without
//!                              rewriting it (the pre-package CI gate).
//!   plumb preflight            the publish stop-sign: run every gate that must
//!                              hold at `cargo publish` and exit non-zero unless
//!                              all pass.
//!   plumb capabilities         emit plumb's own contract (verbs, exit codes,
//!                              gates, and the full error-code catalog) as JSON
//!                              on stdout. Needs no config; runs anywhere.
//!
//! A project describes itself in one JSON config (default `plumbline.json` in the
//! working directory, or `--config <path>`). The working directory is the crate
//! root; every relative path in the config resolves there, so `plumb` is invoked
//! by name inside a repo, like `br`. plumbline itself stays generic.

mod commands;
mod config;
mod diagnostic;
mod engine;
mod markers;
mod registry;

use config::Config;
use diagnostic::{codes, Diagnostic};
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // Pull an optional `--config <path>` from anywhere in the args; the rest is
    // the verb and its flags.
    let mut config_path = PathBuf::from("plumbline.json");
    let mut rest: Vec<String> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--config" {
            match it.next() {
                Some(p) => config_path = PathBuf::from(p),
                None => return fail(Diagnostic::new(codes::USAGE, "--config needs a path")),
            }
        } else {
            rest.push(a.clone());
        }
    }

    let cmd = rest.first().map(String::as_str).unwrap_or("");
    if !matches!(cmd, "check" | "capture" | "preflight" | "capabilities") {
        return fail(Diagnostic::new(
            codes::USAGE,
            "usage: plumb [--config <path>] \
             <check | capture [--check] | preflight | capabilities>",
        ));
    }

    // `capabilities` describes the tool, not a project, so it needs no config
    // and must run in any directory. Dispatch it before touching the config.
    if cmd == "capabilities" {
        return match commands::cmd_capabilities() {
            Ok(()) => ExitCode::SUCCESS,
            Err(d) => fail(d),
        };
    }

    let root = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            return fail(Diagnostic::new(
                codes::WORKDIR_UNREADABLE,
                format!("cannot determine working directory: {e}"),
            ))
        }
    };
    let cfg = match Config::load(&config_path, root) {
        Ok(c) => c,
        Err(d) => return fail(d),
    };

    let result = match cmd {
        "check" => commands::cmd_check(&cfg),
        "capture" => {
            commands::cmd_capture(&cfg, rest.get(1).map(String::as_str) == Some("--check"))
        }
        "preflight" => commands::cmd_preflight(&cfg),
        _ => unreachable!("verb already validated"),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(d) => fail(d),
    }
}

/// Print a diagnostic as `plumb: [CODE] message` and return its family's exit
/// code. The `[CODE]` prefix is what makes the contract's codes observable.
fn fail(d: Diagnostic) -> ExitCode {
    eprintln!("plumb: {d}");
    ExitCode::from(d.exit())
}
