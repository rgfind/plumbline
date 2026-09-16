//! Safe execution of declared release verification commands.
//!
//! Config supplies an argv vector, never a shell program. This module starts
//! that vector at the configured crate root and returns only bounded execution
//! facts; it deliberately never stores command output in release reports.

use crate::config::VerificationGate;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, PartialEq, Eq)]
pub enum GateOutcome {
    Passed,
    Failed { exit_code: Option<i32> },
    TimedOut,
    Unavailable,
}

pub trait CommandRunner {
    fn run(&mut self, argv: &[String], root: &Path, timeout: Duration) -> GateOutcome;
}

pub struct SystemRunner;

impl CommandRunner for SystemRunner {
    fn run(&mut self, argv: &[String], root: &Path, timeout: Duration) -> GateOutcome {
        let Some(program) = argv.first() else {
            return GateOutcome::Unavailable;
        };
        let mut command = Command::new(program);
        command
            .args(&argv[1..])
            .current_dir(root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(_) => return GateOutcome::Unavailable,
        };
        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => return GateOutcome::Passed,
                Ok(Some(status)) => {
                    return GateOutcome::Failed {
                        exit_code: status.code(),
                    }
                }
                Ok(None) if started.elapsed() < timeout => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Ok(None) => {
                    #[cfg(unix)]
                    unsafe {
                        libc::kill(-(child.id() as i32), libc::SIGKILL);
                    }
                    #[cfg(not(unix))]
                    {
                        let _ = child.kill();
                    }
                    let _ = child.wait();
                    return GateOutcome::TimedOut;
                }
                Err(_) => return GateOutcome::Unavailable,
            }
        }
    }
}

pub fn run_gates<R: CommandRunner>(
    runner: &mut R,
    gates: &[VerificationGate],
    root: &Path,
    stage: &str,
) -> Vec<serde_json::Value> {
    gates.iter().map(|gate| {
        let outcome = runner.run(&gate.argv, root, Duration::from_secs(gate.timeout_seconds));
        let action = serde_json::json!({
            "command": gate.argv.join(" "),
            "rationale": format!("the configured {} gate `{}` did not pass", stage, gate.id),
            "alternatives": [{"command": format!("plumb verify --stage={stage} --json"), "purpose": "rerun the declared local gates"}],
        });
        match outcome {
            GateOutcome::Passed => serde_json::json!({
                "id": gate.id, "kind": "command", "stage": stage,
                "argv": gate.argv, "status": "passed", "exit_code": 0,
            }),
            GateOutcome::Failed { exit_code } => serde_json::json!({
                "id": gate.id, "kind": "command", "stage": stage,
                "argv": gate.argv, "status": "failed", "exit_code": exit_code,
                "diagnostic_code": "VERIFY_COMMAND_FAILED", "recommended_action": action,
            }),
            GateOutcome::TimedOut => serde_json::json!({
                "id": gate.id, "kind": "command", "stage": stage,
                "argv": gate.argv, "status": "failed", "timed_out": true,
                "diagnostic_code": "VERIFY_COMMAND_TIMED_OUT", "recommended_action": action,
            }),
            GateOutcome::Unavailable => serde_json::json!({
                "id": gate.id, "kind": "command", "stage": stage,
                "argv": gate.argv, "status": "failed",
                "diagnostic_code": "VERIFY_COMMAND_UNAVAILABLE", "recommended_action": action,
            }),
        }
    }).collect()
}

#[cfg(test)]
pub struct FakeRunner {
    pub outcomes: Vec<GateOutcome>,
    pub seen: Vec<(Vec<String>, String, Duration)>,
}

#[cfg(test)]
impl CommandRunner for FakeRunner {
    fn run(&mut self, argv: &[String], root: &Path, timeout: Duration) -> GateOutcome {
        self.seen
            .push((argv.to_vec(), root.display().to_string(), timeout));
        self.outcomes.remove(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_runner_records_argument_vectors_without_a_shell() {
        let mut runner = FakeRunner {
            outcomes: vec![GateOutcome::Passed],
            seen: Vec::new(),
        };
        let argv = vec!["echo".into(), "$(must-not-expand)".into()];
        assert_eq!(
            runner.run(&argv, Path::new("/tmp"), Duration::from_secs(1)),
            GateOutcome::Passed
        );
        assert_eq!(runner.seen[0].0, argv);
    }
}
