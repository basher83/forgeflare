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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_current_dir() {
        let result = execute(serde_json::json!({}));
        assert!(result.is_ok());
        let files: Vec<String> = serde_json::from_str(&result.unwrap()).unwrap();
        assert!(!files.is_empty());
    }

    #[test]
    fn list_specific_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/b.txt"), "").unwrap();
        let result = execute(serde_json::json!({"path": dir.path().to_str().unwrap()}));
        let files: Vec<String> = serde_json::from_str(&result.unwrap()).unwrap();
        assert!(files.contains(&"a.txt".to_string()));
        assert!(files.contains(&"sub/".to_string()));
        assert!(files.contains(&"sub/b.txt".to_string()));
    }

    #[test]
    fn list_excludes_git_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git/config"), "").unwrap();
        std::fs::write(dir.path().join("real.txt"), "").unwrap();
        let result = execute(serde_json::json!({"path": dir.path().to_str().unwrap()}));
        let files: Vec<String> = serde_json::from_str(&result.unwrap()).unwrap();
        assert!(!files.iter().any(|f| f.contains(".git")));
        assert!(files.contains(&"real.txt".to_string()));
    }

    #[test]
    fn list_nonexistent_dir() {
        let result = execute(serde_json::json!({"path": "/tmp/_nonexistent_forgeflare_test_dir_"}));
        assert!(result.is_err());
    }
}
