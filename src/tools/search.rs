use super::{ToolDef, Value, schema};
use std::process::Command;

pub fn tool() -> ToolDef {
    ToolDef {
        name: "code_search",
        description: "Search for code patterns using ripgrep (rg)",
        input_schema: schema!(
            {"pattern", "string", "The search pattern or regex"},
            {"path", "string", "Optional path to search in"},
            {"file_type", "string", "File extension filter (e.g. 'go', 'js')"},
            {"case_sensitive", "boolean", "Case sensitive (default: false)"}
            ; "pattern"
        ),
        function: execute,
    }
}

fn execute(input: Value) -> Result<String, String> {
    let pattern = input["pattern"].as_str().ok_or("pattern is required")?;
    if pattern.is_empty() {
        return Err("pattern is required".into());
    }
    let path = input["path"].as_str().unwrap_or(".");
    let mut args = vec!["--line-number", "--with-filename", "--color=never"];
    if !input["case_sensitive"].as_bool().unwrap_or(false) {
        args.push("--ignore-case");
    }
    if let Some(ft) = input["file_type"].as_str() {
        args.push("--type");
        args.push(ft);
    }
    args.push(pattern);
    args.push(path);
    let output = Command::new("rg")
        .args(&args)
        .output()
        .map_err(|e| format!("rg failed: {e}"))?;
    if !output.status.success() {
        if output.status.code() == Some(1) {
            return Ok("No matches found".into());
        }
        return Err(format!(
            "search failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let result = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let lines: Vec<&str> = result.lines().collect();
    if lines.len() > 50 {
        Ok(format!(
            "{}\n... (showing 50 of {} matches)",
            lines[..50].join("\n"),
            lines.len()
        ))
    } else {
        Ok(result)
    }
}
