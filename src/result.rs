//! One typed result model and renderer for every command response.

use crate::diagnostic::Diagnostic;
use serde_json::{json, Value};
use std::io::{self, Write};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub const CONTRACT_VERSION: &str = "0.1";

pub struct CommandResult {
    pub data: Value,
    pub human: String,
    pub commands: Vec<String>,
    pub schema_version: u32,
}

impl CommandResult {
    pub fn new(data: Value, human: impl Into<String>) -> Self {
        Self { data, human: human.into(), commands: Vec::new(), schema_version: 1 }
    }
}

pub fn render(
    result: Result<CommandResult, Diagnostic>,
    json_mode: bool,
    started: Instant,
) -> u8 {
    let (ok, data, human, commands, errors, schema_version) = match result {
        Ok(result) => (true, result.data, result.human, result.commands, Vec::new(), result.schema_version),
        Err(error) => (false, Value::Null, String::new(), Vec::new(), vec![error.as_json()], 1),
    };
    if json_mode {
        let envelope = envelope(ok, data, commands, errors.clone(), schema_version, started.elapsed().as_millis() as u64);
        write_stdout(&format!("{}\n", serde_json::to_string(&envelope).expect("serialize envelope")));
        if !ok {
            let error = &errors[0];
            let _ = writeln!(io::stderr(), "plumb: [{}] {}", error["code"].as_str().unwrap_or("INTERNAL"), error["message"].as_str().unwrap_or("command failed"));
        }
    } else if ok {
        if !human.is_empty() {
            write_stdout(&format!("{human}\n"));
        }
    } else if let Some(error) = errors.first() {
        let message = error["message"].as_str().unwrap_or("command failed");
        let code = error["code"].as_str().unwrap_or("INTERNAL");
        let _ = writeln!(io::stderr(), "plumb: [{code}] {message}");
    }
    if ok { 0 } else { errors[0]["exit_code"].as_u64().unwrap_or(6) as u8 }
}

fn envelope(
    ok: bool,
    data: Value,
    commands: Vec<String>,
    errors: Vec<Value>,
    schema_version: u32,
    elapsed_ms: u64,
) -> Value {
    json!({
        "ok": ok,
        "tool_version": env!("CARGO_PKG_VERSION"),
        "data": data,
        "meta": {
            "request_id": request_id(),
            "ts_iso": timestamp(),
            "elapsed_ms": elapsed_ms,
            "contract_version": CONTRACT_VERSION,
            "schema_version": schema_version,
        },
        "warnings": [],
        "commands": commands,
        "errors": errors,
    })
}

fn write_stdout(text: &str) {
    let _ = io::stdout().write_all(text.as_bytes());
}

fn request_id() -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("req-{:x}-{:x}", std::process::id(), nanos)
}

fn timestamp() -> String {
    // `SOURCE_DATE_EPOCH` permits deterministic fixtures.
    let seconds = std::env::var("SOURCE_DATE_EPOCH").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or_else(|| {
        SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
    });
    unix_seconds_to_rfc3339(seconds)
}

/// Convert a Unix second value to a UTC RFC 3339 timestamp without adding a
/// runtime dependency. This civil-date algorithm is valid for the full `u64`
/// range that can be represented as a signed day count.
fn unix_seconds_to_rfc3339(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let day_seconds = seconds % 86_400;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += if month <= 2 { 1 } else { 0 };
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", day_seconds / 3_600, (day_seconds % 3_600) / 60, day_seconds % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::{codes, Diagnostic};

    #[test]
    fn unix_epoch_is_rfc3339() {
        assert_eq!(unix_seconds_to_rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(unix_seconds_to_rfc3339(86_400), "1970-01-02T00:00:00Z");
    }

    #[test]
    fn pins_all_exit_class_envelope_shapes() {
        let success = envelope(true, json!({"status": "passed"}), Vec::new(), Vec::new(), 1, 0);
        assert_eq!(success["ok"], true);
        assert_eq!(success["errors"], json!([]));

        let cases = [
            (codes::UNKNOWN_FLAG, 1),
            (codes::WORKTREE_DIRTY, 2),
            (codes::CONFIG_UNREADABLE, 3),
            (codes::LOCKED, 4),
            (codes::CONFIG_WRITE_CONFLICT, 5),
            (codes::INTERNAL, 6),
        ];
        for (code, exit_code) in cases {
            let value = envelope(false, Value::Null, Vec::new(), vec![Diagnostic::new(code, "test").as_json()], 1, 0);
            for key in ["ok", "tool_version", "data", "meta", "warnings", "commands", "errors"] {
                assert!(value.get(key).is_some(), "missing {key}");
            }
            assert_eq!(value["data"], Value::Null);
            assert_eq!(value["errors"][0]["exit_code"], exit_code);
        }
    }
}
