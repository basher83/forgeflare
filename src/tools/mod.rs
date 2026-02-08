pub mod bash;
pub mod edit;
pub mod list;
pub mod read;
pub mod search;

use crate::api::{ContentBlock, ToolSchema};
pub(crate) use serde_json::Value;

pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
    pub function: fn(Value) -> Result<String, String>,
}

macro_rules! schema {
    ($({ $name:expr, $ty:expr, $desc:expr }),* $(,)? ; $($req:expr),* $(,)?) => {
        serde_json::json!({
            "type": "object",
            "properties": { $( $name: { "type": $ty, "description": $desc } ),* },
            "required": [ $($req),* ]
        })
    };
    () => { serde_json::json!({"type": "object", "properties": {}, "required": []}) };
}
pub(crate) use schema;

fn registry_exec(_: Value) -> Result<String, String> {
    let listing: Vec<Value> = all_tools()
        .iter()
        .map(|t| serde_json::json!({"name": t.name, "description": t.description}))
        .collect();
    serde_json::to_string_pretty(&listing).map_err(|e| e.to_string())
}

pub fn all_tools() -> Vec<ToolDef> {
    vec![
        read::tool(),
        list::tool(),
        bash::tool(),
        edit::tool(),
        search::tool(),
        ToolDef {
            name: "registry",
            description: "List all available tools and their descriptions",
            input_schema: schema!(),
            function: registry_exec,
        },
    ]
}

pub fn tools_as_schemas(tools: &[ToolDef]) -> Vec<ToolSchema> {
    tools
        .iter()
        .map(|t| ToolSchema {
            name: t.name.to_string(),
            description: t.description.to_string(),
            input_schema: t.input_schema.clone(),
        })
        .collect()
}

pub fn dispatch_tool(tools: &[ToolDef], name: &str, input: Value, id: &str) -> ContentBlock {
    let (content, is_error) = tools
        .iter()
        .find(|t| t.name == name)
        .map(|t| match (t.function)(input) {
            Ok(s) => (s, None),
            Err(s) => (s, Some(true)),
        })
        .unwrap_or_else(|| (format!("tool '{name}' not found"), Some(true)));
    ContentBlock::ToolResult {
        tool_use_id: id.to_string(),
        content,
        is_error,
    }
}
