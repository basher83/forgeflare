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
