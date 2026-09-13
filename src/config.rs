//! The project config. One JSON file per crate tells plumbline everything
//! rf-specific: how to build and capture the binary, which contract fixture to
//! check, which doc surfaces carry generated blocks, and how to render each
//! block from a throwaway sample tree. plumbline itself stays generic; the
//! config is the only place a project's own facts live.

use crate::diagnostic::{codes, Diagnostic};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The whole config, loaded from a single JSON file.
pub struct Config {
    /// The crate root: the directory `plumb` runs in. All relative paths in the
    /// config resolve here, so the config file itself can live anywhere.
    pub root: PathBuf,
    /// How to build and capture the contract envelope. `None` when the crate
    /// has no machine-readable contract to capture (plumbline itself is such a
    /// crate); the contract-dependent gates are then skipped.
    pub capture: Option<Capture>,
    /// Path to the committed contract fixture, relative to `root`. `None` when
    /// the crate declares no fixture.
    pub fixture: Option<String>,
    /// Meta fields dropped before comparing two captures (volatile ids, times).
    pub normalize_meta: Vec<String>,
    /// Registry claims checked against the fixture.
    pub claims: Vec<Value>,
    /// Doc files scanned for generated blocks.
    pub surfaces: Vec<String>,
    /// Each generated block: its id, its surface, and how to render it.
    pub generated: Vec<Generated>,
    /// Source of the packaged-file allowlist. Only "cargo-include" for now:
    /// derive it from Cargo.toml's `include`.
    pub package_allowlist: String,
}

/// How to produce the contract envelope from the built binary.
pub struct Capture {
    pub build: Vec<String>,
    pub command: Vec<String>,
    pub env: BTreeMap<String, String>,
}

/// One generated doc block and the recipe to render it.
pub struct Generated {
    pub id: String,
    pub surface: String,
    pub render: Vec<String>,
    pub env: BTreeMap<String, String>,
    /// A shell-prompt line printed above the render (e.g. "$ rf content .").
    /// Empty means no prompt line.
    pub prompt: String,
    /// The throwaway tree the render command runs against.
    pub tree: SampleTree,
}

/// A throwaway directory built fresh for one render, then discarded. Declaring
/// files inline keeps the sample reproducible: same bytes, same output, forever.
pub struct SampleTree {
    pub git_init: bool,
    /// Relative path -> file contents. Parent dirs are created as needed.
    pub files: BTreeMap<String, String>,
}

impl Config {
    /// Load and validate a config file, resolving its relative paths against
    /// `root` (the directory `plumb` runs in). Returns a human-readable error on
    /// any missing or mistyped field, so a bad config fails loud, not silently.
    pub fn load(path: &Path, root: PathBuf) -> Result<Config, Diagnostic> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            Diagnostic::new(
                codes::CONFIG_UNREADABLE,
                format!("cannot read config `{}`: {e}", path.display()),
            )
        })?;
        let doc: Value = serde_json::from_str(&text).map_err(|e| {
            Diagnostic::new(
                codes::CONFIG_INVALID_JSON,
                format!("config `{}` is not valid JSON: {e}", path.display()),
            )
        })?;

        // A `capture` block is optional. When present it must be complete
        // (build + command); a declared-but-empty block is an error, not "no
        // capture". Absent means the crate has no contract to capture.
        let capture = match doc.get("capture") {
            Some(c) if c.is_object() => Some(Capture {
                build: str_vec(&c["build"], "capture.build")?,
                command: str_vec(&c["command"], "capture.command")?,
                env: str_map(&c["env"], "capture.env")?,
            }),
            _ => None,
        };

        let fixture = doc["fixture"].as_str().map(str::to_string);
        let normalize_meta = opt_str_vec(&doc["normalize_meta"]);
        let claims = doc["claims"].as_array().cloned().unwrap_or_default();
        let surfaces = opt_str_vec(&doc["surfaces"]);

        let mut generated = Vec::new();
        if let Some(items) = doc["generated"].as_array() {
            for (i, g) in items.iter().enumerate() {
                generated.push(parse_generated(g, i)?);
            }
        }

        // Coherence: a partial config must fail loud, not silently do nothing.
        // Claims are checked against the fixture, so claims need a fixture.
        // Generated blocks render from the binary, so blocks need a capture.
        if !claims.is_empty() && fixture.is_none() {
            return Err(Diagnostic::new(
                codes::CONFIG_INCOHERENT,
                "`claims` are declared but `fixture` is missing; \
                 claims are checked against the fixture",
            ));
        }
        if !generated.is_empty() && capture.is_none() {
            return Err(Diagnostic::new(
                codes::CONFIG_INCOHERENT,
                "`generated` blocks are declared but `capture` is missing; \
                 blocks render from the built binary",
            ));
        }

        let package_allowlist = doc["package_allowlist"]
            .as_str()
            .unwrap_or("cargo-include")
            .to_string();

        Ok(Config {
            root,
            capture,
            fixture,
            normalize_meta,
            claims,
            surfaces,
            generated,
            package_allowlist,
        })
    }
}

fn parse_generated(g: &Value, i: usize) -> Result<Generated, Diagnostic> {
    let where_ = format!("generated[{i}]");
    let tree = &g["tree"];
    let mut files = BTreeMap::new();
    if let Some(map) = tree["files"].as_object() {
        for (name, content) in map {
            let c = content.as_str().ok_or_else(|| {
                Diagnostic::new(
                    codes::CONFIG_SCHEMA,
                    format!("{where_}.tree.files[{name}] must be a string"),
                )
            })?;
            files.insert(name.clone(), c.to_string());
        }
    }
    Ok(Generated {
        id: req_str(&g["id"], &format!("{where_}.id"))?,
        surface: req_str(&g["surface"], &format!("{where_}.surface"))?,
        render: str_vec(&g["render"], &format!("{where_}.render"))?,
        env: str_map(&g["env"], &format!("{where_}.env"))?,
        prompt: g["prompt"].as_str().unwrap_or("").to_string(),
        tree: SampleTree {
            git_init: tree["git_init"].as_bool().unwrap_or(false),
            files,
        },
    })
}

fn req_str(v: &Value, field: &str) -> Result<String, Diagnostic> {
    v.as_str()
        .map(str::to_string)
        .ok_or_else(|| Diagnostic::new(codes::CONFIG_SCHEMA, format!("`{field}` must be a string")))
}

fn str_vec(v: &Value, field: &str) -> Result<Vec<String>, Diagnostic> {
    let arr = v.as_array().ok_or_else(|| {
        Diagnostic::new(
            codes::CONFIG_SCHEMA,
            format!("`{field}` must be an array of strings"),
        )
    })?;
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        out.push(
            item.as_str()
                .ok_or_else(|| {
                    Diagnostic::new(
                        codes::CONFIG_SCHEMA,
                        format!("`{field}` has a non-string element"),
                    )
                })?
                .to_string(),
        );
    }
    Ok(out)
}

fn opt_str_vec(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn str_map(v: &Value, field: &str) -> Result<BTreeMap<String, String>, Diagnostic> {
    if v.is_null() {
        return Ok(BTreeMap::new());
    }
    let obj = v.as_object().ok_or_else(|| {
        Diagnostic::new(
            codes::CONFIG_SCHEMA,
            format!("`{field}` must be an object of string values"),
        )
    })?;
    let mut out = BTreeMap::new();
    for (k, val) in obj {
        out.insert(
            k.clone(),
            val.as_str()
                .ok_or_else(|| {
                    Diagnostic::new(
                        codes::CONFIG_SCHEMA,
                        format!("`{field}[{k}]` must be a string"),
                    )
                })?
                .to_string(),
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_tmp(name: &str, body: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("plumbline-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn loads_a_full_config() {
        let p = write_tmp(
            "full.json",
            r#"{
              "capture": {"build":["cargo","build"],"command":["rf","capabilities","--json"],
                          "env":{"SOURCE_DATE_EPOCH":"0"}},
              "fixture": "tests/fixtures/contract/capabilities.rc.json",
              "normalize_meta": ["request_id"],
              "claims": [{"id":"x","fixture_path":"a","mode":"value","expected":1}],
              "surfaces": ["README.md"],
              "generated": [
                {"id":"readme-example","surface":"README.md",
                 "render":["rf","content","."],"env":{"NO_COLOR":"1"},
                 "prompt":"$ rf content .",
                 "tree":{"git_init":true,"files":{"a.py":"x = 1\n"}}}
              ],
              "package_allowlist": "cargo-include"
            }"#,
        );
        let cfg = Config::load(&p, PathBuf::from(".")).unwrap();
        assert_eq!(
            cfg.capture.as_ref().unwrap().command,
            ["rf", "capabilities", "--json"]
        );
        assert_eq!(
            cfg.fixture.as_deref(),
            Some("tests/fixtures/contract/capabilities.rc.json")
        );
        assert_eq!(cfg.normalize_meta, ["request_id"]);
        assert_eq!(cfg.claims.len(), 1);
        assert_eq!(cfg.generated.len(), 1);
        let g = &cfg.generated[0];
        assert_eq!(g.id, "readme-example");
        assert!(g.tree.git_init);
        assert_eq!(g.tree.files["a.py"], "x = 1\n");
        assert_eq!(cfg.package_allowlist, "cargo-include");
    }

    #[test]
    fn declared_but_empty_capture_errors() {
        // A `capture` block that is present must be complete; empty is a
        // mistake, not "no contract".
        let p = write_tmp("bad.json", r#"{"capture":{},"surfaces":[]}"#);
        assert!(Config::load(&p, PathBuf::from(".")).is_err());
    }

    #[test]
    fn contract_less_config_loads() {
        // A crate with no JSON contract (plumbline itself) declares neither
        // capture nor fixture, and still loads.
        let p = write_tmp("minimal.json", r#"{"surfaces":["README.md"]}"#);
        let cfg = Config::load(&p, PathBuf::from(".")).unwrap();
        assert!(cfg.capture.is_none());
        assert!(cfg.fixture.is_none());
        assert!(cfg.claims.is_empty());
        assert_eq!(cfg.surfaces, ["README.md"]);
    }

    #[test]
    fn claims_without_fixture_errors() {
        let p = write_tmp(
            "claims-no-fixture.json",
            r#"{"claims":[{"id":"x","fixture_path":"a","mode":"value","expected":1}]}"#,
        );
        assert!(Config::load(&p, PathBuf::from(".")).is_err());
    }

    #[test]
    fn generated_without_capture_errors() {
        let p = write_tmp(
            "gen-no-capture.json",
            r#"{"surfaces":["README.md"],
                "generated":[{"id":"x","surface":"README.md","render":["dummy"],"tree":{}}]}"#,
        );
        assert!(Config::load(&p, PathBuf::from(".")).is_err());
    }
}
