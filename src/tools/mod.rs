use crate::api::ContentBlock;
use serde_json::Value;
use std::{fs, io::Read, path::Path, process::Command, time::Duration};
use wait_timeout::ChildExt;

const BASH_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_READ_SIZE: u64 = 1024 * 1024; // 1MB
const MAX_BASH_OUTPUT: usize = 100 * 1024; // 100KB

/// Patterns that indicate destructive bash commands. Checked before execution.
const BLOCKED_PATTERNS: &[&str] = &[
    "rm -rf /",
    "rm -rf ~",
    "rm -rf .",
    "rm -rf *",
    "mkfs.",
    "of=/dev/sd",
    "of=/dev/nvme",
    "> /dev/sd",
    "chmod -r 777 /",
    ":(){ :|:& };:",
];

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
    "read_file", "Read the contents of a given relative file path with line numbers. Use this when you want to see what's inside a file. Do not use this with directory names.",
    serde_json::json!({"type": "object", "properties": {"path": {"type": "string", "description": "Relative file path"}}, "required": ["path"]}),
    read_exec;
    "list_files", "List files and directories at a given path. If no path is provided, lists files in the current directory.",
    serde_json::json!({"type": "object", "properties": {"path": {"type": "string", "description": "Optional path to list"}, "recursive": {"type": "boolean", "description": "Recurse into subdirectories (default: false)"}}, "required": []}),
    list_exec;
    "bash", "Execute a bash command and return its output. Use this to run shell commands. Commands are killed after 120s.",
    serde_json::json!({"type": "object", "properties": {"command": {"type": "string", "description": "The bash command to execute"}, "cwd": {"type": "string", "description": "Optional working directory"}}, "required": ["command"]}),
    bash_exec;
    "edit_file", "Make edits to a text file. Replaces 'old_str' with 'new_str' in the given file. 'old_str' and 'new_str' MUST be different from each other. If the file doesn't exist and old_str is empty, it will be created.",
    serde_json::json!({"type": "object", "properties": {"path": {"type": "string", "description": "The path to the file"}, "old_str": {"type": "string", "description": "Text to search for (must match exactly once). Empty string = create/append mode"}, "new_str": {"type": "string", "description": "Text to replace old_str with"}}, "required": ["path", "old_str", "new_str"]}),
    edit_exec;
    "code_search", "Search for code patterns using ripgrep (rg). Use this to find code patterns, function definitions, variable usage, or any text in the codebase.",
    serde_json::json!({"type": "object", "properties": {"pattern": {"type": "string", "description": "The search pattern or regex"}, "path": {"type": "string", "description": "Optional path to search in"}, "file_type": {"type": "string", "description": "File extension filter (e.g. 'go', 'js')"}, "case_sensitive": {"type": "boolean", "description": "Case sensitive (default: false)"}}, "required": ["pattern"]}),
    search_exec;
}

fn read_exec(input: Value) -> Result<String, String> {
    let path = input["path"].as_str().ok_or("path is required")?;
    let meta = fs::metadata(path).map_err(|e| format!("{path}: {e}"))?;
    if meta.len() > MAX_READ_SIZE {
        let (size, max) = (meta.len() / 1024, MAX_READ_SIZE / 1024);
        return Err(format!("{path}: {size}KB exceeds {max}KB limit"));
    }
    let raw = fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    if raw[..raw.len().min(8192)].contains(&0) {
        return Err(format!("{path}: binary file, cannot display contents"));
    }
    let content = String::from_utf8(raw).map_err(|_| format!("{path}: not valid UTF-8"))?;
    Ok(content
        .lines()
        .enumerate()
        .map(|(i, l)| format!("{}: {l}", i + 1))
        .collect::<Vec<_>>()
        .join("\n"))
}

const MAX_LIST_ENTRIES: usize = 1000;

fn list_exec(input: Value) -> Result<String, String> {
    let dir = input["path"].as_str().unwrap_or(".");
    let recursive = input["recursive"].as_bool().unwrap_or(false);
    let mut files = Vec::new();
    walk(Path::new(dir), Path::new(dir), &mut files, recursive, 0).map_err(|e| e.to_string())?;
    files.sort();
    let total = files.len();
    if total > MAX_LIST_ENTRIES {
        files.truncate(MAX_LIST_ENTRIES);
        let mut out = serde_json::to_string(&files).map_err(|e| e.to_string())?;
        out.push_str(&format!(
            "\n... (showing {MAX_LIST_ENTRIES} of {total} entries)"
        ));
        return Ok(out);
    }
    serde_json::to_string(&files).map_err(|e| e.to_string())
}

const SKIP_DIRS: &[&str] = &[
    ".git",
    ".devenv",
    "node_modules",
    "target",
    ".venv",
    "vendor",
];

const MAX_WALK_DEPTH: usize = 20;

fn walk(
    base: &Path,
    dir: &Path,
    files: &mut Vec<String>,
    recursive: bool,
    depth: usize,
) -> std::io::Result<()> {
    if depth > MAX_WALK_DEPTH {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let rel = path.strip_prefix(base).unwrap_or(&path).to_string_lossy();
        if entry.file_type()?.is_dir() {
            let name = path.file_name().unwrap_or_default();
            if SKIP_DIRS.iter().any(|s| *s == name) {
                continue;
            }
            files.push(format!("{rel}/"));
            if recursive {
                walk(base, &path, files, recursive, depth + 1)?;
            }
        } else {
            files.push(rel.into_owned());
        }
    }
    Ok(())
}

fn truncate_with_marker(s: &mut String, max: usize) {
    let end = (0..=max)
        .rev()
        .find(|&i| s.is_char_boundary(i))
        .unwrap_or(0);
    s.truncate(end);
    s.push_str("\n... (output truncated at 100KB)");
}

fn bash_exec(input: Value) -> Result<String, String> {
    let command = input["command"].as_str().ok_or("command is required")?;
    let lower = command.to_lowercase();
    if let Some(pat) = BLOCKED_PATTERNS.iter().find(|p| lower.contains(*p)) {
        return Err(format!(
            "blocked: command matches dangerous pattern '{pat}'"
        ));
    }
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
    fn drain<R: Read + Send + 'static>(mut r: R) -> std::thread::JoinHandle<String> {
        std::thread::spawn(move || {
            let mut s = String::new();
            r.read_to_string(&mut s).ok();
            s
        })
    }
    let out_h = drain(child.stdout.take().ok_or("failed to capture stdout")?);
    let err_h = drain(child.stderr.take().ok_or("failed to capture stderr")?);

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
    let stdout = out_h.join().map_err(|_| "stdout reader thread panicked")?;
    let stderr = err_h.join().map_err(|_| "stderr reader thread panicked")?;
    let mut output = if !stdout.is_empty() && !stderr.is_empty() {
        format!("{stdout}\n--- stderr ---\n{stderr}")
    } else {
        format!("{stdout}{stderr}")
    }
    .trim()
    .to_string();
    if !status.success() {
        let mut msg = format!("Command failed ({status}): {output}");
        if msg.len() > MAX_BASH_OUTPUT {
            truncate_with_marker(&mut msg, MAX_BASH_OUTPUT);
        }
        return Err(msg);
    }
    if output.len() > MAX_BASH_OUTPUT {
        truncate_with_marker(&mut output, MAX_BASH_OUTPUT);
    }
    Ok(output)
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
    let meta = fs::metadata(path).map_err(|e| format!("{path_s}: {e}"))?;
    if meta.len() > MAX_READ_SIZE {
        let (size, max) = (meta.len() / 1024, MAX_READ_SIZE / 1024);
        return Err(format!("{path_s}: {size}KB exceeds {max}KB edit limit"));
    }
    let content = fs::read_to_string(path).map_err(|e| format!("{path_s}: {e}"))?;
    if old_str.is_empty() {
        fs::write(path, format!("{content}{new_str}")).map_err(|e| format!("write: {e}"))?;
    } else {
        match content.matches(old_str).count() {
            0 => return Err("old_str not found".into()),
            1 => {}
            n => return Err(format!("old_str found {n} times, must be unique")),
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
        args.extend(["--type", ft]);
    }
    args.extend([pattern, path]);
    let output = Command::new("rg")
        .args(&args)
        .output()
        .map_err(|e| format!("rg failed: {e}"))?;
    if output.status.code() == Some(1) {
        return Ok("No matches found".into());
    }
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(format!("search failed: {err}"));
    }
    let mut result = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if result.len() > MAX_BASH_OUTPUT {
        truncate_with_marker(&mut result, MAX_BASH_OUTPUT);
        return Ok(result);
    }
    let lines: Vec<&str> = result.lines().collect();
    if lines.len() <= 50 {
        return Ok(result);
    }
    let shown = lines[..50].join("\n");
    let total = lines.len();
    Ok(format!("{shown}\n... (showing 50 of {total} matches)"))
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

    #[test]
    fn read_file_binary_detected() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut f, b"\x00\x01\x02binary").unwrap();
        let result = read_exec(serde_json::json!({"path": f.path().to_str().unwrap()}));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("binary file"));
    }

    #[test]
    fn read_file_size_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.txt");
        // Write 2MB file (exceeds 1MB limit)
        let data = "x".repeat(2 * 1024 * 1024);
        fs::write(&path, &data).unwrap();
        let result = read_exec(serde_json::json!({"path": path.to_str().unwrap()}));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("exceeds"));
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
    fn list_specific_dir_recursive() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "").unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("sub/b.txt"), "").unwrap();
        let result =
            list_exec(serde_json::json!({"path": dir.path().to_str().unwrap(), "recursive": true}));
        let files: Vec<String> = serde_json::from_str(&result.unwrap()).unwrap();
        assert!(files.contains(&"a.txt".to_string()));
        assert!(files.contains(&"sub/".to_string()));
        assert!(files.contains(&"sub/b.txt".to_string()));
    }

    #[test]
    fn list_shallow_by_default() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "").unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("sub/b.txt"), "").unwrap();
        let result = list_exec(serde_json::json!({"path": dir.path().to_str().unwrap()}));
        let files: Vec<String> = serde_json::from_str(&result.unwrap()).unwrap();
        assert!(files.contains(&"a.txt".to_string()));
        assert!(files.contains(&"sub/".to_string()));
        assert!(!files.contains(&"sub/b.txt".to_string()));
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
    fn list_excludes_nested_git_dir() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("sub/.git")).unwrap();
        fs::write(dir.path().join("sub/.git/config"), "").unwrap();
        fs::write(dir.path().join("sub/real.txt"), "").unwrap();
        let result =
            list_exec(serde_json::json!({"path": dir.path().to_str().unwrap(), "recursive": true}));
        let files: Vec<String> = serde_json::from_str(&result.unwrap()).unwrap();
        assert!(!files.iter().any(|f| f.contains(".git")));
        assert!(files.contains(&"sub/real.txt".to_string()));
    }

    #[test]
    fn list_excludes_skip_dirs() {
        let dir = tempfile::tempdir().unwrap();
        for skip in &["node_modules", "target", ".venv", "vendor"] {
            fs::create_dir(dir.path().join(skip)).unwrap();
        }
        fs::write(dir.path().join("keep.txt"), "").unwrap();
        let result = list_exec(serde_json::json!({"path": dir.path().to_str().unwrap()}));
        let files: Vec<String> = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(files, vec!["keep.txt"]);
    }

    #[test]
    fn list_nonexistent_dir() {
        let result =
            list_exec(serde_json::json!({"path": "/tmp/_nonexistent_forgeflare_test_dir_"}));
        assert!(result.is_err());
    }

    #[test]
    fn walk_respects_depth_limit() {
        let dir = tempfile::tempdir().unwrap();
        // Create a directory tree deeper than MAX_WALK_DEPTH (20)
        let mut path = dir.path().to_path_buf();
        for i in 0..25 {
            path = path.join(format!("d{i}"));
            fs::create_dir(&path).unwrap();
            fs::write(path.join("file.txt"), "").unwrap();
        }
        let result =
            list_exec(serde_json::json!({"path": dir.path().to_str().unwrap(), "recursive": true}));
        let files: Vec<String> = serde_json::from_str(&result.unwrap()).unwrap();
        // Files at depth 25 should NOT appear (limit is 20)
        assert!(
            !files.iter().any(|f| f.contains("d24/file.txt")),
            "files beyond depth limit should be excluded"
        );
        // Files at depth 1 should appear
        assert!(
            files.iter().any(|f| f.contains("d0/file.txt")),
            "files within depth limit should be present"
        );
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
        let err = result.unwrap_err();
        assert!(err.starts_with("Command failed"));
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

    #[test]
    fn bash_stdout_stderr_separated() {
        // When both stdout and stderr have content, they should be labeled and separated
        let result = bash_exec(serde_json::json!({"command": "echo out; echo err >&2"}));
        let output = result.unwrap();
        assert!(output.contains("out"), "should contain stdout");
        assert!(output.contains("err"), "should contain stderr");
        assert!(
            output.contains("--- stderr ---"),
            "should have labeled stderr separator: {output}"
        );
    }

    #[test]
    fn bash_blocks_dangerous_rm_rf() {
        let result = bash_exec(serde_json::json!({"command": "rm -rf /"}));
        let err = result.unwrap_err();
        assert!(err.contains("blocked"), "should block rm -rf /: {err}");
    }

    #[test]
    fn bash_blocks_fork_bomb() {
        let result = bash_exec(serde_json::json!({"command": ":(){ :|:& };:"}));
        let err = result.unwrap_err();
        assert!(err.contains("blocked"), "should block fork bomb: {err}");
    }

    #[test]
    fn bash_blocks_dd_to_device() {
        let result = bash_exec(serde_json::json!({"command": "dd if=/dev/zero of=/dev/sda"}));
        let err = result.unwrap_err();
        assert!(err.contains("blocked"), "should block dd to device: {err}");
    }

    #[test]
    fn bash_blocks_chmod_777_root() {
        let result = bash_exec(serde_json::json!({"command": "chmod -R 777 /"}));
        let err = result.unwrap_err();
        assert!(err.contains("blocked"), "should block chmod 777 /: {err}");
    }

    #[test]
    fn bash_blocks_mkfs() {
        let result = bash_exec(serde_json::json!({"command": "mkfs.ext4 /dev/sda1"}));
        let err = result.unwrap_err();
        assert!(err.contains("blocked"), "should block mkfs: {err}");
    }

    #[test]
    fn bash_allows_safe_commands() {
        // Ensure the guard doesn't block normal commands
        let result = bash_exec(serde_json::json!({"command": "echo hello"}));
        assert!(result.is_ok(), "safe commands should not be blocked");
    }

    #[test]
    fn bash_output_truncated() {
        // Generate output larger than MAX_BASH_OUTPUT (100KB) using printf
        let result = bash_exec(
            serde_json::json!({"command": "dd if=/dev/zero bs=1024 count=200 2>/dev/null | tr '\\0' 'x'"}),
        );
        let output = result.unwrap();
        assert!(output.contains("truncated at 100KB"));
        assert!(output.len() <= 110 * 1024); // 100KB + truncation message
    }

    #[test]
    fn truncate_with_marker_respects_char_boundary() {
        // 'é' is 2 bytes (0xC3 0xA9); truncating at byte 1 would split the char
        let mut s = "é".repeat(100);
        truncate_with_marker(&mut s, 5); // 5 bytes → 2 full 'é' chars (4 bytes)
        assert!(s.starts_with("éé"));
        assert!(s.contains("truncated at 100KB"));
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
    fn edit_delete_text() {
        // Replacing with empty new_str effectively deletes the matched text
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        fs::write(&path, "keep DELETE_ME keep").unwrap();
        let result = edit_exec(serde_json::json!({
            "path": path.to_str().unwrap(),
            "old_str": " DELETE_ME",
            "new_str": ""
        }));
        assert_eq!(result.unwrap(), "OK");
        assert_eq!(fs::read_to_string(&path).unwrap(), "keep keep");
    }

    #[test]
    fn edit_empty_old_and_new_rejected() {
        // When both old_str and new_str are empty, the no-op check rejects it.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        fs::write(&path, "content").unwrap();
        let result = edit_exec(serde_json::json!({
            "path": path.to_str().unwrap(),
            "old_str": "",
            "new_str": ""
        }));
        assert_eq!(result.unwrap_err(), "old_str and new_str must differ");
    }

    #[test]
    fn bash_error_output_truncated() {
        // Error path should also truncate oversized output
        let result = bash_exec(
            serde_json::json!({"command": "dd if=/dev/zero bs=1024 count=200 2>/dev/null | tr '\\0' 'x'; exit 1"}),
        );
        let err = result.unwrap_err();
        assert!(
            err.contains("truncated at 100KB"),
            "error output should be truncated: {err}"
        );
        assert!(err.len() <= 110 * 1024); // 100KB + prefix + truncation message
    }

    #[test]
    fn list_files_output_is_sorted() {
        let dir = tempfile::tempdir().unwrap();
        // Create files in reverse alphabetical order
        for name in &["zebra.txt", "apple.txt", "mango.txt"] {
            fs::write(dir.path().join(name), "").unwrap();
        }
        let result = list_exec(serde_json::json!({"path": dir.path().to_str().unwrap()}));
        let files: Vec<String> = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(files, vec!["apple.txt", "mango.txt", "zebra.txt"]);
    }

    #[test]
    fn bash_failed_command_signals_is_error() {
        // Non-zero exit returns Err, which dispatch_tool maps to is_error: Some(true).
        // This matches the Anthropic API protocol: tool failures should set is_error.
        let result = bash_exec(serde_json::json!({"command": "exit 0"}));
        assert!(result.is_ok(), "successful commands return Ok");

        let block = dispatch_tool(
            "bash",
            serde_json::json!({"command": "false"}),
            "error-test",
        );
        if let ContentBlock::ToolResult {
            content, is_error, ..
        } = &block
        {
            assert_eq!(*is_error, Some(true), "failed commands set is_error");
            assert!(content.contains("Command failed"));
        } else {
            panic!("expected ToolResult");
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

    #[test]
    fn search_invalid_regex() {
        // rg returns exit code 2 for invalid regex; should surface as error
        let result = search_exec(serde_json::json!({"pattern": "[invalid(regex"}));
        assert!(result.is_err(), "invalid regex should error");
        let err = result.unwrap_err();
        assert!(
            err.contains("search failed"),
            "should contain error context: {err}"
        );
    }

    #[test]
    fn dispatch_null_input_returns_error() {
        // Corrupt tool_use blocks from SSE parse failures have Value::Null input.
        // Tools with required parameters should return is_error.
        // list_files has no required parameters, so null input succeeds (lists cwd).
        for name in ["read_file", "bash", "edit_file", "code_search"] {
            let block = dispatch_tool(name, Value::Null, "null-test");
            if let ContentBlock::ToolResult { is_error, .. } = &block {
                assert_eq!(*is_error, Some(true), "{name} should error on null input");
            } else {
                panic!("expected ToolResult for {name}");
            }
        }
    }

    #[test]
    fn edit_rejects_oversized_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.txt");
        let data = "x".repeat(2 * 1024 * 1024); // 2MB > 1MB limit
        fs::write(&path, &data).unwrap();
        let result = edit_exec(serde_json::json!({
            "path": path.to_str().unwrap(),
            "old_str": "x",
            "new_str": "y"
        }));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("exceeds"));
    }

    #[test]
    fn search_truncates_at_50_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("many.txt");
        // 100 lines matching "hit" — result should be capped at 50
        let content: String = (0..100).map(|i| format!("hit line {i}\n")).collect();
        fs::write(&path, &content).unwrap();
        let result = search_exec(
            serde_json::json!({"pattern": "hit", "path": dir.path().to_str().unwrap()}),
        );
        let output = result.unwrap();
        assert!(
            output.contains("showing 50 of"),
            "should indicate truncation: {output}"
        );
    }

    #[test]
    fn bash_invalid_cwd_returns_error() {
        let result = bash_exec(
            serde_json::json!({"command": "pwd", "cwd": "/tmp/_nonexistent_forgeflare_dir_"}),
        );
        assert!(result.is_err(), "invalid cwd should error");
    }

    #[test]
    fn list_files_caps_at_max_entries() {
        let dir = tempfile::tempdir().unwrap();
        // Create 1100 files to exceed the 1000 cap
        for i in 0..1100 {
            fs::write(dir.path().join(format!("f{i:04}.txt")), "").unwrap();
        }
        let result = list_exec(serde_json::json!({"path": dir.path().to_str().unwrap()}));
        let output = result.unwrap();
        assert!(
            output.contains("showing 1000 of 1100"),
            "should indicate truncation: {output}"
        );
    }
}
