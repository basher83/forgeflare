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
