//! Registry-driven lexical parser for the initial 0.0.3 command core.

use crate::diagnostic::{codes, Diagnostic};
use serde_json::json;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verb {
    Check,
    Capture,
    Preflight,
    Release,
    Capabilities,
    Schema,
    Config,
    Conformance,
    RobotDocs,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigAction {
    Schema,
    Validate,
    Show,
    Get,
    Set,
    Patch,
}

impl ConfigAction {
    fn parse(value: &str) -> Option<Self> {
        CONFIG_ACTIONS
            .iter()
            .find(|spec| spec.name == value)
            .map(|spec| spec.action)
    }
}

pub struct ConfigActionSpec {
    pub action: ConfigAction,
    pub name: &'static str,
    pub mutates: bool,
    pub positionals: &'static [&'static str],
    pub flags: &'static [&'static str],
}

pub const CONFIG_ACTIONS: &[ConfigActionSpec] = &[
    ConfigActionSpec {
        action: ConfigAction::Schema,
        name: "schema",
        mutates: false,
        positionals: &[],
        flags: &[],
    },
    ConfigActionSpec {
        action: ConfigAction::Validate,
        name: "validate",
        mutates: false,
        positionals: &[],
        flags: &[],
    },
    ConfigActionSpec {
        action: ConfigAction::Show,
        name: "show",
        mutates: false,
        positionals: &[],
        flags: &[],
    },
    ConfigActionSpec {
        action: ConfigAction::Get,
        name: "get",
        mutates: false,
        positionals: &["json-pointer"],
        flags: &[],
    },
    ConfigActionSpec {
        action: ConfigAction::Set,
        name: "set",
        mutates: true,
        positionals: &["json-pointer", "json-value"],
        flags: &["--yes", "--if-match"],
    },
    ConfigActionSpec {
        action: ConfigAction::Patch,
        name: "patch",
        mutates: true,
        positionals: &[],
        flags: &["--from-stdin", "--yes", "--if-match"],
    },
];

/// The initial registry is the only list of command names and local flags.
/// Later slices add nested verbs and payload schemas without a second parser
/// list.
pub struct CommandSpec {
    pub verb: Verb,
    pub name: &'static str,
    pub summary: &'static str,
    pub flags: &'static [&'static str],
    pub needs_config: bool,
}

pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        verb: Verb::Check,
        name: "check",
        summary: "check fixture claims and generated blocks",
        flags: &[],
        needs_config: true,
    },
    CommandSpec {
        verb: Verb::Capture,
        name: "capture",
        summary: "capture or compare current binary output",
        flags: &["--check", "--yes"],
        needs_config: true,
    },
    CommandSpec {
        verb: Verb::Preflight,
        name: "preflight",
        summary: "run the ordered release gates",
        flags: &[],
        needs_config: true,
    },
    CommandSpec {
        verb: Verb::Release,
        name: "release",
        summary: "validate, tag, and atomically push",
        flags: &["--yes"],
        needs_config: true,
    },
    CommandSpec {
        verb: Verb::Capabilities,
        name: "capabilities",
        summary: "return the live command contract",
        flags: &[],
        needs_config: false,
    },
    CommandSpec {
        verb: Verb::Schema,
        name: "schema",
        summary: "return JSON Schemas for command responses",
        flags: &["--command"],
        needs_config: false,
    },
    CommandSpec {
        verb: Verb::Config,
        name: "config",
        summary: "inspect or safely change the selected configuration",
        flags: &["--yes", "--if-match", "--from-stdin"],
        needs_config: false,
    },
    CommandSpec {
        verb: Verb::Conformance,
        name: "conformance",
        summary: "run installed-command self checks",
        flags: &[],
        needs_config: false,
    },
    CommandSpec {
        verb: Verb::RobotDocs,
        name: "robot-docs",
        summary: "render the agent workflow guide",
        flags: &[],
        needs_config: false,
    },
];

impl Verb {
    pub fn parse(value: &str) -> Option<Self> {
        COMMANDS
            .iter()
            .find(|spec| spec.name == value)
            .map(|spec| spec.verb)
    }
}

#[derive(Debug)]
pub struct Invocation {
    pub verb: Option<Verb>,
    pub json: bool,
    pub config_path: Option<PathBuf>,
    pub capture_check: bool,
    pub help: bool,
    pub version: bool,
    pub schema_command: Option<String>,
    pub robot_docs_guide: bool,
    pub yes: bool,
    pub config_action: Option<ConfigAction>,
    pub config_arguments: Vec<String>,
    pub if_match: Option<String>,
    pub from_stdin: bool,
}

pub fn parse(args: &[String]) -> Result<Invocation, Diagnostic> {
    let mut json = false;
    let mut config_path = None;
    let mut color_seen = false;
    let mut no_color = false;
    let mut help = false;
    let mut version = false;
    let mut capture_check = false;
    let mut verb = None;
    let mut schema_command = None;
    let mut robot_docs_guide = false;
    let mut yes = false;
    let mut config_action = None;
    let mut config_arguments = Vec::new();
    let mut if_match = None;
    let mut from_stdin = false;
    let mut raw = false;
    let mut i = 0;

    while i < args.len() {
        let token = &args[i];
        if raw {
            return Err(Diagnostic::new(
                codes::INVALID_INPUT,
                "raw arguments are not declared for this command",
            ));
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
                return Err(Diagnostic::new(
                    codes::INVALID_INPUT,
                    "select either --no-color or one --color value",
                ));
            }
            let value = value(token, args, &mut i, "--color")?;
            if !matches!(value.as_str(), "auto" | "always" | "never") {
                return Err(Diagnostic::new(
                    codes::INVALID_INPUT,
                    "--color must be auto, always, or never",
                ));
            }
            color_seen = true;
        } else if token == "--config" || token.starts_with("--config=") {
            config_path = Some(PathBuf::from(value(token, args, &mut i, "--config")?));
        } else if token == "--command" || token.starts_with("--command=") {
            if verb != Some(Verb::Schema) || schema_command.is_some() {
                return Err(Diagnostic::new(
                    codes::UNKNOWN_FLAG,
                    "--command is declared only once for schema",
                ));
            }
            schema_command = Some(value(token, args, &mut i, "--command")?);
        } else if token == "--check" {
            if verb != Some(Verb::Capture) || capture_check {
                return Err(Diagnostic::new(
                    codes::UNKNOWN_FLAG,
                    "--check is declared only once for capture",
                ));
            }
            capture_check = true;
        } else if token == "--yes" || token == "-y" {
            if !matches!(verb, Some(Verb::Capture | Verb::Release))
                && !matches!(config_action, Some(ConfigAction::Set | ConfigAction::Patch))
                || yes
            {
                return Err(Diagnostic::new(
                    codes::UNKNOWN_FLAG,
                    "--yes is declared only once for a mutating command",
                ));
            }
            yes = true;
        } else if token == "--if-match" || token.starts_with("--if-match=") {
            if !matches!(config_action, Some(ConfigAction::Set | ConfigAction::Patch))
                || if_match.is_some()
            {
                return Err(Diagnostic::new(
                    codes::UNKNOWN_FLAG,
                    "--if-match is declared only once for config set or patch",
                ));
            }
            if_match = Some(value(token, args, &mut i, "--if-match")?);
        } else if token == "--from-stdin" {
            if config_action != Some(ConfigAction::Patch) || from_stdin {
                return Err(Diagnostic::new(
                    codes::UNKNOWN_FLAG,
                    "--from-stdin is declared only once for config patch",
                ));
            }
            from_stdin = true;
        } else if token.starts_with('-') {
            return Err(Diagnostic::new(
                codes::UNKNOWN_FLAG,
                format!("unknown flag `{token}`"),
            ));
        } else if verb.is_none() {
            verb = Verb::parse(token);
            if verb.is_none() {
                return Err(Diagnostic::new(
                    codes::UNKNOWN_COMMAND,
                    format!("unknown command `{token}`"),
                ));
            }
        } else if verb == Some(Verb::RobotDocs) && token == "guide" && !robot_docs_guide {
            robot_docs_guide = true;
        } else if verb == Some(Verb::Config) && config_action.is_none() {
            config_action = ConfigAction::parse(token);
            if config_action.is_none() {
                return Err(Diagnostic::new(
                    codes::INVALID_INPUT,
                    format!("unknown config command `{token}`"),
                ));
            }
        } else if verb == Some(Verb::Config) {
            config_arguments.push(token.clone());
        } else {
            return Err(Diagnostic::new(
                codes::INVALID_INPUT,
                format!("unexpected argument `{token}`"),
            ));
        }
        i += 1;
    }
    if no_color && color_seen {
        return Err(Diagnostic::new(
            codes::INVALID_INPUT,
            "select either --no-color or one --color value",
        ));
    }
    Ok(Invocation {
        verb,
        json,
        config_path,
        capture_check,
        help,
        version,
        schema_command,
        robot_docs_guide,
        yes,
        config_action,
        config_arguments,
        if_match,
        from_stdin,
    })
}

fn value(token: &str, args: &[String], i: &mut usize, flag: &str) -> Result<String, Diagnostic> {
    if let Some(value) = token.strip_prefix(&format!("{flag}=")) {
        if value.is_empty() {
            return Err(Diagnostic::new(
                codes::INVALID_INPUT,
                format!("{flag} must not be empty"),
            ));
        }
        return Ok(value.to_string());
    }
    *i += 1;
    let value = args
        .get(*i)
        .ok_or_else(|| Diagnostic::new(codes::MISSING_REQUIRED, format!("{flag} needs a value")))?;
    if value.starts_with('-') || value.is_empty() {
        return Err(Diagnostic::new(
            codes::INVALID_INPUT,
            format!("{flag} needs a non-empty value"),
        ));
    }
    Ok(value.clone())
}

pub fn terse_help() -> String {
    let commands = COMMANDS
        .iter()
        .map(|spec| spec.name)
        .collect::<Vec<_>>()
        .join(", ");
    format!("Usage: plumb [global-flags] <command> [command-flags]\n\nCommands: {commands}\nGlobal flags: --config=<path>, --json, --no-color, --color=<auto|always|never>, --help, --version")
}

pub fn capability_verbs() -> serde_json::Value {
    let mut verbs = serde_json::Map::new();
    for spec in COMMANDS {
        let mut value = json!({
            "description": spec.summary,
            "mutates": matches!(spec.verb, Verb::Capture | Verb::Release),
            "positionals": [],
            "flags": spec.flags,
            "output_modes": ["text", "json"],
            "possible_exit_codes": [0, 1, 2, 3, 4, 5, 6],
            "payload_schema": {"type": "object"},
            "meta_fields": ["request_id", "ts_iso", "elapsed_ms", "contract_version", "schema_version"],
            "examples": [format!("plumb {} --json", spec.name)],
            "needs_config": spec.needs_config,
        });
        if spec.verb == Verb::Config {
            value["subcommands"] = json!(CONFIG_ACTIONS
                .iter()
                .map(|action| json!({
                    "name": action.name,
                    "mutates": action.mutates,
                    "positionals": action.positionals,
                    "flags": action.flags,
                }))
                .collect::<Vec<_>>());
        }
        verbs.insert(spec.name.to_string(), value);
    }
    serde_json::Value::Object(verbs)
}

pub fn parser_manifest() -> serde_json::Value {
    json!({
        "commands": COMMANDS.iter().map(|spec| json!({
            "name": spec.name,
            "flags": spec.flags,
            "subcommands": if spec.verb == Verb::Config {
                Some(CONFIG_ACTIONS.iter().map(|action| action.name).collect::<Vec<_>>())
            } else { None },
        })).collect::<Vec<_>>(),
        "global_flags": ["--config", "--json", "--no-color", "--color", "--help", "--version"],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| (*v).to_string()).collect()
    }

    #[test]
    fn global_flags_work_on_both_sides_of_a_command() {
        let before = parse(&strings(&["--json", "--config=a.json", "check"])).unwrap();
        let after = parse(&strings(&["check", "--config=a.json", "--json"])).unwrap();
        assert_eq!(before.verb, Some(Verb::Check));
        assert_eq!(after.config_path, Some(PathBuf::from("a.json")));
    }

    #[test]
    fn value_flags_do_not_consume_flag_shaped_values() {
        let error = parse(&strings(&["--config", "--json", "check"])).unwrap_err();
        assert_eq!(error.code.name, "INVALID_INPUT");
    }

    #[test]
    fn capture_check_is_command_local() {
        assert!(parse(&strings(&["capture", "--check"])).is_ok());
        assert_eq!(
            parse(&strings(&["check", "--check"]))
                .unwrap_err()
                .code
                .name,
            "UNKNOWN_FLAG"
        );
    }

    #[test]
    fn yes_is_limited_to_mutating_commands() {
        assert!(parse(&strings(&["capture", "--yes"])).unwrap().yes);
        assert!(parse(&strings(&["release", "-y"])).unwrap().yes);
        assert_eq!(
            parse(&strings(&["check", "--yes"])).unwrap_err().code.name,
            "UNKNOWN_FLAG"
        );
    }

    #[test]
    fn config_mutation_flags_follow_the_config_action() {
        let parsed = parse(&strings(&[
            "config",
            "set",
            "/release/branch",
            "\"main\"",
            "--yes",
            "--if-match=abc",
        ]))
        .unwrap();
        assert_eq!(parsed.config_action, Some(ConfigAction::Set));
        assert!(parsed.yes);
        assert_eq!(parsed.if_match.as_deref(), Some("abc"));
        assert_eq!(
            parse(&strings(&["config", "show", "--yes"]))
                .unwrap_err()
                .code
                .name,
            "UNKNOWN_FLAG"
        );
    }
}
