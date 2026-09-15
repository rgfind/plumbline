//! The project config. One JSON file per crate tells plumbline everything
//! rf-specific: how to build and capture the binary, which contract fixture to
//! check, which doc surfaces carry generated blocks, and how to render each
//! block from a throwaway sample tree. plumbline itself stays generic; the
//! config is the only place a project's own facts live.

use crate::diagnostic::{codes, Diagnostic};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// The whole config, loaded from a single JSON file.
pub struct Config {
    /// The crate root: the directory `plumb` runs in. All relative paths in the
    /// config resolve here, so the config file itself can live anywhere.
    pub root: PathBuf,
    /// The exact file selected for this command.
    pub path: PathBuf,
    /// The selection rule that chose `path`.
    pub selection_source: SelectionSource,
    /// The parsed effective JSON document. Config commands expose and edit it.
    pub document: Value,
    /// SHA-256 of canonical JSON for `document`.
    pub config_hash: String,
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
    /// Settings that identify the branch and remote a release may publish.
    /// Absent is valid for commands that do not release a crate.
    pub release: Option<Release>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionSource {
    CommandLine,
    Environment,
    Default,
}

impl SelectionSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CommandLine => "command_line",
            Self::Environment => "environment",
            Self::Default => "default",
        }
    }
}

/// The repository destination a release command is allowed to use.
pub struct Release {
    pub branch: String,
    pub remote: String,
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

        Self::from_document(path.to_path_buf(), root, SelectionSource::CommandLine, doc)
    }

    /// Select one configuration file. An explicit command flag has precedence
    /// over `PLUMB_CONFIG`, which has precedence over `./plumbline.json`.
    pub fn load_selected(explicit: Option<&Path>, cwd: PathBuf) -> Result<Config, Diagnostic> {
        let (candidate, source) = if let Some(path) = explicit {
            (path.to_path_buf(), SelectionSource::CommandLine)
        } else if let Some(path) = std::env::var_os("PLUMB_CONFIG") {
            (PathBuf::from(path), SelectionSource::Environment)
        } else {
            (PathBuf::from("plumbline.json"), SelectionSource::Default)
        };
        let path = if candidate.is_absolute() {
            candidate
        } else {
            cwd.join(candidate)
        };
        let text = std::fs::read_to_string(&path).map_err(|e| {
            Diagnostic::new(
                codes::CONFIG_UNREADABLE,
                format!("cannot read config `{}`: {e}", path.display()),
            )
        })?;
        let doc = serde_json::from_str(&text).map_err(|e| {
            Diagnostic::new(
                codes::CONFIG_INVALID_JSON,
                format!("config `{}` is not valid JSON: {e}", path.display()),
            )
        })?;
        let root = path.parent().unwrap_or(&cwd).to_path_buf();
        Self::from_document(path, root, source, doc)
    }

    fn from_document(
        path: PathBuf,
        root: PathBuf,
        selection_source: SelectionSource,
        doc: Value,
    ) -> Result<Config, Diagnostic> {
        if !doc.is_object() {
            return Err(Diagnostic::new(
                codes::CONFIG_SCHEMA,
                "the configuration root must be an object",
            ));
        }
        validate_top_level(&doc)?;

        // A `capture` block is optional. When present it must be complete
        // (build + command); a declared-but-empty block is an error, not "no
        // capture". Absent means the crate has no contract to capture.
        let capture = match doc.get("capture") {
            Some(c) => Some(Capture {
                build: str_vec(&c["build"], "capture.build")?,
                command: str_vec(&c["command"], "capture.command")?,
                env: str_map(&c["env"], "capture.env")?,
            }),
            None => None,
        };

        let fixture = optional_string(&doc, "fixture")?;
        let normalize_meta = optional_str_vec(&doc, "normalize_meta")?;
        let claims = optional_array(&doc, "claims")?;
        let surfaces = optional_str_vec(&doc, "surfaces")?;

        let mut generated = Vec::new();
        if let Some(items) = doc.get("generated").and_then(Value::as_array) {
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

        let package_allowlist = optional_string(&doc, "package_allowlist")?
            .unwrap_or_else(|| "cargo-include".to_string());

        // Release settings are optional so existing project configurations stay
        // valid. If a project declares this block, however, its destination
        // must be complete and explicit.
        let release = match doc.get("release") {
            None => None,
            Some(value) => {
                if !value.is_object() {
                    return Err(Diagnostic::new(
                        codes::CONFIG_SCHEMA,
                        "`release` must be an object",
                    ));
                }
                Some(Release {
                    branch: req_nonempty_str(&value["branch"], "release.branch")?,
                    remote: req_nonempty_str(&value["remote"], "release.remote")?,
                })
            }
        };

        let config_hash = canonical_hash(&doc);
        Ok(Config {
            root,
            path,
            selection_source,
            document: doc,
            config_hash,
            capture,
            fixture,
            normalize_meta,
            claims,
            surfaces,
            generated,
            package_allowlist,
            release,
        })
    }

    pub fn validate_document(
        path: PathBuf,
        root: PathBuf,
        selection_source: SelectionSource,
        document: Value,
    ) -> Result<Config, Diagnostic> {
        Self::from_document(path, root, selection_source, document)
    }

    /// Return the source of the top-level field addressed by a JSON pointer.
    /// Values inherit the provenance of their top-level configuration field.
    pub fn provenance(&self, pointer: &str) -> &'static str {
        let top = pointer
            .trim_start_matches('/')
            .split('/')
            .next()
            .unwrap_or("");
        if self.document.get(top).is_some() {
            "file"
        } else {
            "default"
        }
    }
}

const TOP_LEVEL_FIELDS: &[&str] = &[
    "capture",
    "claims",
    "config_version",
    "fixture",
    "generated",
    "normalize_meta",
    "package_allowlist",
    "release",
    "surfaces",
];

fn validate_top_level(doc: &Value) -> Result<(), Diagnostic> {
    let fields = doc.as_object().expect("object was checked");
    for key in fields.keys() {
        if !TOP_LEVEL_FIELDS.contains(&key.as_str()) {
            return Err(Diagnostic::new(
                codes::CONFIG_SCHEMA,
                format!(
                    "unknown config field `/{key}`; legal fields are {}",
                    TOP_LEVEL_FIELDS.join(", ")
                ),
            ));
        }
    }
    if let Some(version) = fields.get("config_version") {
        if version.as_u64() != Some(1) {
            return Err(Diagnostic::new(
                codes::CONFIG_SCHEMA,
                "`config_version` must be integer 1",
            ));
        }
    }
    if let Some(value) = fields.get("capture") {
        if !value.is_object() {
            return Err(Diagnostic::new(
                codes::CONFIG_SCHEMA,
                "`capture` must be an object",
            ));
        }
    }
    if let Some(value) = fields.get("generated") {
        if !value.is_array() {
            return Err(Diagnostic::new(
                codes::CONFIG_SCHEMA,
                "`generated` must be an array",
            ));
        }
    }
    Ok(())
}

fn optional_string(doc: &Value, field: &str) -> Result<Option<String>, Diagnostic> {
    match doc.get(field) {
        None => Ok(None),
        Some(value) => value.as_str().map(str::to_string).map(Some).ok_or_else(|| {
            Diagnostic::new(codes::CONFIG_SCHEMA, format!("`{field}` must be a string"))
        }),
    }
}

fn optional_array(doc: &Value, field: &str) -> Result<Vec<Value>, Diagnostic> {
    match doc.get(field) {
        None => Ok(Vec::new()),
        Some(value) => value.as_array().cloned().ok_or_else(|| {
            Diagnostic::new(codes::CONFIG_SCHEMA, format!("`{field}` must be an array"))
        }),
    }
}

fn optional_str_vec(doc: &Value, field: &str) -> Result<Vec<String>, Diagnostic> {
    match doc.get(field) {
        None => Ok(Vec::new()),
        Some(value) => str_vec(value, field),
    }
}

pub fn canonical_json(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => serde_json::to_string(value).expect("serialize JSON string"),
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Value::Object(values) => {
            let mut entries = values.iter().collect::<Vec<_>>();
            entries.sort_by(|(left, _), (right, _)| left.cmp(right));
            format!(
                "{{{}}}",
                entries
                    .into_iter()
                    .map(|(key, value)| format!(
                        "{}:{}",
                        serde_json::to_string(key).expect("serialize JSON key"),
                        canonical_json(value)
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
    }
}

pub fn canonical_hash(value: &Value) -> String {
    format!("{:x}", Sha256::digest(canonical_json(value).as_bytes()))
}

pub fn config_schema() -> Value {
    serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "properties": {
            "config_version": {"type": "integer", "const": 1},
            "capture": {"type": "object"}, "fixture": {"type": "string"},
            "normalize_meta": {"type": "array", "items": {"type": "string"}},
            "claims": {"type": "array"}, "surfaces": {"type": "array", "items": {"type": "string"}},
            "generated": {"type": "array"}, "package_allowlist": {"type": "string", "enum": ["cargo-include"]},
            "release": {"type": "object"}
        },
        "additionalProperties": false
    })
}

pub fn pointer_get<'a>(value: &'a Value, pointer: &str) -> Result<&'a Value, Diagnostic> {
    if pointer.is_empty() {
        return Ok(value);
    }
    if !pointer.starts_with('/') {
        return Err(Diagnostic::new(
            codes::INVALID_INPUT,
            "JSON Pointer must be empty or start with `/`",
        ));
    }
    let mut current = value;
    for raw in pointer.trim_start_matches('/').split('/') {
        let token = decode_pointer_token(raw)?;
        current = match current {
            Value::Object(map) => map.get(&token),
            Value::Array(values) => token
                .parse::<usize>()
                .ok()
                .and_then(|index| values.get(index)),
            _ => None,
        }
        .ok_or_else(|| {
            Diagnostic::new(
                codes::NOT_FOUND,
                format!("JSON Pointer `{pointer}` does not exist"),
            )
        })?;
    }
    Ok(current)
}

pub fn pointer_set(value: &mut Value, pointer: &str, replacement: Value) -> Result<(), Diagnostic> {
    if pointer.is_empty() {
        *value = replacement;
        return Ok(());
    }
    let (parent, token) = pointer
        .rsplit_once('/')
        .ok_or_else(|| Diagnostic::new(codes::INVALID_INPUT, "JSON Pointer must start with `/`"))?;
    let parent = pointer_get_mut(value, parent)?;
    let token = decode_pointer_token(token)?;
    match parent {
        Value::Object(map) => {
            map.insert(token, replacement);
            Ok(())
        }
        Value::Array(items) => {
            let index = token.parse::<usize>().map_err(|_| {
                Diagnostic::new(
                    codes::INVALID_INPUT,
                    "JSON Pointer array index must be a non-negative integer",
                )
            })?;
            if index >= items.len() {
                return Err(Diagnostic::new(
                    codes::NOT_FOUND,
                    format!("JSON Pointer `{pointer}` does not exist"),
                ));
            }
            items[index] = replacement;
            Ok(())
        }
        _ => Err(Diagnostic::new(
            codes::INVALID_INPUT,
            "JSON Pointer parent is not a container",
        )),
    }
}

fn pointer_get_mut<'a>(value: &'a mut Value, pointer: &str) -> Result<&'a mut Value, Diagnostic> {
    if pointer.is_empty() {
        return Ok(value);
    }
    if !pointer.starts_with('/') {
        return Err(Diagnostic::new(
            codes::INVALID_INPUT,
            "JSON Pointer must be empty or start with `/`",
        ));
    }
    let mut current = value;
    for raw in pointer.trim_start_matches('/').split('/') {
        let token = decode_pointer_token(raw)?;
        current = match current {
            Value::Object(map) => map.get_mut(&token),
            Value::Array(values) => token
                .parse::<usize>()
                .ok()
                .and_then(|index| values.get_mut(index)),
            _ => None,
        }
        .ok_or_else(|| {
            Diagnostic::new(
                codes::NOT_FOUND,
                format!("JSON Pointer `{pointer}` does not exist"),
            )
        })?;
    }
    Ok(current)
}

fn decode_pointer_token(raw: &str) -> Result<String, Diagnostic> {
    let mut decoded = String::new();
    let mut chars = raw.chars();
    while let Some(character) = chars.next() {
        if character != '~' {
            decoded.push(character);
            continue;
        }
        match chars.next() {
            Some('0') => decoded.push('~'),
            Some('1') => decoded.push('/'),
            _ => {
                return Err(Diagnostic::new(
                    codes::INVALID_INPUT,
                    "JSON Pointer escape must be `~0` or `~1`",
                ))
            }
        }
    }
    Ok(decoded)
}

pub fn merge_patch(target: &mut Value, patch: &Value) {
    let Value::Object(patch) = patch else {
        *target = patch.clone();
        return;
    };
    if !target.is_object() {
        *target = Value::Object(serde_json::Map::new());
    }
    let target = target.as_object_mut().expect("object was installed");
    for (key, value) in patch {
        if value.is_null() {
            target.remove(key);
        } else {
            merge_patch(target.entry(key.clone()).or_insert(Value::Null), value);
        }
    }
}

pub struct ConfigLock {
    path: PathBuf,
}

impl ConfigLock {
    pub fn acquire(config_path: &Path) -> Result<Self, Diagnostic> {
        let name = config_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("plumbline.json");
        let path = config_path.with_file_name(format!(".{name}.plumb.lock"));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(_) => Ok(Self { path }),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Err(Diagnostic::new(codes::LOCKED, format!("config write lock is busy; retry after removing `{}` only if no writer is active", path.display()))),
            Err(error) => Err(Diagnostic::new(codes::WRITE_FAILED, format!("create config write lock: {error}"))),
        }
    }
}

impl Drop for ConfigLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

pub fn write_document(path: &Path, value: &Value) -> Result<(), Diagnostic> {
    let text = format!(
        "{}\n",
        serde_json::to_string_pretty(value).expect("serialize config")
    );
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("plumbline.json");
    let temp = path.with_file_name(format!(".{name}.plumb-{}.tmp", std::process::id()));
    let mut file = std::fs::File::create(&temp).map_err(|e| {
        Diagnostic::new(codes::WRITE_FAILED, format!("create temporary config: {e}"))
    })?;
    file.write_all(text.as_bytes()).map_err(|e| {
        Diagnostic::new(codes::WRITE_FAILED, format!("write temporary config: {e}"))
    })?;
    file.sync_all()
        .map_err(|e| Diagnostic::new(codes::WRITE_FAILED, format!("sync temporary config: {e}")))?;
    std::fs::rename(&temp, path)
        .map_err(|e| Diagnostic::new(codes::WRITE_FAILED, format!("replace config: {e}")))
}

pub fn read_stdin_patch() -> Result<Value, Diagnostic> {
    let mut text = String::new();
    std::io::stdin().read_to_string(&mut text).map_err(|e| {
        Diagnostic::new(codes::INVALID_INPUT, format!("read patch from stdin: {e}"))
    })?;
    serde_json::from_str(&text).map_err(|e| {
        Diagnostic::new(
            codes::INVALID_INPUT,
            format!("stdin is not valid JSON Merge Patch: {e}"),
        )
    })
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

fn req_nonempty_str(v: &Value, field: &str) -> Result<String, Diagnostic> {
    let value = req_str(v, field)?;
    if value.trim().is_empty() {
        return Err(Diagnostic::new(
            codes::CONFIG_SCHEMA,
            format!("`{field}` must not be empty"),
        ));
    }
    Ok(value)
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
              "package_allowlist": "cargo-include",
              "release": {"branch":"main","remote":"origin"}
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
        let release = cfg.release.as_ref().unwrap();
        assert_eq!(release.branch, "main");
        assert_eq!(release.remote, "origin");
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
        assert!(cfg.release.is_none());
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

    #[test]
    fn release_requires_an_object_with_nonempty_branch_and_remote() {
        for (name, body) in [
            ("release-scalar.json", r#"{"release":"origin"}"#),
            (
                "release-missing-branch.json",
                r#"{"release":{"remote":"origin"}}"#,
            ),
            (
                "release-missing-remote.json",
                r#"{"release":{"branch":"main"}}"#,
            ),
            (
                "release-mistyped.json",
                r#"{"release":{"branch":true,"remote":"origin"}}"#,
            ),
            (
                "release-empty.json",
                r#"{"release":{"branch":"","remote":" "}}"#,
            ),
        ] {
            let p = write_tmp(name, body);
            let error = match Config::load(&p, PathBuf::from(".")) {
                Ok(_) => panic!("{name} unexpectedly loaded"),
                Err(error) => error,
            };
            assert_eq!(error.code.name, "CONFIG_SCHEMA");
        }
    }
}
