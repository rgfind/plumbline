//! The guarded local release sequence. It has one command-runner boundary so
//! tests can prove ordering and failure handling without changing a real repo.

use crate::config::Config;
use crate::diagnostic::{codes, Diagnostic};
use crate::engine;
use serde_json::Value;
use std::io;
use std::path::Path;
use std::process::Command;

/// A completed external command. The release sequence needs only these stable
/// facts, so a test runner can provide them without starting Git or Cargo.
pub(crate) struct RunOutput {
    pub success: bool,
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// The boundary around every Git and Cargo process used for a release.
pub(crate) trait CommandRunner {
    fn run(&mut self, program: &str, args: &[String], root: &Path) -> Result<RunOutput, io::Error>;
}

pub(crate) struct SystemRunner;

impl CommandRunner for SystemRunner {
    fn run(&mut self, program: &str, args: &[String], root: &Path) -> Result<RunOutput, io::Error> {
        let output = Command::new(program)
            .args(args)
            .current_dir(root)
            .output()?;
        Ok(RunOutput {
            success: output.status.success(),
            code: output.status.code(),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}

/// Run every check in order, then create and publish the tag. `preflight` is a
/// closure because it is an in-process gate, not a child process.
pub(crate) fn run<R, F>(cfg: &Config, runner: &mut R, preflight: F) -> Result<(), Diagnostic>
where
    R: CommandRunner,
    F: FnOnce() -> Result<(), Diagnostic>,
{
    let release = cfg.release.as_ref().ok_or_else(|| {
        Diagnostic::new(
            codes::RELEASE_CONFIG_MISSING,
            "release needs a `release` object with `branch` and `remote` settings",
        )
    })?;

    ensure_clean(cfg, runner)?;
    ensure_branch(cfg, runner, &release.branch)?;
    ensure_upstream_synced(cfg, runner)?;

    let version = package_version(cfg, runner)?;
    ensure_changelog_entry(cfg, &version)?;
    let tag = format!("v{version}");
    ensure_tag_absent(cfg, runner, &tag, &release.remote)?;
    let remote_url = remote_url(cfg, runner, &release.remote)?;

    preflight()?;
    dry_run_publish(cfg, runner)?;

    run_ok(
        runner,
        "git",
        &strings(["tag", "-a", &tag, "-m", &tag]),
        &cfg.root,
        codes::TAG_CREATE_FAILED,
        "create annotated release tag",
    )?;
    run_ok(
        runner,
        "git",
        &strings([
            "push",
            "--atomic",
            &release.remote,
            &format!("HEAD:refs/heads/{}", release.branch),
            &format!("refs/tags/{tag}"),
        ]),
        &cfg.root,
        codes::PUSH_FAILED,
        "push branch and tag atomically",
    )?;

    let commit = stdout(run_ok(
        runner,
        "git",
        &strings(["rev-parse", "HEAD"]),
        &cfg.root,
        codes::GIT_UNAVAILABLE,
        "read release commit",
    )?);
    println!(
        "release: pushed {commit} and {tag} to {} ({remote_url})",
        release.remote
    );
    Ok(())
}

fn ensure_clean<R: CommandRunner>(cfg: &Config, runner: &mut R) -> Result<(), Diagnostic> {
    let out = run_ok(
        runner,
        "git",
        &strings(["status", "--porcelain"]),
        &cfg.root,
        codes::GIT_UNAVAILABLE,
        "read worktree status",
    )?;
    if stdout(out).is_empty() {
        Ok(())
    } else {
        Err(Diagnostic::new(
            codes::WORKTREE_DIRTY,
            "commit or stash changes before release",
        ))
    }
}

fn ensure_branch<R: CommandRunner>(
    cfg: &Config,
    runner: &mut R,
    expected: &str,
) -> Result<(), Diagnostic> {
    let out = invoke(
        runner,
        "git",
        &strings(["symbolic-ref", "--quiet", "--short", "HEAD"]),
        &cfg.root,
        codes::GIT_UNAVAILABLE,
        "read current branch",
    )?;
    if !out.success || stdout(out) != expected {
        return Err(Diagnostic::new(
            codes::RELEASE_BRANCH_MISMATCH,
            format!("HEAD must be attached to configured branch `{expected}`"),
        ));
    }
    Ok(())
}

fn ensure_upstream_synced<R: CommandRunner>(
    cfg: &Config,
    runner: &mut R,
) -> Result<(), Diagnostic> {
    let upstream = invoke(
        runner,
        "git",
        &strings(["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"]),
        &cfg.root,
        codes::GIT_UNAVAILABLE,
        "read branch upstream",
    )?;
    if !upstream.success {
        return Err(Diagnostic::new(
            codes::UPSTREAM_NOT_SYNCED,
            "the configured branch has no usable upstream",
        ));
    }
    let counts = invoke(
        runner,
        "git",
        &strings(["rev-list", "--left-right", "--count", "@{u}...HEAD"]),
        &cfg.root,
        codes::GIT_UNAVAILABLE,
        "count upstream differences",
    )?;
    if !counts.success || !zero_ahead_behind(&stdout(counts)) {
        return Err(Diagnostic::new(
            codes::UPSTREAM_NOT_SYNCED,
            "the configured branch must have zero ahead and behind commits",
        ));
    }
    Ok(())
}

fn package_version<R: CommandRunner>(cfg: &Config, runner: &mut R) -> Result<String, Diagnostic> {
    let out = run_ok(
        runner,
        &engine::cargo(),
        &strings(["metadata", "--no-deps", "--format-version", "1"]),
        &cfg.root,
        codes::CARGO_METADATA_FAILED,
        "read Cargo package metadata",
    )?;
    let metadata: Value = serde_json::from_slice(&out.stdout).map_err(|e| {
        Diagnostic::new(
            codes::CARGO_METADATA_FAILED,
            format!("parse cargo metadata: {e}"),
        )
    })?;
    let manifest = cfg.root.join("Cargo.toml");
    metadata["packages"]
        .as_array()
        .and_then(|packages| {
            packages.iter().find(|package| {
                package["manifest_path"]
                    .as_str()
                    .map(Path::new)
                    .is_some_and(|path| path == manifest)
            })
        })
        .and_then(|package| package["version"].as_str())
        .filter(|version| !version.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            Diagnostic::new(
                codes::CARGO_METADATA_FAILED,
                "cargo metadata has no root Cargo.toml package version",
            )
        })
}

fn ensure_changelog_entry(cfg: &Config, version: &str) -> Result<(), Diagnostic> {
    let text = std::fs::read_to_string(cfg.root.join("CHANGELOG.md")).map_err(|_| {
        Diagnostic::new(
            codes::CHANGELOG_VERSION_MISSING,
            format!("CHANGELOG.md has no H2 entry for version {version}"),
        )
    })?;
    if text
        .lines()
        .any(|line| changelog_heading_matches(line, version))
    {
        Ok(())
    } else {
        Err(Diagnostic::new(
            codes::CHANGELOG_VERSION_MISSING,
            format!("CHANGELOG.md has no H2 entry for version {version}"),
        ))
    }
}

fn ensure_tag_absent<R: CommandRunner>(
    cfg: &Config,
    runner: &mut R,
    tag: &str,
    remote: &str,
) -> Result<(), Diagnostic> {
    let local = invoke(
        runner,
        "git",
        &strings([
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/tags/{tag}"),
        ]),
        &cfg.root,
        codes::GIT_UNAVAILABLE,
        "check local release tag",
    )?;
    if local.success {
        return Err(Diagnostic::new(
            codes::TAG_EXISTS,
            format!("tag `{tag}` already exists"),
        ));
    }
    if local.code != Some(1) {
        return Err(command_failed(
            codes::GIT_UNAVAILABLE,
            "check local release tag",
            &local,
        ));
    }

    let remote_tag = format!("refs/tags/{tag}");
    let remote_out = invoke(
        runner,
        "git",
        &strings(["ls-remote", "--exit-code", "--tags", remote, &remote_tag]),
        &cfg.root,
        codes::REMOTE_UNAVAILABLE,
        "check remote release tag",
    )?;
    if remote_out.success {
        return Err(Diagnostic::new(
            codes::TAG_EXISTS,
            format!("tag `{tag}` already exists on remote `{remote}`"),
        ));
    }
    if remote_out.code == Some(2) {
        Ok(())
    } else {
        Err(command_failed(
            codes::REMOTE_UNAVAILABLE,
            "check remote release tag",
            &remote_out,
        ))
    }
}

fn remote_url<R: CommandRunner>(
    cfg: &Config,
    runner: &mut R,
    remote: &str,
) -> Result<String, Diagnostic> {
    Ok(stdout(run_ok(
        runner,
        "git",
        &strings(["remote", "get-url", remote]),
        &cfg.root,
        codes::REMOTE_UNAVAILABLE,
        "read remote URL",
    )?))
}

fn dry_run_publish<R: CommandRunner>(cfg: &Config, runner: &mut R) -> Result<(), Diagnostic> {
    let out = invoke(
        runner,
        &engine::cargo(),
        &strings(["publish", "--dry-run", "--locked"]),
        &cfg.root,
        codes::CARGO_UNAVAILABLE,
        "run cargo publish --dry-run",
    )?;
    if out.success {
        Ok(())
    } else {
        Err(command_failed(
            codes::PUBLISH_DRY_RUN_FAILED,
            "cargo publish --dry-run --locked",
            &out,
        ))
    }
}

fn invoke<R: CommandRunner>(
    runner: &mut R,
    program: &str,
    args: &[String],
    root: &Path,
    code: crate::diagnostic::Code,
    action: &str,
) -> Result<RunOutput, Diagnostic> {
    runner
        .run(program, args, root)
        .map_err(|e| Diagnostic::new(code, format!("cannot {action}: {e}")))
}

fn run_ok<R: CommandRunner>(
    runner: &mut R,
    program: &str,
    args: &[String],
    root: &Path,
    code: crate::diagnostic::Code,
    action: &str,
) -> Result<RunOutput, Diagnostic> {
    let out = invoke(runner, program, args, root, code, action)?;
    if out.success {
        Ok(out)
    } else {
        Err(command_failed(code, action, &out))
    }
}

fn command_failed(code: crate::diagnostic::Code, action: &str, out: &RunOutput) -> Diagnostic {
    Diagnostic::new(
        code,
        format!(
            "cannot {action}: exited {}{}",
            out.code.unwrap_or(-1),
            stderr(out)
                .filter(|text| !text.is_empty())
                .map(|text| format!(" ({text})"))
                .unwrap_or_default()
        ),
    )
}

fn strings<const N: usize>(items: [&str; N]) -> Vec<String> {
    items.into_iter().map(str::to_string).collect()
}

fn stdout(out: RunOutput) -> String {
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn stderr(out: &RunOutput) -> Option<String> {
    let text = String::from_utf8_lossy(&out.stderr).trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn zero_ahead_behind(text: &str) -> bool {
    let values: Vec<&str> = text.split_whitespace().collect();
    matches!(values.as_slice(), ["0", "0"])
}

fn changelog_heading_matches(line: &str, version: &str) -> bool {
    let Some(heading) = line.strip_prefix("## ") else {
        return false;
    };
    if heading == version {
        return true;
    }
    if let Some(rest) = heading.strip_prefix(version) {
        return rest.starts_with(" - ") || rest.starts_with(" — ");
    }
    let bracketed = format!("[{version}]");
    if heading == bracketed {
        return true;
    }
    heading
        .strip_prefix(&bracketed)
        .is_some_and(|rest| rest.starts_with(" - ") || rest.starts_with(" — "))
}
