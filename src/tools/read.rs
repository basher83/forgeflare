use super::{ToolDef, Value, schema};

pub fn tool() -> ToolDef {
    ToolDef {
        name: "read_file",
        description: "Read a file's contents with line numbers",
        input_schema: schema!({"path", "string", "Relative file path"} ; "path"),
        function: execute,
    }
}

fn execute(input: Value) -> Result<String, String> {
    let path = input["path"].as_str().ok_or("path is required")?;
    let content = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    Ok(content
        .lines()
        .enumerate()
        .map(|(i, l)| format!("{}: {l}", i + 1))
        .collect::<Vec<_>>()
        .join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn read_file_with_line_numbers() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        write!(f, "hello\nworld\n").unwrap();
        let result = execute(serde_json::json!({"path": f.path().to_str().unwrap()}));
        assert_eq!(result.unwrap(), "1: hello\n2: world");
    }

    #[test]
    fn read_file_missing_path() {
        let result = execute(serde_json::json!({}));
        assert_eq!(result.unwrap_err(), "path is required");
    }

    #[test]
    fn read_file_nonexistent() {
        let result = execute(serde_json::json!({"path": "/tmp/_nonexistent_forgeflare_test_"}));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("No such file"));
    }

    #[test]
    fn read_file_empty() {
        let f = tempfile::NamedTempFile::new().unwrap();
        let result = execute(serde_json::json!({"path": f.path().to_str().unwrap()}));
        assert_eq!(result.unwrap(), "");
    }
}
