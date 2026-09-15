//! Registry-driven lexical parser for the initial 0.0.3 command core.

use crate::diagnostic::{codes, Diagnostic};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verb {
    Check,
    Capture,
    Preflight,
    Release,
    Capabilities,
}

impl Verb {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "check" => Some(Self::Check),
            "capture" => Some(Self::Capture),
            "preflight" => Some(Self::Preflight),
            "release" => Some(Self::Release),
            "capabilities" => Some(Self::Capabilities),
            _ => None,
        }
    }

}

#[derive(Debug)]
pub struct Invocation {
    pub verb: Option<Verb>,
    pub json: bool,
    pub config_path: PathBuf,
    pub capture_check: bool,
    pub help: bool,
    pub version: bool,
}

pub fn parse(args: &[String]) -> Result<Invocation, Diagnostic> {
    let mut json = false;
    let mut config_path = PathBuf::from("plumbline.json");
    let mut color_seen = false;
    let mut no_color = false;
    let mut help = false;
    let mut version = false;
    let mut capture_check = false;
    let mut verb = None;
    let mut raw = false;
    let mut i = 0;

    while i < args.len() {
        let token = &args[i];
        if raw {
            return Err(Diagnostic::new(codes::INVALID_INPUT, "raw arguments are not declared for this command"));
        }
        if token == "--" {
            raw = true;
            i += 1;
            continue;
        }
        if token == "--json" {
            json = true;
        } else if token == "--no-color" {
            no_color = true;
        } else if token == "--help" || token == "-h" {
            help = true;
        } else if token == "--version" {
            version = true;
        } else if token == "--color" || token.starts_with("--color=") {
            if no_color || color_seen {
                return Err(Diagnostic::new(codes::INVALID_INPUT, "select either --no-color or one --color value"));
            }
            let value = value(token, args, &mut i, "--color")?;
            if !matches!(value.as_str(), "auto" | "always" | "never") {
                return Err(Diagnostic::new(codes::INVALID_INPUT, "--color must be auto, always, or never"));
            }
            color_seen = true;
        } else if token == "--config" || token.starts_with("--config=") {
            config_path = PathBuf::from(value(token, args, &mut i, "--config")?);
        } else if token == "--check" {
            if verb != Some(Verb::Capture) || capture_check {
                return Err(Diagnostic::new(codes::UNKNOWN_FLAG, "--check is declared only once for capture"));
            }
            capture_check = true;
        } else if token.starts_with('-') {
            return Err(Diagnostic::new(codes::UNKNOWN_FLAG, format!("unknown flag `{token}`")));
        } else if verb.is_none() {
            verb = Verb::parse(token);
            if verb.is_none() {
                return Err(Diagnostic::new(codes::UNKNOWN_COMMAND, format!("unknown command `{token}`")));
            }
        } else {
            return Err(Diagnostic::new(codes::INVALID_INPUT, format!("unexpected argument `{token}`")));
        }
        i += 1;
    }
    if no_color && color_seen {
        return Err(Diagnostic::new(codes::INVALID_INPUT, "select either --no-color or one --color value"));
    }
    Ok(Invocation { verb, json, config_path, capture_check, help, version })
}

fn value(token: &str, args: &[String], i: &mut usize, flag: &str) -> Result<String, Diagnostic> {
    if let Some(value) = token.strip_prefix(&format!("{flag}=")) {
        if value.is_empty() {
            return Err(Diagnostic::new(codes::INVALID_INPUT, format!("{flag} must not be empty")));
        }
        return Ok(value.to_string());
    }
    *i += 1;
    let value = args.get(*i).ok_or_else(|| Diagnostic::new(codes::MISSING_REQUIRED, format!("{flag} needs a value")))?;
    if value.starts_with('-') || value.is_empty() {
        return Err(Diagnostic::new(codes::INVALID_INPUT, format!("{flag} needs a non-empty value")));
    }
    Ok(value.clone())
}

pub fn terse_help() -> &'static str {
    "Usage: plumb [global-flags] <command> [command-flags]\n\nCommands: check, capture, preflight, release, capabilities\nGlobal flags: --config=<path>, --json, --no-color, --color=<auto|always|never>, --help, --version"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> { values.iter().map(|v| (*v).to_string()).collect() }

    #[test]
    fn global_flags_work_on_both_sides_of_a_command() {
        let before = parse(&strings(&["--json", "--config=a.json", "check"])).unwrap();
        let after = parse(&strings(&["check", "--config=a.json", "--json"])).unwrap();
        assert_eq!(before.verb, Some(Verb::Check));
        assert_eq!(after.config_path, PathBuf::from("a.json"));
    }

    #[test]
    fn value_flags_do_not_consume_flag_shaped_values() {
        let error = parse(&strings(&["--config", "--json", "check"])).unwrap_err();
        assert_eq!(error.code.name, "INVALID_INPUT");
    }

    #[test]
    fn capture_check_is_command_local() {
        assert!(parse(&strings(&["capture", "--check"])).is_ok());
        assert_eq!(parse(&strings(&["check", "--check"])).unwrap_err().code.name, "UNKNOWN_FLAG");
    }
}
