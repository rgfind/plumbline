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
#[derive(Debug)]
pub(crate) struct ReleaseResult {
    pub version: String,
    pub tag: String,
    pub commit: String,
    pub remote: String,
    pub remote_url: Option<String>,
    pub already_complete: bool,
}

impl ReleaseResult {
    pub fn human(&self) -> String {
        if self.already_complete {
            format!(
                "release: {tag} from {commit} is already submitted to {remote}",
                tag = self.tag,
                commit = self.commit,
                remote = self.remote,
            )
        } else {
            format!(
                "release: pushed {commit} and {tag} to {remote} ({url})",
                commit = self.commit,
                tag = self.tag,
                remote = self.remote,
                url = self.remote_url.as_deref().unwrap_or("unknown remote URL"),
            )
        }
    }
}

pub(crate) fn run<R, F>(
    cfg: &Config,
    runner: &mut R,
    preflight: F,
) -> Result<ReleaseResult, Diagnostic>
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
    let state = release_state(cfg, runner, &tag, &release.remote, &release.branch)?;
    if state.already_complete {
        return Ok(ReleaseResult {
            version,
            tag,
            commit: state.head,
            remote: release.remote.clone(),
            remote_url: None,
            already_complete: true,
        });
    }
    let remote_url = remote_url(cfg, runner, &release.remote)?;

    preflight()?;

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

    Ok(ReleaseResult {
        version,
        tag,
        commit: state.head,
        remote: release.remote.clone(),
        remote_url: Some(remote_url),
        already_complete: false,
    })
}

pub(crate) fn run_dry<R, F>(
    cfg: &Config,
    runner: &mut R,
    preflight: F,
) -> Result<ReleaseResult, Diagnostic>
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
    let state = release_state(cfg, runner, &tag, &release.remote, &release.branch)?;
    preflight()?;
    Ok(ReleaseResult {
        version,
        tag,
        commit: state.head,
        remote: release.remote.clone(),
        remote_url: None,
        already_complete: state.already_complete,
    })
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

struct ReleaseState {
    head: String,
    already_complete: bool,
}

struct TagState {
    object: String,
    target: String,
    annotated: bool,
}

/// Read the immutable release state before a tag can be created. A retry is a
/// no-op only when the local and remote annotated tags are identical and both
/// the tag target and remote release branch point at the current commit.
fn release_state<R: CommandRunner>(
    cfg: &Config,
    runner: &mut R,
    tag: &str,
    remote: &str,
    branch: &str,
) -> Result<ReleaseState, Diagnostic> {
    let head = stdout(run_ok(
        runner,
        "git",
        &strings(["rev-parse", "HEAD"]),
        &cfg.root,
        codes::GIT_UNAVAILABLE,
        "read release commit",
    )?);
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
    let local_tag = if local.success {
        Some(local_tag_state(cfg, runner, tag)?)
    } else if local.code == Some(1) {
        None
    } else {
        return Err(command_failed(
            codes::GIT_UNAVAILABLE,
            "check local release tag",
            &local,
        ));
    };

    let remote_tag = format!("refs/tags/{tag}");
    let remote_out = invoke(
        runner,
        "git",
        &strings(["ls-remote", "--exit-code", "--tags", remote, &remote_tag]),
        &cfg.root,
        codes::REMOTE_UNAVAILABLE,
        "check remote release tag",
    )?;
    let remote_tag = if remote_out.success {
        Some(remote_tag_state(cfg, runner, tag, remote)?)
    } else if remote_out.code == Some(2) {
        None
    } else {
        Err(command_failed(
            codes::REMOTE_UNAVAILABLE,
            "check remote release tag",
            &remote_out,
        ))?
    };

    match (local_tag, remote_tag) {
        (None, None) => Ok(ReleaseState { head, already_complete: false }),
        (Some(_), None) | (None, Some(_)) => Err(Diagnostic::new(
            codes::TAG_EXISTS,
            format!(
                "tag `{tag}` exists in only one release location; inspect with `git show {tag}` and `git ls-remote --tags {remote} refs/tags/{tag}`; no overwrite is available"
            ),
        )),
        (Some(local), Some(remote_tag)) => {
            let remote_branch = remote_branch_commit(cfg, runner, remote, branch)?;
            if local.annotated
                && remote_tag.annotated
                && local.object == remote_tag.object
                && local.target == head
                && remote_tag.target == head
                && remote_branch.as_deref() == Some(head.as_str())
            {
                Ok(ReleaseState { head, already_complete: true })
            } else {
                Err(Diagnostic::new(
                    codes::RELEASE_STATE_CONFLICT,
                    format!(
                        "tag `{tag}` does not prove a completed submission from current commit `{head}`; inspect with `git show {tag}` and `git ls-remote --tags {remote} refs/tags/{tag}`; no overwrite is available"
                    ),
                ))
            }
        }
    }
}

fn local_tag_state<R: CommandRunner>(
    cfg: &Config,
    runner: &mut R,
    tag: &str,
) -> Result<TagState, Diagnostic> {
    let reference = format!("refs/tags/{tag}");
    let object = stdout(run_ok(
        runner,
        "git",
        &strings(["rev-parse", &reference]),
        &cfg.root,
        codes::GIT_UNAVAILABLE,
        "read local release tag",
    )?);
    let kind = stdout(run_ok(
        runner,
        "git",
        &strings(["cat-file", "-t", &reference]),
        &cfg.root,
        codes::GIT_UNAVAILABLE,
        "inspect local release tag",
    )?);
    let target = stdout(run_ok(
        runner,
        "git",
        &strings(["rev-list", "-n", "1", tag]),
        &cfg.root,
        codes::GIT_UNAVAILABLE,
        "read local release tag target",
    )?);
    Ok(TagState {
        object,
        target,
        annotated: kind == "tag",
    })
}

fn remote_tag_state<R: CommandRunner>(
    cfg: &Config,
    runner: &mut R,
    tag: &str,
    remote: &str,
) -> Result<TagState, Diagnostic> {
    let reference = format!("refs/tags/{tag}");
    let peeled = format!("{reference}^{{}}");
    let out = run_ok(
        runner,
        "git",
        &strings(["ls-remote", "--tags", remote, &reference, &peeled]),
        &cfg.root,
        codes::REMOTE_UNAVAILABLE,
        "inspect remote release tag",
    )?;
    let mut object = None;
    let mut target = None;
    for line in stdout(out).lines() {
        let Some((value, name)) = line.split_once('\t') else {
            continue;
        };
        if name == reference {
            object = Some(value.to_string());
        } else if name == peeled {
            target = Some(value.to_string());
        }
    }
    match (object, target) {
        (Some(object), Some(target)) => Ok(TagState { object, target, annotated: true }),
        (Some(object), None) => Ok(TagState { object: object.clone(), target: object, annotated: false }),
        (None, _) => Err(Diagnostic::new(
            codes::RELEASE_STATE_CONFLICT,
            format!("remote `{remote}` reported tag `{tag}` without a readable reference; no overwrite is available"),
        )),
    }
}

fn remote_branch_commit<R: CommandRunner>(
    cfg: &Config,
    runner: &mut R,
    remote: &str,
    branch: &str,
) -> Result<Option<String>, Diagnostic> {
    let reference = format!("refs/heads/{branch}");
    let out = invoke(
        runner,
        "git",
        &strings(["ls-remote", "--exit-code", remote, &reference]),
        &cfg.root,
        codes::REMOTE_UNAVAILABLE,
        "read remote release branch",
    )?;
    if out.success {
        Ok(stdout(out).split_whitespace().next().map(str::to_string))
    } else if out.code == Some(2) {
        Ok(None)
    } else {
        Err(command_failed(
            codes::REMOTE_UNAVAILABLE,
            "read remote release branch",
            &out,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        Capture, Generated, GitHubActions, Release, SelectionSource, Verification, VerificationGate,
    };
    use std::collections::VecDeque;
    use std::fs;
    use std::path::PathBuf;

    enum Reply {
        Output {
            success: bool,
            code: Option<i32>,
            stdout: String,
            stderr: String,
        },
    }

    struct FakeRunner {
        replies: VecDeque<Reply>,
        calls: Vec<(String, Vec<String>)>,
        local_tag_created: bool,
        remote_tag_created: bool,
    }

    impl FakeRunner {
        fn new(replies: Vec<Reply>) -> Self {
            Self {
                replies: replies.into(),
                calls: Vec::new(),
                local_tag_created: false,
                remote_tag_created: false,
            }
        }
    }

    impl CommandRunner for FakeRunner {
        fn run(
            &mut self,
            program: &str,
            args: &[String],
            _: &Path,
        ) -> Result<RunOutput, io::Error> {
            self.calls.push((program.to_string(), args.to_vec()));
            match self.replies.pop_front().expect("unexpected command") {
                Reply::Output {
                    success,
                    code,
                    stdout,
                    stderr,
                } => {
                    if success && program == "git" && args.first().is_some_and(|arg| arg == "tag") {
                        self.local_tag_created = true;
                    }
                    if success && program == "git" && args.first().is_some_and(|arg| arg == "push")
                    {
                        self.remote_tag_created = true;
                    }
                    Ok(RunOutput {
                        success,
                        code,
                        stdout: stdout.into_bytes(),
                        stderr: stderr.into_bytes(),
                    })
                }
            }
        }
    }

    /// Uses real Git against temporary repositories while it simulates Cargo.
    /// This proves tag behavior without a registry or network service.
    struct LocalRunner {
        cargo_replies: VecDeque<Reply>,
        fail_push: bool,
    }

    impl CommandRunner for LocalRunner {
        fn run(
            &mut self,
            program: &str,
            args: &[String],
            root: &Path,
        ) -> Result<RunOutput, io::Error> {
            if program == engine::cargo() {
                return match self
                    .cargo_replies
                    .pop_front()
                    .expect("unexpected cargo command")
                {
                    Reply::Output {
                        success,
                        code,
                        stdout,
                        stderr,
                    } => Ok(RunOutput {
                        success,
                        code,
                        stdout: stdout.into_bytes(),
                        stderr: stderr.into_bytes(),
                    }),
                };
            }
            if self.fail_push && program == "git" && args.first().is_some_and(|arg| arg == "push") {
                return Ok(RunOutput {
                    success: false,
                    code: Some(1),
                    stdout: Vec::new(),
                    stderr: b"simulated push failure".to_vec(),
                });
            }
            SystemRunner.run(program, args, root)
        }
    }

    fn ok(stdout: &str) -> Reply {
        Reply::Output {
            success: true,
            code: Some(0),
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }

    fn failed(code: i32) -> Reply {
        Reply::Output {
            success: false,
            code: Some(code),
            stdout: String::new(),
            stderr: "failed".into(),
        }
    }

    fn test_root(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "plumbline-release-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn config(root: PathBuf) -> Config {
        Config {
            root: root.clone(),
            path: root.join("plumbline.json"),
            selection_source: SelectionSource::CommandLine,
            document: serde_json::json!({}),
            config_hash: "test-config-hash".into(),
            capture: None::<Capture>,
            fixture: None,
            normalize_meta: Vec::new(),
            claims: Vec::new(),
            surfaces: Vec::new(),
            generated: Vec::<Generated>::new(),
            package_allowlist: "cargo-include".into(),
            release: Some(Release {
                branch: "main".into(),
                remote: "origin".into(),
                verification: Verification {
                    ci: vec![VerificationGate {
                        id: "format".into(),
                        argv: vec!["cargo".into(), "fmt".into(), "--check".into()],
                        timeout_seconds: 120,
                    }],
                    release: vec![VerificationGate {
                        id: "publish-dry-run".into(),
                        argv: vec!["cargo".into(), "publish".into(), "--dry-run".into()],
                        timeout_seconds: 600,
                    }],
                    github_actions: GitHubActions {
                        repository: "owner/repo".into(),
                        workflow_path: ".github/workflows/ci.yml".into(),
                    },
                },
            }),
        }
    }

    fn standard_replies(cfg: &Config) -> Vec<Reply> {
        let manifest = cfg.root.join("Cargo.toml");
        let metadata = format!(
            r#"{{"packages":[{{"manifest_path":"{}","version":"1.2.3"}}]}}"#,
            manifest.display()
        );
        vec![
            ok(""),
            ok("main\n"),
            ok("origin/main\n"),
            ok("0\t0\n"),
            ok(&metadata),
            ok("0123456789abcdef\n"),
            failed(1),
            failed(2),
            ok("https://example.invalid/project.git\n"),
            ok(""),
            ok(""),
            ok(""),
        ]
    }

    fn prepared(name: &str) -> (Config, Vec<Reply>) {
        let cfg = config(test_root(name));
        fs::write(
            cfg.root.join("CHANGELOG.md"),
            "# Changelog\n\n## 1.2.3 — release\n",
        )
        .unwrap();
        let replies = standard_replies(&cfg);
        (cfg, replies)
    }

    fn git(root: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn local_repository(name: &str) -> (Config, PathBuf) {
        let root = test_root(name);
        let remote = root.with_extension("remote.git");
        git(&root, &["init", "-q", "-b", "main"]);
        git(&root, &["config", "user.name", "Release Test"]);
        git(
            &root,
            &["config", "user.email", "release-test@example.invalid"],
        );
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"example\"\nversion = \"1.2.3\"\n",
        )
        .unwrap();
        fs::write(
            root.join("CHANGELOG.md"),
            "# Changelog\n\n## 1.2.3 — release\n",
        )
        .unwrap();
        git(&root, &["add", "Cargo.toml", "CHANGELOG.md"]);
        git(
            &root,
            &[
                "-c",
                "user.name=Release Test",
                "-c",
                "user.email=release-test@example.invalid",
                "commit",
                "-qm",
                "initial release candidate",
            ],
        );
        git(&root, &["init", "--bare", "-q", remote.to_str().unwrap()]);
        git(
            &root,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        git(&root, &["push", "-qu", "origin", "main"]);
        (config(root), remote)
    }

    fn local_cargo_replies(cfg: &Config) -> VecDeque<Reply> {
        let metadata = format!(
            r#"{{"packages":[{{"manifest_path":"{}","version":"1.2.3"}}]}}"#,
            cfg.root.join("Cargo.toml").display()
        );
        vec![ok(&metadata), ok("")].into()
    }

    #[test]
    fn successful_release_uses_locked_dry_run_and_atomic_push() {
        let (cfg, replies) = prepared("success");
        let mut runner = FakeRunner::new(replies);
        run(&cfg, &mut runner, || Ok(())).unwrap();

        assert!(runner.local_tag_created);
        assert!(runner.remote_tag_created);
        assert!(!runner.calls.iter().any(|(program, args)| {
            program == &engine::cargo()
                && args.first().is_some_and(|argument| argument == "publish")
        }));
        assert!(runner.calls.iter().any(|(program, args)| {
            program == "git"
                && args
                    == &[
                        "push",
                        "--atomic",
                        "origin",
                        "HEAD:refs/heads/main",
                        "refs/tags/v1.2.3",
                    ]
        }));
        assert!(runner.calls.iter().any(|(program, args)| {
            program == "git" && args == &["tag", "-a", "v1.2.3", "-m", "v1.2.3"]
        }));
        assert!(runner.calls.iter().all(|(program, args)| {
            program != &engine::cargo()
                || args.first().is_none_or(|argument| argument != "publish")
                || args.iter().any(|argument| argument == "--dry-run")
        }));
    }

    #[test]
    fn validation_failures_stop_before_tag_creation() {
        let cases: Vec<(&str, usize, Reply, crate::diagnostic::Code)> = vec![
            ("dirty", 0, ok(" M src/main.rs\n"), codes::WORKTREE_DIRTY),
            ("branch", 1, failed(1), codes::RELEASE_BRANCH_MISMATCH),
            ("upstream", 2, failed(1), codes::UPSTREAM_NOT_SYNCED),
        ];
        for (name, position, replacement, expected) in cases {
            let (cfg, mut replies) = prepared(name);
            replies[position] = replacement;
            let mut runner = FakeRunner::new(replies);
            let error = run(&cfg, &mut runner, || Ok(())).unwrap_err();
            assert_eq!(error.code.name, expected.name, "case {name}");
            assert!(!runner.local_tag_created, "case {name}");
            assert!(!runner.remote_tag_created, "case {name}");
        }
    }

    #[test]
    fn changelog_and_preflight_failures_have_stable_codes() {
        let cfg = config(test_root("missing-changelog"));
        let mut runner = FakeRunner::new(standard_replies(&cfg));
        let error = run(&cfg, &mut runner, || Ok(())).unwrap_err();
        assert_eq!(error.code.name, codes::CHANGELOG_VERSION_MISSING.name);

        let (cfg, replies) = prepared("preflight");
        let mut runner = FakeRunner::new(replies);
        let error = run(&cfg, &mut runner, || {
            Err(Diagnostic::new(codes::PREFLIGHT_FAILED, "preflight failed"))
        })
        .unwrap_err();
        assert_eq!(error.code.name, codes::PREFLIGHT_FAILED.name);
        assert!(!runner.local_tag_created);
    }

    #[test]
    fn failed_push_keeps_local_tag_and_never_records_remote_tag() {
        let (cfg, mut replies) = prepared("push-failure");
        replies[10] = failed(1);
        let mut runner = FakeRunner::new(replies);
        let error = run(&cfg, &mut runner, || Ok(())).unwrap_err();
        assert_eq!(error.code.name, codes::PUSH_FAILED.name);
        assert!(runner.local_tag_created);
        assert!(!runner.remote_tag_created);
    }

    #[test]
    fn completed_release_is_an_idempotent_no_op() {
        let (cfg, mut replies) = prepared("completed-release");
        let tag = "v1.2.3";
        replies.truncate(6);
        replies.extend([
            ok(""),
            ok("tag-object\n"),
            ok("tag\n"),
            ok("0123456789abcdef\n"),
            ok("tag-object\trefs/tags/v1.2.3\n"),
            ok("tag-object\trefs/tags/v1.2.3\n0123456789abcdef\trefs/tags/v1.2.3^{}\n"),
            ok("0123456789abcdef\trefs/heads/main\n"),
        ]);
        let mut runner = FakeRunner::new(replies);
        let result = run(&cfg, &mut runner, || {
            panic!("a completed release must skip preflight")
        })
        .unwrap();

        assert_eq!(result.tag, tag);
        assert!(result.already_complete);
        assert!(!runner.local_tag_created);
        assert!(!runner.remote_tag_created);
        assert!(!runner
            .calls
            .iter()
            .any(|(_, args)| args.first().is_some_and(|arg| arg == "push")));
    }

    #[test]
    fn local_only_release_tag_is_an_immutable_conflict() {
        let (cfg, mut replies) = prepared("local-only-tag");
        replies.truncate(6);
        replies.extend([
            ok(""),
            ok("tag-object\n"),
            ok("tag\n"),
            ok("0123456789abcdef\n"),
            failed(2),
        ]);
        let mut runner = FakeRunner::new(replies);
        let error = run(&cfg, &mut runner, || Ok(())).unwrap_err();

        assert_eq!(error.code.name, codes::TAG_EXISTS.name);
        assert_eq!(error.exit(), 5);
        assert!(error.message.contains("no overwrite is available"));
        assert!(!runner.local_tag_created);
        assert!(!runner.remote_tag_created);
    }

    #[test]
    fn mismatched_remote_tag_target_is_an_immutable_conflict() {
        let (cfg, mut replies) = prepared("mismatched-tag-target");
        replies.truncate(6);
        replies.extend([
            ok(""),
            ok("tag-object\n"),
            ok("tag\n"),
            ok("0123456789abcdef\n"),
            ok("tag-object\trefs/tags/v1.2.3\n"),
            ok("tag-object\trefs/tags/v1.2.3\nother-commit\trefs/tags/v1.2.3^{}\n"),
            ok("0123456789abcdef\trefs/heads/main\n"),
        ]);
        let mut runner = FakeRunner::new(replies);
        let error = run(&cfg, &mut runner, || Ok(())).unwrap_err();

        assert_eq!(error.code.name, codes::RELEASE_STATE_CONFLICT.name);
        assert_eq!(error.exit(), 5);
        assert!(error.message.contains("no overwrite is available"));
    }

    #[test]
    fn local_repositories_receive_an_annotated_tag_on_success() {
        let (cfg, remote) = local_repository("real-success");
        let mut runner = LocalRunner {
            cargo_replies: local_cargo_replies(&cfg),
            fail_push: false,
        };
        run(&cfg, &mut runner, || Ok(())).unwrap();

        assert_eq!(git(&cfg.root, &["cat-file", "-t", "v1.2.3"]), "tag");
        assert!(!git(
            &cfg.root,
            &[
                "--git-dir",
                remote.to_str().unwrap(),
                "show-ref",
                "--verify",
                "refs/tags/v1.2.3",
            ],
        )
        .is_empty());
    }

    #[test]
    fn real_completed_release_retry_is_a_no_op() {
        let (cfg, _) = local_repository("real-completed-retry");
        let mut first = LocalRunner {
            cargo_replies: local_cargo_replies(&cfg),
            fail_push: false,
        };
        run(&cfg, &mut first, || Ok(())).unwrap();

        let mut retry = LocalRunner {
            cargo_replies: local_cargo_replies(&cfg),
            fail_push: false,
        };
        let result = run(&cfg, &mut retry, || {
            panic!("a completed release retry must not run preflight")
        })
        .unwrap();

        assert!(result.already_complete);
        assert_eq!(result.commit, git(&cfg.root, &["rev-parse", "HEAD"]));
    }

    #[test]
    fn real_remote_only_release_tag_is_a_conflict() {
        let (cfg, _) = local_repository("real-remote-only-tag");
        let mut first = LocalRunner {
            cargo_replies: local_cargo_replies(&cfg),
            fail_push: false,
        };
        run(&cfg, &mut first, || Ok(())).unwrap();
        git(&cfg.root, &["tag", "-d", "v1.2.3"]);

        let mut retry = LocalRunner {
            cargo_replies: local_cargo_replies(&cfg),
            fail_push: false,
        };
        let error = run(&cfg, &mut retry, || Ok(())).unwrap_err();

        assert_eq!(error.code.name, codes::TAG_EXISTS.name);
        assert_eq!(error.exit(), 5);
    }

    #[test]
    fn real_push_failure_keeps_the_local_tag_out_of_the_remote() {
        let (cfg, remote) = local_repository("real-push-failure");
        let mut runner = LocalRunner {
            cargo_replies: local_cargo_replies(&cfg),
            fail_push: true,
        };
        let error = run(&cfg, &mut runner, || Ok(())).unwrap_err();
        assert_eq!(error.code.name, codes::PUSH_FAILED.name);
        assert_eq!(git(&cfg.root, &["cat-file", "-t", "v1.2.3"]), "tag");

        let output = Command::new("git")
            .args([
                "--git-dir",
                remote.to_str().unwrap(),
                "show-ref",
                "--verify",
                "--quiet",
                "refs/tags/v1.2.3",
            ])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
    }

    #[test]
    fn accepts_documented_changelog_heading_forms() {
        for line in [
            "## 1.2.3",
            "## 1.2.3 — summary",
            "## 1.2.3 - summary",
            "## [1.2.3]",
            "## [1.2.3] - 2026-09-14",
        ] {
            assert!(changelog_heading_matches(line, "1.2.3"), "{line}");
        }
        assert!(!changelog_heading_matches("## 1.2.30", "1.2.3"));
        assert!(!changelog_heading_matches("### 1.2.3", "1.2.3"));
    }
}
