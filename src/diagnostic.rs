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
    /// The family's contract string (`"GATE"`, `"CONFIG"`, ...). Emitted by the
    /// `capabilities` envelope, which is the next step in CONTRACT.md's "Building
    /// this"; unused until then.
    #[allow(dead_code)]
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

/// A catalog entry: a code name bound to its family. Values live in `codes`.
#[derive(Clone, Copy, Debug)]
pub struct Code {
    pub name: &'static str,
    pub family: Family,
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

/// The code catalog. Each entry pairs a name with its family, once, so the two
/// can never drift apart. `stringify!` keeps the string equal to the constant.
pub mod codes {
    use super::{Code, Family};

    macro_rules! code {
        ($id:ident, $family:ident) => {
            pub const $id: Code = Code {
                name: stringify!($id),
                family: Family::$family,
            };
        };
    }

    code!(USAGE, Usage);

    code!(CONFIG_UNREADABLE, Config);
    code!(CONFIG_INVALID_JSON, Config);
    code!(CONFIG_SCHEMA, Config);
    code!(CONFIG_INCOHERENT, Config);
    code!(ALLOWLIST_UNKNOWN, Config);

    code!(WORKDIR_UNREADABLE, Env);
    code!(FIXTURE_UNREADABLE, Env);
    code!(SURFACE_UNREADABLE, Env);
    code!(WRITE_FAILED, Env);
    code!(GIT_UNAVAILABLE, Env);
    code!(PACKAGE_LIST_FAILED, Env);

    code!(NO_CONTRACT, Capture);
    code!(BUILD_FAILED, Capture);
    code!(CAPTURE_RUN_FAILED, Capture);
    code!(CAPTURE_NOT_JSON, Capture);
    code!(RENDER_FAILED, Capture);
    code!(MARKER_MISSING, Capture);

    code!(WORKTREE_DIRTY, Gate);
    code!(FIXTURE_STALE, Gate);
    code!(CLAIM_DRIFT, Gate);
    code!(STRAY_BLOCK, Gate);
    code!(PACKAGED_LEAK, Gate);
    code!(BLOCK_STALE, Gate);
    code!(PREFLIGHT_FAILED, Gate);
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
}
