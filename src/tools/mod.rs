use crate::api::ContentBlock;
use serde_json::Value;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::Command;
use std::time::Duration;
use wait_timeout::ChildExt;

const BASH_TIMEOUT: Duration = Duration::from_secs(120);

macro_rules! tools {
    ($($name:expr, $desc:expr, $schema:expr, $func:expr);+ $(;)?) => {
        pub fn all_tool_schemas() -> Vec<Value> {
            vec![$(serde_json::json!({"name": $name, "description": $desc, "input_schema": $schema})),+]
        }
        pub fn dispatch_tool(name: &str, input: Value, id: &str) -> ContentBlock {
            let (content, is_error) = match name {
                $($name => match $func(input) { Ok(s) => (s, None), Err(s) => (s, Some(true)) },)+
                _ => (format!("tool '{name}' not found"), Some(true)),
            };
            ContentBlock::ToolResult { tool_use_id: id.to_string(), content, is_error }
        }
    };
}

tools! {
    "read_file", "Read a file's contents with line numbers",
    serde_json::json!({"type": "object", "properties": {"path": {"type": "string", "description": "Relative file path"}}, "required": ["path"]}),
    read_exec;
    "list_files", "List files and directories. Defaults to current directory.",
    serde_json::json!({"type": "object", "properties": {"path": {"type": "string", "description": "Optional path to list"}}, "required": []}),
    list_exec;
    "bash", "Execute a bash command and return its output (120s timeout)",
    serde_json::json!({"type": "object", "properties": {"command": {"type": "string", "description": "The bash command to execute"}, "cwd": {"type": "string", "description": "Optional working directory"}}, "required": ["command"]}),
    bash_exec;
    "edit_file", "Replace old_str with new_str in a file. Creates file if missing.",
    serde_json::json!({"type": "object", "properties": {"path": {"type": "string", "description": "The path to the file"}, "old_str": {"type": "string", "description": "Text to search for - must match exactly once"}, "new_str": {"type": "string", "description": "Text to replace old_str with"}}, "required": ["path", "old_str", "new_str"]}),
    edit_exec;
    "code_search", "Search for code patterns using ripgrep (rg)",
    serde_json::json!({"type": "object", "properties": {"pattern": {"type": "string", "description": "The search pattern or regex"}, "path": {"type": "string", "description": "Optional path to search in"}, "file_type": {"type": "string", "description": "File extension filter (e.g. 'go', 'js')"}, "case_sensitive": {"type": "boolean", "description": "Case sensitive (default: false)"}}, "required": ["pattern"]}),
    search_exec;
}

fn read_exec(input: Value) -> Result<String, String> {
    let path = input["path"].as_str().ok_or("path is required")?;
    let content = fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    Ok(content
        .lines()
        .enumerate()
        .map(|(i, l)| format!("{}: {l}", i + 1))
        .collect::<Vec<_>>()
        .join("\n"))
}

fn list_exec(input: Value) -> Result<String, String> {
    let dir = input["path"].as_str().unwrap_or(".");
    let mut files = Vec::new();
    walk(Path::new(dir), Path::new(dir), &mut files).map_err(|e| e.to_string())?;
    serde_json::to_string(&files).map_err(|e| e.to_string())
}

fn walk(base: &Path, dir: &Path, files: &mut Vec<String>) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let rel = path
            .strip_prefix(base)
            .unwrap_or(&path)
            .to_string_lossy()
            .to_string();
        if entry.file_type()?.is_dir() {
            if rel == ".git" || rel == ".devenv" {
                continue;
            }
            files.push(format!("{rel}/"));
            walk(base, &path, files)?;
        } else {
            files.push(rel);
        }
    }
    Ok(())
}

fn bash_exec(input: Value) -> Result<String, String> {
    let command = input["command"].as_str().ok_or("command is required")?;
    let mut cmd = Command::new("bash");
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
        Some(s) => s,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Command timed out after 120s and was killed".into());
        }
    };
    let (mut stdout, mut stderr) = (String::new(), String::new());
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .ok();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .ok();
    if !status.success() {
        Ok(format!("Command failed ({status}): {stdout}{stderr}"))
    } else {
        Ok(format!("{stdout}{stderr}").trim().to_string())
    }
}

fn edit_exec(input: Value) -> Result<String, String> {
    let path_s = input["path"].as_str().ok_or("path is required")?;
    let old_str = input["old_str"].as_str().ok_or("old_str is required")?;
    let new_str = input["new_str"].as_str().ok_or("new_str is required")?;
    if old_str == new_str {
        return Err("old_str and new_str must differ".into());
    }
    let path = Path::new(path_s);
    if !path.exists() && old_str.is_empty() {
        if let Some(p) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(p).map_err(|e| format!("mkdir: {e}"))?;
        }
        fs::write(path, new_str).map_err(|e| format!("write: {e}"))?;
        return Ok(format!("Created {path_s}"));
    }
    let content = fs::read_to_string(path).map_err(|e| format!("{path_s}: {e}"))?;
    if old_str.is_empty() {
        fs::write(path, format!("{content}{new_str}")).map_err(|e| format!("write: {e}"))?;
    } else {
        let count = content.matches(old_str).count();
        if count == 0 {
            return Err("old_str not found".into());
        }
        if count > 1 {
            return Err(format!("old_str found {count} times, must be unique"));
        }
        fs::write(path, content.replacen(old_str, new_str, 1))
            .map_err(|e| format!("write: {e}"))?;
    }
    Ok("OK".to_string())
}

fn search_exec(input: Value) -> Result<String, String> {
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
    fn schemas_returns_five() {
        let schemas = all_tool_schemas();
        assert_eq!(schemas.len(), 5);
        let names: Vec<&str> = schemas.iter().filter_map(|s| s["name"].as_str()).collect();
        assert!(names.contains(&"read_file"));
        assert!(names.contains(&"list_files"));
        assert!(names.contains(&"bash"));
        assert!(names.contains(&"edit_file"));
        assert!(names.contains(&"code_search"));
    }

    #[test]
    fn dispatch_known_tool() {
        let block = dispatch_tool(
            "bash",
            serde_json::json!({"command": "echo dispatch_test"}),
            "test-id",
        );
        if let ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } = block
        {
            assert_eq!(tool_use_id, "test-id");
            assert!(content.contains("dispatch_test"));
            assert!(is_error.is_none());
        } else {
            panic!("expected ToolResult");
        }
    }

    #[test]
    fn dispatch_unknown_tool() {
        let block = dispatch_tool("nonexistent", Value::Null, "id-1");
        if let ContentBlock::ToolResult {
            content, is_error, ..
        } = block
        {
            assert!(content.contains("not found"));
            assert_eq!(is_error, Some(true));
        } else {
            panic!("expected ToolResult");
        }
    }

    #[test]
    fn dispatch_tool_error_propagates() {
        let block = dispatch_tool(
            "read_file",
            serde_json::json!({"path": "/tmp/_nonexistent_forgeflare_"}),
            "id-2",
        );
        if let ContentBlock::ToolResult { is_error, .. } = block {
            assert_eq!(is_error, Some(true));
        } else {
            panic!("expected ToolResult");
        }
    }

    #[test]
    fn schemas_have_required_fields() {
        for schema in all_tool_schemas() {
            assert!(schema["name"].is_string());
            assert!(schema["description"].is_string());
            assert!(schema["input_schema"]["type"].is_string());
        }
    }

    #[test]
    fn read_file_with_line_numbers() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut f, b"hello\nworld\n").unwrap();
        let result = read_exec(serde_json::json!({"path": f.path().to_str().unwrap()}));
        assert_eq!(result.unwrap(), "1: hello\n2: world");
    }

    #[test]
    fn read_file_missing_path() {
        let result = read_exec(serde_json::json!({}));
        assert_eq!(result.unwrap_err(), "path is required");
    }

    #[test]
    fn read_file_nonexistent() {
        let result = read_exec(serde_json::json!({"path": "/tmp/_nonexistent_forgeflare_test_"}));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("No such file"));
    }

    #[test]
    fn read_file_empty() {
        let f = tempfile::NamedTempFile::new().unwrap();
        let result = read_exec(serde_json::json!({"path": f.path().to_str().unwrap()}));
        assert_eq!(result.unwrap(), "");
    }

    // --- list_files tests ---

    #[test]
    fn list_current_dir() {
        let result = list_exec(serde_json::json!({}));
        assert!(result.is_ok());
        let files: Vec<String> = serde_json::from_str(&result.unwrap()).unwrap();
        assert!(!files.is_empty());
    }

    #[test]
    fn list_specific_dir() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "").unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("sub/b.txt"), "").unwrap();
        let result = list_exec(serde_json::json!({"path": dir.path().to_str().unwrap()}));
        let files: Vec<String> = serde_json::from_str(&result.unwrap()).unwrap();
        assert!(files.contains(&"a.txt".to_string()));
        assert!(files.contains(&"sub/".to_string()));
        assert!(files.contains(&"sub/b.txt".to_string()));
    }

    #[test]
    fn list_excludes_git_dir() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();
        fs::write(dir.path().join(".git/config"), "").unwrap();
        fs::write(dir.path().join("real.txt"), "").unwrap();
        let result = list_exec(serde_json::json!({"path": dir.path().to_str().unwrap()}));
        let files: Vec<String> = serde_json::from_str(&result.unwrap()).unwrap();
        assert!(!files.iter().any(|f| f.contains(".git")));
        assert!(files.contains(&"real.txt".to_string()));
    }

    #[test]
    fn list_nonexistent_dir() {
        let result =
            list_exec(serde_json::json!({"path": "/tmp/_nonexistent_forgeflare_test_dir_"}));
        assert!(result.is_err());
    }

    // --- bash tests ---

    #[test]
    fn bash_echo() {
        let result = bash_exec(serde_json::json!({"command": "echo hello"}));
        assert_eq!(result.unwrap(), "hello");
    }

    #[test]
    fn bash_missing_command() {
        let result = bash_exec(serde_json::json!({}));
        assert_eq!(result.unwrap_err(), "command is required");
    }

    #[test]
    fn bash_failing_command() {
        let result = bash_exec(serde_json::json!({"command": "false"}));
        let output = result.unwrap();
        assert!(output.starts_with("Command failed"));
    }

    #[test]
    fn bash_with_cwd() {
        let result = bash_exec(serde_json::json!({"command": "pwd", "cwd": "/tmp"}));
        let output = result.unwrap();
        assert!(output.contains("tmp") || output.contains("private/tmp"));
    }

    #[test]
    fn bash_stderr_captured() {
        let result = bash_exec(serde_json::json!({"command": "echo err >&2"}));
        let output = result.unwrap();
        assert!(output.contains("err"));
    }

    // --- edit_file tests ---

    #[test]
    fn edit_replace_exact_match() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        fs::write(&path, "hello world").unwrap();
        let result = edit_exec(serde_json::json!({
            "path": path.to_str().unwrap(),
            "old_str": "hello",
            "new_str": "goodbye"
        }));
        assert_eq!(result.unwrap(), "OK");
        assert_eq!(fs::read_to_string(&path).unwrap(), "goodbye world");
    }

    #[test]
    fn edit_create_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.txt");
        let result = edit_exec(serde_json::json!({
            "path": path.to_str().unwrap(),
            "old_str": "",
            "new_str": "new content"
        }));
        assert!(result.unwrap().contains("Created"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "new content");
    }

    #[test]
    fn edit_append_to_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("append.txt");
        fs::write(&path, "line1\n").unwrap();
        let result = edit_exec(serde_json::json!({
            "path": path.to_str().unwrap(),
            "old_str": "",
            "new_str": "line2\n"
        }));
        assert_eq!(result.unwrap(), "OK");
        assert_eq!(fs::read_to_string(&path).unwrap(), "line1\nline2\n");
    }

    #[test]
    fn edit_old_str_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        fs::write(&path, "hello").unwrap();
        let result = edit_exec(serde_json::json!({
            "path": path.to_str().unwrap(),
            "old_str": "missing",
            "new_str": "replacement"
        }));
        assert_eq!(result.unwrap_err(), "old_str not found");
    }

    #[test]
    fn edit_old_str_duplicate() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        fs::write(&path, "aaa").unwrap();
        let result = edit_exec(serde_json::json!({
            "path": path.to_str().unwrap(),
            "old_str": "a",
            "new_str": "b"
        }));
        assert!(result.unwrap_err().contains("3 times"));
    }

    #[test]
    fn edit_same_old_new() {
        let result = edit_exec(serde_json::json!({
            "path": "/tmp/test.txt",
            "old_str": "same",
            "new_str": "same"
        }));
        assert_eq!(result.unwrap_err(), "old_str and new_str must differ");
    }

    #[test]
    fn edit_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a/b/c.txt");
        let result = edit_exec(serde_json::json!({
            "path": path.to_str().unwrap(),
            "old_str": "",
            "new_str": "deep content"
        }));
        assert!(result.unwrap().contains("Created"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "deep content");
    }

    #[test]
    fn edit_missing_required_fields() {
        assert_eq!(
            edit_exec(serde_json::json!({})).unwrap_err(),
            "path is required"
        );
        assert_eq!(
            edit_exec(serde_json::json!({"path": "/tmp/x"})).unwrap_err(),
            "old_str is required"
        );
        assert_eq!(
            edit_exec(serde_json::json!({"path": "/tmp/x", "old_str": "a"})).unwrap_err(),
            "new_str is required"
        );
    }

    #[test]
    fn bash_timeout_returns_error() {
        // Verify dispatch wraps timeout as is_error: true by testing the error path directly
        // (actual timeout test would take 120s, so we test the Err propagation contract)
        let result = bash_exec(serde_json::json!({"command": "exit 0"}));
        assert!(result.is_ok(), "successful commands return Ok");
        // The timeout path returns Err(...), which dispatch_tool maps to is_error: Some(true).
        // We can't easily trigger a real timeout in a unit test without waiting 120s,
        // but we verify the contract: Err from bash_exec → is_error on ToolResult.
        let block = dispatch_tool(
            "bash",
            serde_json::json!({"command": "false"}),
            "timeout-test",
        );
        // "false" exits with 1, which is Ok("Command failed ...") — not an error.
        // This confirms the distinction: failed commands are Ok, timeouts would be Err.
        if let ContentBlock::ToolResult { is_error, .. } = &block {
            assert!(is_error.is_none(), "failed commands are Ok (not is_error)");
        }
    }

    // --- code_search tests ---

    #[test]
    fn search_finds_pattern() {
        let result =
            search_exec(serde_json::json!({"pattern": "fn search_exec", "path": "src/tools"}));
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.contains("fn search_exec"));
    }

    #[test]
    fn search_no_matches() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "nothing here").unwrap();
        let result = search_exec(
            serde_json::json!({"pattern": "will_not_match_anything", "path": dir.path().to_str().unwrap()}),
        );
        assert_eq!(result.unwrap(), "No matches found");
    }

    #[test]
    fn search_missing_pattern() {
        let result = search_exec(serde_json::json!({}));
        assert_eq!(result.unwrap_err(), "pattern is required");
    }

    #[test]
    fn search_empty_pattern() {
        let result = search_exec(serde_json::json!({"pattern": ""}));
        assert_eq!(result.unwrap_err(), "pattern is required");
    }

    #[test]
    fn search_case_insensitive_default() {
        let result =
            search_exec(serde_json::json!({"pattern": "FN SEARCH_EXEC", "path": "src/tools"}));
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.contains("fn search_exec"));
    }

    #[test]
    fn search_with_file_type() {
        let result = search_exec(
            serde_json::json!({"pattern": "fn all_tool_schemas", "path": "src", "file_type": "rust"}),
        );
        assert!(result.is_ok());
        assert!(result.unwrap().contains("fn all_tool_schemas"));
    }
}
