//! The binary-facing engine. Everything here runs the project's own build and
//! binary: capture the contract envelope, render a doc block from a throwaway
//! sample tree, and decide which packaged files stay within the crate's include
//! allowlist. These are the generalized forms of what was hardcoded for `rf`;
//! the specifics now arrive through `Config`.

use crate::config::{Capture, Generated};
use crate::diagnostic::{codes, Diagnostic};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The freshly built binary lives at target/debug/<name>. The config names the
/// binary as the first word of each command (e.g. "rf" in ["rf","capabilities"]).
fn bin_path(root: &Path, name: &str) -> PathBuf {
    root.join("target").join("debug").join(name)
}

/// Run a build command (e.g. ["cargo","build","--bin","rf"]) in the crate root.
pub fn run_build(root: &Path, build: &[String]) -> Result<(), Diagnostic> {
    let (prog, args) = build
        .split_first()
        .ok_or_else(|| Diagnostic::new(codes::CONFIG_SCHEMA, "capture.build is empty"))?;
    let prog = if prog == "cargo" {
        cargo()
    } else {
        prog.clone()
    };
    let status = Command::new(&prog)
        .args(args)
        .current_dir(root)
        .status()
        .map_err(|e| {
            Diagnostic::new(
                codes::BUILD_FAILED,
                format!("run build `{}`: {e}", build.join(" ")),
            )
        })?;
    if status.success() {
        Ok(())
    } else {
        Err(Diagnostic::new(
            codes::BUILD_FAILED,
            format!("build `{}` failed", build.join(" ")),
        ))
    }
}

/// Build, then run the capture command under the configured env, and parse its
/// stdout as the JSON contract envelope.
pub fn capture(root: &Path, capture: &Capture) -> Result<Value, Diagnostic> {
    run_build(root, &capture.build)?;
    let (name, args) = capture
        .command
        .split_first()
        .ok_or_else(|| Diagnostic::new(codes::CONFIG_SCHEMA, "capture.command is empty"))?;
    let out = Command::new(bin_path(root, name))
        .args(args)
        .envs(&capture.env)
        .current_dir(root)
        .output()
        .map_err(|e| {
            Diagnostic::new(
                codes::CAPTURE_RUN_FAILED,
                format!("run capture command: {e}"),
            )
        })?;
    if !out.status.success() {
        return Err(Diagnostic::new(
            codes::CAPTURE_RUN_FAILED,
            format!("capture command exited {}", out.status.code().unwrap_or(-1)),
        ));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| {
        Diagnostic::new(
            codes::CAPTURE_NOT_JSON,
            format!("captured output is not valid JSON: {e}"),
        )
    })
}

/// Render one generated block from a real run of the binary. Lays down the
/// block's throwaway sample tree, runs the render command inside it under the
/// configured env, discards the tree, and returns the fenced block body
/// (optional prompt line, then the render), ready to sit between markers.
///
/// Precondition: the binary is already built. Callers build once (via
/// `run_build`) before rendering, so a batch of blocks shares one build.
pub fn render_block(root: &Path, gen: &Generated) -> Result<String, Diagnostic> {
    let tree = make_tree(&gen.tree)?;
    let (name, args) = gen
        .render
        .split_first()
        .ok_or_else(|| Diagnostic::new(codes::CONFIG_SCHEMA, "render command is empty"))?;
    let result = Command::new(bin_path(root, name))
        .args(args)
        .envs(&gen.env)
        .current_dir(&tree)
        .output()
        .map_err(|e| Diagnostic::new(codes::RENDER_FAILED, format!("run render command: {e}")));
    std::fs::remove_dir_all(&tree).ok();
    let out = result?;
    if !out.status.success() {
        return Err(Diagnostic::new(
            codes::RENDER_FAILED,
            format!("render command exited {}", out.status.code().unwrap_or(-1)),
        ));
    }
    let rendered = String::from_utf8(out.stdout)
        .map_err(|e| Diagnostic::new(codes::RENDER_FAILED, format!("render not UTF-8: {e}")))?;
    let body = rendered.trim_end_matches('\n');
    if gen.prompt.is_empty() {
        Ok(format!("```\n{body}\n```"))
    } else {
        Ok(format!("```\n{}\n{body}\n```", gen.prompt))
    }
}

/// Lay down a sample tree's declared files in a fresh temp dir, optionally as a
/// git repo (so vcs-aware behavior is live). Returns the tree root; the caller
/// removes it. Files are written with parent dirs created as needed.
fn make_tree(tree: &crate::config::SampleTree) -> Result<PathBuf, Diagnostic> {
    let dir = std::env::temp_dir().join(format!(
        "plumbline-sample-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir)
        .map_err(|e| Diagnostic::new(codes::RENDER_FAILED, format!("mkdir sample tree: {e}")))?;
    if tree.git_init {
        let init = Command::new("git")
            .args(["init", "-q"])
            .current_dir(&dir)
            .status()
            .map_err(|e| Diagnostic::new(codes::RENDER_FAILED, format!("git init: {e}")))?;
        if !init.success() {
            std::fs::remove_dir_all(&dir).ok();
            return Err(Diagnostic::new(
                codes::RENDER_FAILED,
                "git init failed in sample tree",
            ));
        }
    }
    for (name, contents) in &tree.files {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                Diagnostic::new(codes::RENDER_FAILED, format!("mkdir for {name}: {e}"))
            })?;
        }
        std::fs::write(&path, contents)
            .map_err(|e| Diagnostic::new(codes::RENDER_FAILED, format!("write {name}: {e}")))?;
    }
    Ok(dir)
}

// ---- packaged-file allowlist ----------------------------------------------

/// The files `cargo package` always ships that are cargo's own generated
/// metadata, not listed in `include`. Always allowed.
const CARGO_META: &[&str] = &[
    "Cargo.toml",
    "Cargo.toml.orig",
    "Cargo.lock",
    ".cargo_vcs_info.json",
];

/// Run `cargo package --list` and confirm every packaged file is cargo metadata
/// or matches an `include` glob from Cargo.toml. Returns a note on success, and
/// on failure the list of files that would leak into the archive.
pub fn packaged_within_allowlist(root: &Path, allowlist: &str) -> Result<String, Diagnostic> {
    let globs = match allowlist {
        "cargo-include" => cargo_include_globs(root)?,
        other => {
            return Err(Diagnostic::new(
                codes::ALLOWLIST_UNKNOWN,
                format!("unknown package_allowlist `{other}`; only `cargo-include` is supported"),
            ))
        }
    };
    let out = Command::new(cargo())
        .args(["package", "--list", "--quiet"])
        .current_dir(root)
        .output()
        .map_err(|e| {
            Diagnostic::new(
                codes::PACKAGE_LIST_FAILED,
                format!("run cargo package --list: {e}"),
            )
        })?;
    if !out.status.success() {
        return Err(Diagnostic::new(
            codes::PACKAGE_LIST_FAILED,
            format!(
                "cargo package --list failed:\n{}",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        ));
    }
    let listing = String::from_utf8_lossy(&out.stdout);
    let mut count = 0usize;
    let mut stray: Vec<String> = Vec::new();
    for f in listing.lines().map(str::trim).filter(|l| !l.is_empty()) {
        count += 1;
        if !path_is_allowed(f, &globs) {
            stray.push(f.to_string());
        }
    }
    if stray.is_empty() {
        Ok(format!("{count} file(s), all within the allowlist"))
    } else {
        Err(Diagnostic::new(
            codes::PACKAGED_LEAK,
            format!(
                "{} packaged file(s) outside the allowlist:\n{}",
                stray.len(),
                stray.join("\n")
            ),
        ))
    }
}

/// A packaged path is allowed if it is cargo metadata or matches an include glob.
fn path_is_allowed(p: &str, globs: &[String]) -> bool {
    if CARGO_META.contains(&p) {
        return true;
    }
    globs.iter().any(|g| glob_match(g, p))
}

/// Extract the `include` string array from Cargo.toml. This is a targeted read
/// of one well-known array of string literals, not a general TOML parser: it
/// keeps plumbline's single-dependency promise while making Cargo.toml the one
/// source of truth for what ships (no separate allowlist to drift).
fn cargo_include_globs(root: &Path) -> Result<Vec<String>, Diagnostic> {
    let text = std::fs::read_to_string(root.join("Cargo.toml"))
        .map_err(|e| Diagnostic::new(codes::CONFIG_SCHEMA, format!("read Cargo.toml: {e}")))?;
    let start = text
        .find("include")
        .and_then(|i| text[i..].find('[').map(|j| i + j + 1))
        .ok_or_else(|| {
            Diagnostic::new(codes::CONFIG_SCHEMA, "Cargo.toml has no `include` array")
        })?;
    let end = text[start..].find(']').map(|j| start + j).ok_or_else(|| {
        Diagnostic::new(
            codes::CONFIG_SCHEMA,
            "Cargo.toml `include` array is unterminated",
        )
    })?;
    let mut globs = Vec::new();
    for raw in text[start..end].split(',') {
        let g = raw.trim().trim_matches('"');
        if !g.is_empty() {
            globs.push(g.trim_start_matches('/').to_string());
        }
    }
    if globs.is_empty() {
        return Err(Diagnostic::new(
            codes::CONFIG_SCHEMA,
            "Cargo.toml `include` array is empty",
        ));
    }
    Ok(globs)
}

/// Match a gitignore-style glob (rooted at the package, leading slash already
/// stripped) against a package-relative path. Supports `**` (any run of path
/// segments, including none) and `*` (any run of non-slash chars in one segment).
fn glob_match(pattern: &str, path: &str) -> bool {
    let pat: Vec<&str> = pattern.split('/').collect();
    let seg: Vec<&str> = path.split('/').collect();
    seg_match(&pat, &seg)
}

fn seg_match(pat: &[&str], seg: &[&str]) -> bool {
    match pat.first() {
        None => seg.is_empty(),
        Some(&"**") => {
            // `**` matches zero or more leading segments.
            (0..=seg.len()).any(|k| seg_match(&pat[1..], &seg[k..]))
        }
        Some(p) => match seg.first() {
            Some(s) if segment_glob(p, s) => seg_match(&pat[1..], &seg[1..]),
            _ => false,
        },
    }
}

/// Match one path segment against a pattern segment where `*` is any run of
/// non-slash characters.
fn segment_glob(pat: &str, s: &str) -> bool {
    match pat.find('*') {
        None => pat == s,
        Some(_) => {
            let parts: Vec<&str> = pat.split('*').collect();
            let mut pos = 0usize;
            // First part must be a prefix.
            if let Some(first) = parts.first() {
                if !s[pos..].starts_with(first) {
                    return false;
                }
                pos += first.len();
            }
            // Last part must be a suffix.
            if let Some(last) = parts.last() {
                if parts.len() > 1 && !s[pos..].ends_with(last) {
                    return false;
                }
            }
            // Middle parts must appear in order.
            for mid in &parts[1..parts.len().saturating_sub(1)] {
                match s[pos..].find(mid) {
                    Some(i) => pos += i + mid.len(),
                    None => return false,
                }
            }
            true
        }
    }
}

fn cargo() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_matches_rf_include_shape() {
        assert!(glob_match("src/**/*.rs", "src/main.rs"));
        assert!(glob_match("src/**/*.rs", "src/verbs/find.rs"));
        assert!(glob_match("README.md", "README.md"));
        assert!(!glob_match("src/**/*.rs", "src/notes.txt"));
        assert!(!glob_match("src/**/*.rs", "xtask/src/main.rs"));
        assert!(!glob_match("README.md", "CHANGELOG.md"));
    }

    #[test]
    fn allowlist_admits_sources_docs_and_cargo_meta() {
        let globs: Vec<String> = [
            "src/**/*.rs",
            "README.md",
            "CHANGELOG.md",
            "LICENSE",
            "Cargo.toml",
            "Cargo.lock",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        for ok in [
            "Cargo.toml",
            "Cargo.toml.orig",
            "Cargo.lock",
            ".cargo_vcs_info.json",
            "README.md",
            "CHANGELOG.md",
            "LICENSE",
            "src/main.rs",
            "src/verbs/find.rs",
        ] {
            assert!(path_is_allowed(ok, &globs), "should be allowed: {ok}");
        }
        for bad in [
            "xtask/src/main.rs",
            "xtask/Cargo.toml",
            ".cargo/config.toml",
            ".github/workflows/contract-guard.yml",
            "tests/fixtures/contract/capabilities.rc.json",
            "src/notes.txt",
            "deploy/build-and-deploy.sh",
        ] {
            assert!(!path_is_allowed(bad, &globs), "should be rejected: {bad}");
        }
    }

    #[test]
    fn segment_glob_star_is_within_one_segment() {
        assert!(segment_glob("*.rs", "main.rs"));
        assert!(segment_glob("*", "anything"));
        assert!(!segment_glob("*.rs", "main.txt"));
    }
}
