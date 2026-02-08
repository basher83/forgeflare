use super::{ToolDef, Value, schema};

pub fn tool() -> ToolDef {
    ToolDef {
        name: "bash",
        description: "Execute a bash command and return its output",
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
    let output = cmd.output().map_err(|e| format!("exec failed: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        Ok(format!(
            "Command failed ({}): {stdout}{stderr}",
            output.status
        ))
    } else {
        Ok(format!("{stdout}{stderr}").trim().to_string())
    }
}
