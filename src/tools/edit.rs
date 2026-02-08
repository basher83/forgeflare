use super::{ToolDef, Value, schema};
use std::fs;
use std::path::Path;

pub fn tool() -> ToolDef {
    ToolDef {
        name: "edit_file",
        description: "Replace old_str with new_str in a file. Creates file if missing.",
        input_schema: schema!(
            {"path", "string", "The path to the file"},
            {"old_str", "string", "Text to search for - must match exactly once"},
            {"new_str", "string", "Text to replace old_str with"}
            ; "path", "old_str", "new_str"
        ),
        function: execute,
    }
}

fn execute(input: Value) -> Result<String, String> {
    let path_s = input["path"].as_str().ok_or("path is required")?;
    let old_str = input["old_str"].as_str().ok_or("old_str is required")?;
    let new_str = input["new_str"].as_str().ok_or("new_str is required")?;
    if old_str == new_str {
        return Err("old_str and new_str must differ".into());
    }
    let path = Path::new(path_s);
    if !path.exists() && old_str.is_empty() {
        if let Some(p) = path
            .parent()
            .filter(|p| *p != Path::new("") && *p != Path::new("."))
        {
            fs::create_dir_all(p).map_err(|e| format!("mkdir: {e}"))?;
        }
        fs::write(path, new_str).map_err(|e| format!("write: {e}"))?;
        return Ok(format!("Created {path_s}"));
    }
    let content = fs::read_to_string(path).map_err(|e| format!("{path_s}: {e}"))?;
    let new_content = if old_str.is_empty() {
        format!("{content}{new_str}")
    } else {
        let count = content.matches(old_str).count();
        if count == 0 {
            return Err("old_str not found".into());
        }
        if count > 1 {
            return Err(format!("old_str found {count} times, must be unique"));
        }
        content.replacen(old_str, new_str, 1)
    };
    fs::write(path, new_content).map_err(|e| format!("write: {e}"))?;
    Ok("OK".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_replace_exact_match() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        std::fs::write(&path, "hello world").unwrap();
        let result = execute(serde_json::json!({
            "path": path.to_str().unwrap(),
            "old_str": "hello",
            "new_str": "goodbye"
        }));
        assert_eq!(result.unwrap(), "OK");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "goodbye world");
    }

    #[test]
    fn edit_create_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.txt");
        let result = execute(serde_json::json!({
            "path": path.to_str().unwrap(),
            "old_str": "",
            "new_str": "new content"
        }));
        assert!(result.unwrap().contains("Created"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new content");
    }

    #[test]
    fn edit_append_to_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("append.txt");
        std::fs::write(&path, "line1\n").unwrap();
        let result = execute(serde_json::json!({
            "path": path.to_str().unwrap(),
            "old_str": "",
            "new_str": "line2\n"
        }));
        assert_eq!(result.unwrap(), "OK");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "line1\nline2\n");
    }

    #[test]
    fn edit_old_str_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        std::fs::write(&path, "hello").unwrap();
        let result = execute(serde_json::json!({
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
        std::fs::write(&path, "aaa").unwrap();
        let result = execute(serde_json::json!({
            "path": path.to_str().unwrap(),
            "old_str": "a",
            "new_str": "b"
        }));
        assert!(result.unwrap_err().contains("3 times"));
    }

    #[test]
    fn edit_same_old_new() {
        let result = execute(serde_json::json!({
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
        let result = execute(serde_json::json!({
            "path": path.to_str().unwrap(),
            "old_str": "",
            "new_str": "deep content"
        }));
        assert!(result.unwrap().contains("Created"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "deep content");
    }

    #[test]
    fn edit_missing_required_fields() {
        assert_eq!(
            execute(serde_json::json!({})).unwrap_err(),
            "path is required"
        );
        assert_eq!(
            execute(serde_json::json!({"path": "/tmp/x"})).unwrap_err(),
            "old_str is required"
        );
        assert_eq!(
            execute(serde_json::json!({"path": "/tmp/x", "old_str": "a"})).unwrap_err(),
            "new_str is required"
        );
    }
}
