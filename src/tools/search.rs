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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_finds_pattern() {
        let result = execute(serde_json::json!({"pattern": "fn execute", "path": "src/tools"}));
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.contains("fn execute"));
    }

    #[test]
    fn search_no_matches() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "nothing here").unwrap();
        let result = execute(
            serde_json::json!({"pattern": "will_not_match_anything", "path": dir.path().to_str().unwrap()}),
        );
        assert_eq!(result.unwrap(), "No matches found");
    }

    #[test]
    fn search_missing_pattern() {
        let result = execute(serde_json::json!({}));
        assert_eq!(result.unwrap_err(), "pattern is required");
    }

    #[test]
    fn search_empty_pattern() {
        let result = execute(serde_json::json!({"pattern": ""}));
        assert_eq!(result.unwrap_err(), "pattern is required");
    }

    #[test]
    fn search_case_insensitive_default() {
        let result = execute(serde_json::json!({"pattern": "FN EXECUTE", "path": "src/tools"}));
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.contains("fn execute"));
    }

    #[test]
    fn search_with_file_type() {
        let result =
            execute(serde_json::json!({"pattern": "fn tool", "path": "src", "file_type": "rust"}));
        assert!(result.is_ok());
        assert!(result.unwrap().contains("fn tool"));
    }
}
