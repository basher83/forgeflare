use super::{ToolDef, Value, schema};
use std::io::Read;
use std::time::Duration;
use wait_timeout::ChildExt;

const BASH_TIMEOUT: Duration = Duration::from_secs(120);

pub fn tool() -> ToolDef {
    ToolDef {
        name: "bash",
        description: "Execute a bash command and return its output (120s timeout)",
        input_schema: schema!(
            {"command", "string", "The bash command to execute"},
            {"cwd", "string", "Optional working directory"}
            ; "command"
        ),
        function: execute,
    }
}

fn execute(input: Value) -> Result<String, String> {
    let command = input["command"].as_str().ok_or("command is required")?;
    let mut cmd = std::process::Command::new("bash");
    cmd.arg("-c").arg(command);
    if let Some(cwd) = input["cwd"].as_str() {
        cmd.current_dir(cwd);
    }
    let mut child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("exec failed: {e}"))?;

    let status = match child
        .wait_timeout(BASH_TIMEOUT)
        .map_err(|e| format!("wait: {e}"))?
    {
        Some(status) => status,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            return Ok("Command timed out after 120s and was killed".to_string());
        }
    };

    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Some(mut out) = child.stdout.take() {
        out.read_to_string(&mut stdout).ok();
    }
    if let Some(mut err) = child.stderr.take() {
        err.read_to_string(&mut stderr).ok();
    }

    if !status.success() {
        Ok(format!("Command failed ({status}): {stdout}{stderr}"))
    } else {
        Ok(format!("{stdout}{stderr}").trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_echo() {
        let result = execute(serde_json::json!({"command": "echo hello"}));
        assert_eq!(result.unwrap(), "hello");
    }

    #[test]
    fn bash_missing_command() {
        let result = execute(serde_json::json!({}));
        assert_eq!(result.unwrap_err(), "command is required");
    }

    #[test]
    fn bash_failing_command() {
        let result = execute(serde_json::json!({"command": "false"}));
        let output = result.unwrap();
        assert!(output.starts_with("Command failed"));
    }

    #[test]
    fn bash_with_cwd() {
        let result = execute(serde_json::json!({"command": "pwd", "cwd": "/tmp"}));
        let output = result.unwrap();
        assert!(output.contains("tmp") || output.contains("private/tmp"));
    }

    #[test]
    fn bash_stderr_captured() {
        let result = execute(serde_json::json!({"command": "echo err >&2"}));
        let output = result.unwrap();
        assert!(output.contains("err"));
    }
}
