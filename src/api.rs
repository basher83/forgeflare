use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::Write;

#[derive(thiserror::Error, Debug)]
pub enum AgentError {
    #[error("API: {0}")]
    Api(#[from] reqwest::Error),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("ANTHROPIC_API_KEY not set")]
    MissingApiKey,
    #[error("stream: {0}")]
    StreamParse(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

// R8: Subagent coordination types (dispatch not yet implemented)
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SubagentContext {
    pub subagent_id: Option<String>,
}

pub struct AnthropicClient {
    client: reqwest::Client,
    api_key: String,
}

impl AnthropicClient {
    pub fn new() -> Result<Self, AgentError> {
        let api_key = std::env::var("ANTHROPIC_API_KEY").map_err(|_| AgentError::MissingApiKey)?;
        Ok(Self {
            client: reqwest::Client::new(),
            api_key,
        })
    }

    pub async fn send_message(
        &self,
        messages: &[Message],
        tools: &[Value],
        model: &str,
    ) -> Result<(Vec<ContentBlock>, StopReason), AgentError> {
        let body = serde_json::json!({
            "model": model, "max_tokens": 16384, "stream": true,
            "system": "You are a coding agent with tools for reading files, listing directories, running bash commands, editing files, and searching code.\n\nWorkflow: 1) Understand the request. 2) Explore relevant code with read_file and code_search before making changes. 3) Plan your approach. 4) Make targeted edits. 5) Verify changes work.\n\nRules:\n- ALWAYS read a file before editing it. Never edit blind.\n- Use code_search to find relevant code across the project before making assumptions.\n- Make minimal, focused changes. Don't refactor unrelated code.\n- When editing, include enough context in old_str to match exactly once.\n- Verify edits by reading the file after changes.\n- For bash commands: avoid destructive operations (rm -rf, force push) without explicit user approval.\n- If a command fails, analyze the error before retrying with a different approach.\n- Explain what you're doing and why, but be concise.",
            "messages": messages, "tools": tools
        });
        let response = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;
        if !response.status().is_success() {
            let (status, body) = (response.status(), response.text().await.unwrap_or_default());
            return Err(AgentError::StreamParse(format!(
                "API returned {status}: {body}"
            )));
        }

        let mut stream = response.bytes_stream();
        let (mut buf, mut event) = (String::new(), String::new());
        let mut blocks: Vec<ContentBlock> = Vec::new();
        let mut fragments: Vec<String> = Vec::new();
        let mut stop_reason: Option<StopReason> = None;
        let mut message_complete = false;

        while let Some(chunk) = stream.next().await {
            buf.push_str(&String::from_utf8_lossy(&chunk?));
            while let Some(nl) = buf.find('\n') {
                let line = buf[..nl].trim_end().to_string();
                buf.drain(..nl + 1);
                if line.is_empty() {
                    continue;
                }
                if let Some(ev) = line.strip_prefix("event: ") {
                    event = ev.into();
                    continue;
                }
                let Some(data) = line.strip_prefix("data: ") else {
                    continue;
                };
                let p: Value = serde_json::from_str(data)?;
                match event.as_str() {
                    "content_block_start" => {
                        let b = &p["content_block"];
                        if b["type"].as_str() == Some("tool_use") {
                            blocks.push(ContentBlock::ToolUse {
                                id: b["id"].as_str().unwrap_or_default().into(),
                                name: b["name"].as_str().unwrap_or_default().into(),
                                input: Value::Null,
                            });
                        } else {
                            // Text and unknown types (thinking, server_tool_use, etc.):
                            // placeholder keeps blocks[] and fragments[] indices aligned
                            blocks.push(ContentBlock::Text {
                                text: String::new(),
                            });
                        }
                        fragments.push(String::new());
                    }
                    "content_block_delta" => {
                        let Some(idx) = p["index"].as_u64().map(|i| i as usize) else {
                            continue;
                        };
                        let delta = &p["delta"];
                        match delta["type"].as_str() {
                            Some("text_delta") => {
                                let t = delta["text"].as_str().unwrap_or_default();
                                print!("\x1b[93m{t}\x1b[0m");
                                std::io::stdout().flush().ok();
                                if let Some(ContentBlock::Text { text }) = blocks.get_mut(idx) {
                                    text.push_str(t);
                                }
                            }
                            Some("input_json_delta") => {
                                if let Some(f) = fragments.get_mut(idx) {
                                    f.push_str(delta["partial_json"].as_str().unwrap_or_default());
                                }
                            }
                            _ => {}
                        }
                    }
                    "content_block_stop" => {
                        let Some(idx) = p["index"].as_u64().map(|i| i as usize) else {
                            continue;
                        };
                        if let Some(ContentBlock::ToolUse { input, .. }) = blocks.get_mut(idx)
                            && let Some(f) = fragments.get(idx).filter(|f| !f.is_empty())
                        {
                            *input = serde_json::from_str(f).unwrap_or_else(|e| {
                                eprintln!("\x1b[91m[warning]\x1b[0m Corrupt tool input (JSON parse failed: {e})");
                                Value::Null
                            });
                        }
                        if let Some(ContentBlock::Text { text }) = blocks.get(idx)
                            && !text.is_empty()
                        {
                            println!();
                        }
                    }
                    "message_delta" => match p["delta"]["stop_reason"].as_str() {
                        Some("end_turn") => stop_reason = Some(StopReason::EndTurn),
                        Some("tool_use") => stop_reason = Some(StopReason::ToolUse),
                        Some("max_tokens") => stop_reason = Some(StopReason::MaxTokens),
                        _ => {}
                    },
                    "message_stop" => message_complete = true,
                    "error" => {
                        let msg = p["error"]["message"]
                            .as_str()
                            .unwrap_or("unknown stream error");
                        return Err(AgentError::StreamParse(format!("stream error: {msg}")));
                    }
                    _ => {}
                }
            }
        }
        // Filter out placeholder blocks from unknown SSE content types (e.g. thinking)
        blocks.retain(|b| !matches!(b, ContentBlock::Text { text } if text.is_empty()));
        // Detect incomplete streams: if message_delta never delivered a stop_reason,
        // the connection was dropped mid-stream (message_complete is a defensive fallback)
        let stop_reason = stop_reason
            .or(message_complete.then_some(StopReason::EndTurn))
            .ok_or_else(|| {
                AgentError::StreamParse(
                    "stream ended without stop_reason (connection dropped?)".into(),
                )
            })?;
        Ok((blocks, stop_reason))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_block_serialization() {
        let block = ContentBlock::Text {
            text: "hello".into(),
        };
        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["type"], "text");
        assert_eq!(json["text"], "hello");
    }

    #[test]
    fn tool_use_block_serialization() {
        let block = ContentBlock::ToolUse {
            id: "id-1".into(),
            name: "bash".into(),
            input: serde_json::json!({"command": "ls"}),
        };
        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["type"], "tool_use");
        assert_eq!(json["id"], "id-1");
        assert_eq!(json["name"], "bash");
        assert_eq!(json["input"]["command"], "ls");
    }

    #[test]
    fn tool_result_block_serialization() {
        let block = ContentBlock::ToolResult {
            tool_use_id: "id-1".into(),
            content: "output".into(),
            is_error: None,
        };
        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["type"], "tool_result");
        assert_eq!(json["tool_use_id"], "id-1");
        assert!(json.get("is_error").is_none());
    }

    #[test]
    fn tool_result_with_error_flag() {
        let block = ContentBlock::ToolResult {
            tool_use_id: "id-2".into(),
            content: "not found".into(),
            is_error: Some(true),
        };
        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["is_error"], true);
    }

    #[test]
    fn message_roundtrip() {
        let msg = Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "test".into(),
            }],
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: Message = serde_json::from_str(&json).unwrap();
        assert!(matches!(decoded.role, Role::User));
        assert_eq!(decoded.content.len(), 1);
    }

    #[test]
    fn content_block_deserialization() {
        let json = r#"{"type":"text","text":"hello"}"#;
        let block: ContentBlock = serde_json::from_str(json).unwrap();
        assert!(matches!(block, ContentBlock::Text { text } if text == "hello"));
    }

    #[test]
    fn tool_use_deserialization() {
        let json = r#"{"type":"tool_use","id":"abc","name":"bash","input":{"command":"ls"}}"#;
        let block: ContentBlock = serde_json::from_str(json).unwrap();
        if let ContentBlock::ToolUse { id, name, input } = block {
            assert_eq!(id, "abc");
            assert_eq!(name, "bash");
            assert_eq!(input["command"], "ls");
        } else {
            panic!("expected ToolUse");
        }
    }

    #[test]
    fn role_serialization() {
        assert_eq!(serde_json::to_string(&Role::User).unwrap(), r#""user""#);
        assert_eq!(
            serde_json::to_string(&Role::Assistant).unwrap(),
            r#""assistant""#
        );
    }

    #[test]
    fn tool_schema_as_value() {
        let schema = serde_json::json!({
            "name": "test",
            "description": "A test tool",
            "input_schema": {"type": "object"}
        });
        assert_eq!(schema["name"], "test");
        assert_eq!(schema["input_schema"]["type"], "object");
    }

    #[test]
    fn message_with_tool_result() {
        let msg = Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "id-1".into(),
                content: "result".into(),
                is_error: None,
            }],
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["role"], "user");
        assert_eq!(json["content"][0]["type"], "tool_result");
    }

    #[test]
    fn stop_reason_equality() {
        assert_eq!(StopReason::EndTurn, StopReason::EndTurn);
        assert_eq!(StopReason::ToolUse, StopReason::ToolUse);
        assert_eq!(StopReason::MaxTokens, StopReason::MaxTokens);
        assert_ne!(StopReason::EndTurn, StopReason::ToolUse);
        assert_ne!(StopReason::EndTurn, StopReason::MaxTokens);
        assert_ne!(StopReason::ToolUse, StopReason::MaxTokens);
    }

    #[test]
    fn stop_reason_debug_format() {
        assert_eq!(format!("{:?}", StopReason::EndTurn), "EndTurn");
        assert_eq!(format!("{:?}", StopReason::ToolUse), "ToolUse");
        assert_eq!(format!("{:?}", StopReason::MaxTokens), "MaxTokens");
    }

    #[test]
    fn empty_text_blocks_filtered() {
        // Placeholder blocks (from unknown SSE content types like thinking)
        // should be filtered out before returning from send_message.
        // This tests the retain filter logic directly.
        let mut blocks = vec![
            ContentBlock::Text {
                text: String::new(),
            }, // placeholder for unknown type
            ContentBlock::Text {
                text: "real content".into(),
            },
            ContentBlock::ToolUse {
                id: "t1".into(),
                name: "bash".into(),
                input: serde_json::json!({"command": "ls"}),
            },
            ContentBlock::Text {
                text: String::new(),
            }, // another placeholder
        ];
        blocks.retain(|b| !matches!(b, ContentBlock::Text { text } if text.is_empty()));
        assert_eq!(blocks.len(), 2);
        assert!(matches!(&blocks[0], ContentBlock::Text { text } if text == "real content"));
        assert!(matches!(&blocks[1], ContentBlock::ToolUse { .. }));
    }
}
