//! Diagnostics carry a stable code, so every failure the tool emits names
//! itself. The catalog (see `codes`) binds each code to its family in one
//! place, so a call site cannot pair a code with the wrong family. This is the
//! machine-readable half of CONTRACT.md; the families are the stable promise
//! and the exit codes follow from them.

use std::fmt;

/// The nature of a fault, grouped by what a consumer must react to. The family
/// is the stable contract surface; leaf codes are refinements inside it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Family {
    /// Bad invocation. The only family that exits 2.
    Usage,
    /// The config file is wrong.
    Config,
    /// The host, filesystem, or external tooling failed a precondition.
    Env,
    /// The crate's own build or binary failed to produce what was asked.
    Capture,
    /// A guard verdict: the crate is out of lockstep and must not publish.
    Gate,
}

impl Family {
    /// The family's contract string (`"GATE"`, `"CONFIG"`, ...), emitted by the
    /// `capabilities` envelope.
    pub fn as_str(self) -> &'static str {
        match self {
            Family::Usage => "USAGE",
            Family::Config => "CONFIG",
            Family::Env => "ENV",
            Family::Capture => "CAPTURE",
            Family::Gate => "GATE",
        }
    }

    /// Usage errors exit 2; every other family exits 1.
    pub fn exit(self) -> u8 {
        if matches!(self, Family::Usage) {
            2
        } else {
            1
        }
    }
}

/// A catalog entry: a code name bound to its family and its prose meaning.
/// Values live in `codes`. The `name`, `family`, and resulting exit are the
/// promise; `meaning` is prose the `capabilities` envelope emits and may be
/// reworded freely.
#[derive(Clone, Copy, Debug)]
pub struct Code {
    pub name: &'static str,
    pub family: Family,
    pub meaning: &'static str,
}

/// A failure that knows its own code. `message` is human prose and may be
/// reworded freely; `code.name`, `code.family`, and the resulting exit are the
/// promise.
#[derive(Debug)]
pub struct Diagnostic {
    pub code: Code,
    pub message: String,
}

impl Diagnostic {
    pub fn new(code: Code, message: impl Into<String>) -> Diagnostic {
        Diagnostic {
            code,
            message: message.into(),
        }
    }

    pub fn exit(&self) -> u8 {
        self.code.family.exit()
    }
}

impl fmt::Display for Diagnostic {
    /// `[CODE] message`, so the emitted line names its own code.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.code.name, self.message)
    }
}

/// The code catalog. Each entry pairs a name with its family and meaning, once,
/// so the three can never drift apart. `stringify!` keeps the string equal to
/// the constant. The same macro builds `ALL`, so `capabilities` enumerates
/// exactly the codes that exist — a new code cannot be forgotten in the
/// envelope, because adding the const is what adds it to `ALL`.
pub mod codes {
    use super::{Code, Family};

    macro_rules! catalog {
        ($($id:ident : $family:ident = $meaning:literal),* $(,)?) => {
            $(
                pub const $id: Code = Code {
                    name: stringify!($id),
                    family: Family::$family,
                    meaning: $meaning,
                };
            )*
            /// Every code, in catalog order. The single source `capabilities`
            /// iterates to emit `error_codes`.
            pub const ALL: &[Code] = &[$($id),*];
        };
    }

    catalog! {
        USAGE: Usage = "unknown verb, or --config given without a path value",

        CONFIG_UNREADABLE: Config = "the config file could not be read",
        CONFIG_INVALID_JSON: Config = "the config file is not valid JSON",
        CONFIG_SCHEMA: Config = "a field is missing or mistyped inside a declared block",
        CONFIG_INCOHERENT: Config = "claims declared without a fixture, or generated blocks without a capture",
        ALLOWLIST_UNKNOWN: Config = "the package_allowlist value is not recognized",
        RELEASE_CONFIG_MISSING: Config = "release was requested but the config has no release object",

        WORKDIR_UNREADABLE: Env = "the working directory could not be determined",
        FIXTURE_UNREADABLE: Env = "the committed fixture could not be read or parsed",
        SURFACE_UNREADABLE: Env = "a doc surface could not be read",
        WRITE_FAILED: Env = "the fixture or a doc surface could not be written",
        GIT_UNAVAILABLE: Env = "git status could not run",
        PACKAGE_LIST_FAILED: Env = "cargo package --list failed (for example a dirty tree without --allow-dirty)",
        CARGO_METADATA_FAILED: Env = "cargo metadata could not run, failed, or produced unusable data",
        CARGO_UNAVAILABLE: Env = "cargo publish --dry-run could not start",
        REMOTE_UNAVAILABLE: Env = "the configured git remote could not be queried",
        TAG_CREATE_FAILED: Env = "the annotated release tag could not be created",
        PUSH_FAILED: Env = "the branch and release tag could not be pushed atomically",

        NO_CONTRACT: Capture = "capture was invoked on a crate that declares no capture or fixture",
        BUILD_FAILED: Capture = "the build command exited non-zero",
        CAPTURE_RUN_FAILED: Capture = "the capture command was empty or exited non-zero",
        CAPTURE_NOT_JSON: Capture = "captured stdout was not valid JSON",
        RENDER_FAILED: Capture = "a render command was empty, exited non-zero, or emitted non-UTF-8",
        MARKER_MISSING: Capture = "a block's BEGIN or END marker was not found on its surface",

        WORKTREE_DIRTY: Gate = "uncommitted paths in the tree to be packaged",
        FIXTURE_STALE: Gate = "a fresh capture differs from the committed fixture",
        CLAIM_DRIFT: Gate = "a registered claim no longer equals its fixture field",
        STRAY_BLOCK: Gate = "a surface carries a GENERATED block with no declared renderer",
        PACKAGED_LEAK: Gate = "a packaged file falls outside the include allowlist",
        BLOCK_STALE: Gate = "a generated block differs from a fresh render",
        PREFLIGHT_FAILED: Gate = "one or more gates failed (the aggregate; the gate codes are the ground truth)",
        RELEASE_BRANCH_MISMATCH: Gate = "HEAD is detached or is not on the configured release branch",
        UPSTREAM_NOT_SYNCED: Gate = "the configured release branch is ahead of or behind its upstream",
        CHANGELOG_VERSION_MISSING: Gate = "CHANGELOG.md has no accepted H2 heading for the package version",
        TAG_EXISTS: Gate = "the release tag already exists locally or on the configured remote",
        PUBLISH_DRY_RUN_FAILED: Gate = "cargo publish --dry-run --locked failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_exits_two_others_exit_one() {
        assert_eq!(Family::Usage.exit(), 2);
        assert_eq!(Family::Config.exit(), 1);
        assert_eq!(Family::Gate.exit(), 1);
    }

    #[test]
    fn catalog_binds_name_to_the_matching_string_and_family() {
        assert_eq!(codes::STRAY_BLOCK.name, "STRAY_BLOCK");
        assert_eq!(codes::STRAY_BLOCK.family, Family::Gate);
        assert_eq!(codes::USAGE.family, Family::Usage);
    }

    #[test]
    fn display_prefixes_the_code() {
        let d = Diagnostic::new(codes::STRAY_BLOCK, "README.md: orphan block");
        assert_eq!(d.to_string(), "[STRAY_BLOCK] README.md: orphan block");
        assert_eq!(d.exit(), 1);
    }

    #[test]
    fn all_enumerates_every_code_uniquely_with_a_meaning() {
        // `capabilities` emits exactly `ALL`, so a duplicate or empty entry
        // would corrupt the published catalog.
        assert!(codes::ALL.iter().any(|c| c.name == "STRAY_BLOCK"));
        assert!(codes::ALL
            .iter()
            .all(|c| !c.name.is_empty() && !c.meaning.is_empty()));
        let mut names: Vec<&str> = codes::ALL.iter().map(|c| c.name).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "duplicate code name in the catalog");
    }
}
