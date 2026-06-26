use std::path::Path;
use std::process::Stdio;

use tokio::process::Command;
use tokio::time::{timeout, Duration};

use crate::models::ExecutionResult;
use crate::SandboxConfig;

const MAX_OUTPUT_SIZE: usize = 100_000; // 100KB

pub fn truncate_output(output: &str) -> String {
    if output.len() > MAX_OUTPUT_SIZE {
        let mut truncated = output[..MAX_OUTPUT_SIZE].to_string();
        truncated.push_str("\n... (output truncated)");
        truncated
    } else {
        output.to_string()
    }
}

pub async fn run_command(
    config: &SandboxConfig,
    chat_dir: &Path,
    args: &[&str],
) -> Result<ExecutionResult, String> {
    let mut cmd = if config.sandbox_enabled {
        let sandbox_exec = config
            .sandbox_exec_path
            .as_deref()
            .unwrap_or("sandbox-exec");
        let mut cmd = Command::new(sandbox_exec);
        cmd.arg(chat_dir.to_str().unwrap_or_default())
            .arg("--")
            .args(args);
        cmd
    } else {
        let mut cmd = Command::new(args[0]);
        if args.len() > 1 {
            cmd.args(&args[1..]);
        }
        cmd.current_dir(chat_dir).env_clear().envs([
            ("PATH", "/usr/local/bin:/usr/bin:/bin"),
            ("HOME", chat_dir.to_str().unwrap_or("/tmp")),
            ("TMPDIR", "/tmp"),
            ("PYTHONDONTWRITEBYTECODE", "1"),
        ]);
        cmd
    };

    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return Ok(ExecutionResult {
                stdout: String::new(),
                stderr: format!("Execution error: {e}"),
                exit_code: 1,
            });
        }
    };

    let deadline = Duration::from_secs(config.execution_timeout);
    match timeout(deadline, child.wait_with_output()).await {
        Ok(Ok(output)) => Ok(ExecutionResult {
            stdout: truncate_output(&String::from_utf8_lossy(&output.stdout)),
            stderr: truncate_output(&String::from_utf8_lossy(&output.stderr)),
            exit_code: output.status.code().unwrap_or(1),
        }),
        Ok(Err(e)) => Ok(ExecutionResult {
            stdout: String::new(),
            stderr: format!("Execution error: {e}"),
            exit_code: 1,
        }),
        Err(_) => {
            // Timeout — the child is dropped which sends SIGKILL
            Ok(ExecutionResult {
                stdout: String::new(),
                stderr: format!(
                    "Command timed out after {} seconds",
                    config.execution_timeout
                ),
                exit_code: 124,
            })
        }
    }
}
