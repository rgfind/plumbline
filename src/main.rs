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
//!   plumb release              validate a release, create an annotated version
//!                              tag, and atomically push the branch and tag.
//!                              It runs only `cargo publish --dry-run` locally.
//!   plumb capabilities         emit plumb's own contract (verbs, exit codes,
//!                              gates, and the full error-code catalog) as JSON
//!                              on stdout. Needs no config; runs anywhere.
//!   plumb --version            print the installed plumb version. Needs no
//!                              config; runs anywhere.
//!
//! A project describes itself in one JSON config (default `plumbline.json` in the
//! working directory, or `--config <path>`). The working directory is the crate
//! root; every relative path in the config resolves there, so `plumb` is invoked
//! by name inside a repo, like `br`. plumbline itself stays generic.

mod commands;
mod cli;
mod config;
mod diagnostic;
mod engine;
mod markers;
mod registry;
mod release;
mod result;

use config::Config;
use std::process::ExitCode;
use std::time::Instant;

fn main() -> ExitCode {
    let started = Instant::now();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let parsed = cli::parse(&args);
    let json_mode = parsed.as_ref().map(|invocation| invocation.json).unwrap_or_else(|_| args.iter().take_while(|arg| arg.as_str() != "--").any(|arg| arg == "--json"));
    let operation = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| parsed.and_then(|invocation| {
        if invocation.version {
            return Ok(result::CommandResult::new(serde_json::json!({"version": env!("CARGO_PKG_VERSION"), "contract_version": result::CONTRACT_VERSION}), format!("plumb {}", env!("CARGO_PKG_VERSION"))));
        }
        if invocation.help || invocation.verb.is_none() {
            return Ok(result::CommandResult::new(serde_json::json!({"help": cli::terse_help()}), cli::terse_help()));
        }
        match invocation.verb.expect("checked above") {
            cli::Verb::Capabilities => commands::cmd_capabilities(),
            cli::Verb::Schema => commands::cmd_schema(invocation.schema_command.as_deref()),
            cli::Verb::RobotDocs => {
                if invocation.robot_docs_guide { commands::cmd_robot_docs() }
                else { Err(diagnostic::Diagnostic::new(diagnostic::codes::MISSING_REQUIRED, "robot-docs requires the `guide` section")) }
            }
            verb => {
                let root = std::env::current_dir().map_err(|e| diagnostic::Diagnostic::new(diagnostic::codes::WORKDIR_UNREADABLE, format!("cannot determine working directory: {e}")))?;
                let cfg = Config::load(&invocation.config_path, root)?;
                match verb {
                    cli::Verb::Check => commands::cmd_check(&cfg),
                    cli::Verb::Capture => commands::cmd_capture(&cfg, invocation.capture_check),
                    cli::Verb::Preflight => commands::cmd_preflight(&cfg),
                    cli::Verb::Release => commands::cmd_release(&cfg),
                    cli::Verb::Capabilities => unreachable!(),
                    cli::Verb::Schema | cli::Verb::RobotDocs => unreachable!(),
                }
            }
        }
    })))
    .unwrap_or_else(|_| Err(diagnostic::Diagnostic::new(
        diagnostic::codes::INTERNAL,
        "an unexpected internal fault occurred; include this invocation and request ID in a bug report",
    )));
    ExitCode::from(result::render(operation, json_mode, started))
}
