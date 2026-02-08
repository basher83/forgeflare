use super::{ToolDef, Value, schema};
use std::fs;
use std::path::Path;

pub fn tool() -> ToolDef {
    ToolDef {
        name: "list_files",
        description: "List files and directories. Defaults to current directory.",
        input_schema: schema!({"path", "string", "Optional path to list"} ;),
        function: execute,
    }
}

fn execute(input: Value) -> Result<String, String> {
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
