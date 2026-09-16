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

/// Build through Cargo's machine message stream and select the executable that
/// Cargo itself reports for the requested binary target. This supports
/// workspaces, profiles, target directories, and platform executable suffixes.
/// Callers that need to run the built binary use the returned path, never the
/// crate root: rendering a generated block execs this file, and execing the
/// crate directory instead fails with a permission error.
pub(crate) fn resolve_capture_executable(
    root: &Path,
    capture: &Capture,
) -> Result<PathBuf, Diagnostic> {
    let (program, original) = capture
        .build
        .split_first()
        .ok_or_else(|| Diagnostic::new(codes::CONFIG_SCHEMA, "capture.build is empty"))?;
    if program != "cargo" || original.first().map(String::as_str) != Some("build") {
        return Err(Diagnostic::new(
            codes::CONFIG_SCHEMA,
            "capture.build must start with `cargo build`",
        ));
    }
    let mut arguments = original.to_vec();
    if !arguments
        .iter()
        .any(|argument| argument == "--message-format" || argument.starts_with("--message-format="))
    {
        arguments.push("--message-format=json".into());
    }
    let output = Command::new(cargo())
        .args(&arguments)
        .current_dir(root)
        .output()
        .map_err(|error| {
            Diagnostic::new(
                codes::BUILD_FAILED,
                format!("run build `{}`: {error}", capture.build.join(" ")),
            )
        })?;
    if !output.status.success() {
        return Err(Diagnostic::new(
            codes::BUILD_FAILED,
            format!("build `{}` failed", capture.build.join(" ")),
        ));
    }
    let name = capture
        .command
        .first()
        .ok_or_else(|| Diagnostic::new(codes::CONFIG_SCHEMA, "capture.command is empty"))?;
    let mut candidates = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if message["reason"] != "compiler-artifact" || message["target"]["name"] != *name {
            continue;
        }
        if !message["target"]["kind"]
            .as_array()
            .is_some_and(|kind| kind.iter().any(|kind| kind == "bin"))
        {
            continue;
        }
        if let Some(executable) = message["executable"].as_str() {
            candidates.push(PathBuf::from(executable));
        }
    }
    candidates.sort();
    candidates.dedup();
    match candidates.as_slice() {
        [executable] => Ok(executable.clone()),
        [] => Err(Diagnostic::new(codes::CAPTURE_RUN_FAILED, format!("Cargo reported no executable for capture binary `{name}`; narrow `capture.build` with `--bin {name}`"))),
        _ => Err(Diagnostic::new(codes::CAPTURE_RUN_FAILED, format!("Cargo reported ambiguous executables for capture binary `{name}`: {}; narrow `capture.build` with a package or `--bin` selector", candidates.iter().map(|path| path.display().to_string()).collect::<Vec<_>>().join(", ")))),
    }
}

/// Build, then run the capture command under the configured env, and parse its
/// stdout as the JSON contract envelope.
pub fn capture(root: &Path, capture: &Capture) -> Result<Value, Diagnostic> {
    capture_with_executable(root, capture).map(|(captured, _)| captured)
}

/// Capture the envelope and retain the Cargo-reported executable for every
/// generated renderer in the same capture batch.
pub fn capture_with_executable(
    root: &Path,
    capture: &Capture,
) -> Result<(Value, PathBuf), Diagnostic> {
    let executable = resolve_capture_executable(root, capture)?;
    let (_, args) = capture
        .command
        .split_first()
        .ok_or_else(|| Diagnostic::new(codes::CONFIG_SCHEMA, "capture.command is empty"))?;
    let out = Command::new(&executable)
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
    let captured = serde_json::from_slice(&out.stdout).map_err(|e| {
        Diagnostic::new(
            codes::CAPTURE_NOT_JSON,
            format!("captured output is not valid JSON: {e}"),
        )
    })?;
    Ok((captured, executable))
}

/// Render one generated block from a real run of the binary. Lays down the
/// block's throwaway sample tree, runs the render command inside it under the
/// configured env, discards the tree, and returns the fenced block body
/// (optional prompt line, then the render), ready to sit between markers.
///
/// Precondition: `executable` is the built binary, from
/// `resolve_capture_executable`, which builds once so a batch of blocks shares
/// one build. Pass that resolved path, never the crate root.
pub fn render_block(executable: &Path, gen: &Generated) -> Result<String, Diagnostic> {
    let tree = make_tree(&gen.tree)?;
    let (_, args) = gen
        .render
        .split_first()
        .ok_or_else(|| Diagnostic::new(codes::CONFIG_SCHEMA, "render command is empty"))?;
    let result = Command::new(executable)
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
    let (include, exclude) = match allowlist {
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
        if !path_is_allowed(f, &include, &exclude) {
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
fn path_is_allowed(p: &str, include: &[String], exclude: &[String]) -> bool {
    if CARGO_META.contains(&p) {
        return true;
    }
    include.iter().any(|g| glob_match(g, p)) && !exclude.iter().any(|g| glob_match(g, p))
}

/// Extract the `include` string array from Cargo.toml. This is a targeted read
/// of one well-known array of string literals, not a general TOML parser: it
/// keeps plumbline's single-dependency promise while making Cargo.toml the one
/// source of truth for what ships (no separate allowlist to drift).
fn cargo_include_globs(root: &Path) -> Result<(Vec<String>, Vec<String>), Diagnostic> {
    let text = std::fs::read_to_string(root.join("Cargo.toml"))
        .map_err(|e| Diagnostic::new(codes::CONFIG_SCHEMA, format!("read Cargo.toml: {e}")))?;
    let manifest: toml::Value = toml::from_str(&text).map_err(|e| {
        Diagnostic::new(
            codes::CONFIG_SCHEMA,
            format!("Cargo.toml is invalid TOML: {e}"),
        )
    })?;
    let package = manifest
        .get("package")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| {
            Diagnostic::new(codes::CONFIG_SCHEMA, "Cargo.toml has no [package] table")
        })?;
    let include = toml_string_array(package.get("include"), "package.include")?;
    let exclude = toml_string_array(package.get("exclude"), "package.exclude")?;
    if include.is_empty() {
        return Err(Diagnostic::new(
            codes::CONFIG_SCHEMA,
            "Cargo.toml `include` array is empty",
        ));
    }
    Ok((include, exclude))
}

fn toml_string_array(value: Option<&toml::Value>, field: &str) -> Result<Vec<String>, Diagnostic> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let values = value.as_array().ok_or_else(|| {
        Diagnostic::new(
            codes::CONFIG_SCHEMA,
            format!("Cargo.toml `{field}` must be an array of strings"),
        )
    })?;
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(|value| value.trim_start_matches('/').to_string())
                .ok_or_else(|| {
                    Diagnostic::new(
                        codes::CONFIG_SCHEMA,
                        format!("Cargo.toml `{field}` must contain only strings"),
                    )
                })
        })
        .collect()
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

pub(crate) fn cargo() -> String {
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
            assert!(path_is_allowed(ok, &globs, &[]), "should be allowed: {ok}");
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
            assert!(
                !path_is_allowed(bad, &globs, &[]),
                "should be rejected: {bad}"
            );
        }
    }

    #[test]
    fn segment_glob_star_is_within_one_segment() {
        assert!(segment_glob("*.rs", "main.rs"));
        assert!(segment_glob("*", "anything"));
        assert!(!segment_glob("*.rs", "main.txt"));
    }

    #[test]
    fn manifest_arrays_use_a_toml_parser_and_apply_excludes() {
        let manifest: toml::Value = toml::from_str(
            "[package]\ninclude = [\n  \"src/**/*.rs\", # source\n  \"README.md\",\n]\nexclude = [\"src/private/**\"]\n",
        )
        .unwrap();
        let package = manifest
            .get("package")
            .and_then(toml::Value::as_table)
            .unwrap();
        let include = toml_string_array(package.get("include"), "package.include").unwrap();
        let exclude = toml_string_array(package.get("exclude"), "package.exclude").unwrap();
        assert!(path_is_allowed("src/main.rs", &include, &exclude));
        assert!(!path_is_allowed("src/private/key.rs", &include, &exclude));
        assert!(
            toml_string_array(Some(&toml::Value::String("bad".into())), "package.include").is_err()
        );
    }
}
