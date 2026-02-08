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

#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct SubagentContext {
    pub subagent_id: Option<String>,
} // R8: future dispatch

#[derive(Default)]
struct SseParser {
    event: String,
    blocks: Vec<ContentBlock>,
    fragments: Vec<String>,
    stop_reason: Option<StopReason>,
    message_complete: bool,
}

impl SseParser {
    fn process_line(&mut self, line: &str) -> Result<(), AgentError> {
        if line.is_empty() {
            return Ok(());
        }
        if let Some(ev) = line.strip_prefix("event: ") {
            self.event = ev.into();
            return Ok(());
        }
        let Some(data) = line.strip_prefix("data: ") else {
            return Ok(());
        };
        let p: Value = serde_json::from_str(data)?;
        match self.event.as_str() {
            "content_block_start" => {
                let b = &p["content_block"];
                if b["type"].as_str() == Some("tool_use") {
                    self.blocks.push(ContentBlock::ToolUse {
                        id: b["id"].as_str().unwrap_or_default().into(),
                        name: b["name"].as_str().unwrap_or_default().into(),
                        input: Value::Null,
                    });
                } else {
                    // Placeholder for text/unknown types — keeps indices aligned
                    self.blocks.push(ContentBlock::Text {
                        text: String::new(),
                    });
                }
                self.fragments.push(String::new());
            }
            "content_block_delta" => {
                let Some(idx) = p["index"].as_u64().map(|i| i as usize) else {
                    return Ok(());
                };
                let delta = &p["delta"];
                match delta["type"].as_str() {
                    Some("text_delta") => {
                        let t = delta["text"].as_str().unwrap_or_default();
                        print!("\x1b[93m{t}\x1b[0m");
                        std::io::stdout().flush().ok();
                        if let Some(ContentBlock::Text { text }) = self.blocks.get_mut(idx) {
                            text.push_str(t);
                        }
                    }
                    Some("input_json_delta") => {
                        if let Some(f) = self.fragments.get_mut(idx) {
                            f.push_str(delta["partial_json"].as_str().unwrap_or_default());
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                let Some(idx) = p["index"].as_u64().map(|i| i as usize) else {
                    return Ok(());
                };
                if let Some(ContentBlock::ToolUse { input, .. }) = self.blocks.get_mut(idx)
                    && let Some(f) = self.fragments.get(idx).filter(|f| !f.is_empty())
                {
                    *input = serde_json::from_str(f).unwrap_or_else(|e| {
                        eprintln!(
                            "\x1b[91m[warning]\x1b[0m Corrupt tool input (JSON parse failed: {e})"
                        );
                        Value::Null
                    });
                }
                if let Some(ContentBlock::Text { text }) = self.blocks.get(idx)
                    && !text.is_empty()
                {
                    println!();
                }
            }
            "message_delta" => match p["delta"]["stop_reason"].as_str() {
                Some("end_turn") => self.stop_reason = Some(StopReason::EndTurn),
                Some("tool_use") => self.stop_reason = Some(StopReason::ToolUse),
                Some("max_tokens") => self.stop_reason = Some(StopReason::MaxTokens),
                _ => {}
            },
            "message_stop" => self.message_complete = true,
            "error" => {
                let msg = p["error"]["message"]
                    .as_str()
                    .unwrap_or("unknown stream error");
                return Err(AgentError::StreamParse(format!("stream error: {msg}")));
            }
            _ => {}
        }
        Ok(())
    }

    fn finish(mut self) -> Result<(Vec<ContentBlock>, StopReason), AgentError> {
        self.blocks
            .retain(|b| !matches!(b, ContentBlock::Text { text } if text.is_empty()));
        let stop = self
            .stop_reason
            .or(self.message_complete.then_some(StopReason::EndTurn))
            .ok_or(AgentError::StreamParse(
                "stream ended without stop_reason".into(),
            ))?;
        Ok((self.blocks, stop))
    }
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
        let mut buf = String::new();
        let mut parser = SseParser::default();

        while let Some(chunk) = stream.next().await {
            buf.push_str(&String::from_utf8_lossy(&chunk?));
            while let Some(nl) = buf.find('\n') {
                let line = buf[..nl].trim_end().to_string();
                buf.drain(..nl + 1);
                parser.process_line(&line)?;
            }
        }
        parser.finish()
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
        let mut blocks = vec![
            ContentBlock::Text {
                text: String::new(),
            },
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
            },
        ];
        blocks.retain(|b| !matches!(b, ContentBlock::Text { text } if text.is_empty()));
        assert_eq!(blocks.len(), 2);
        assert!(matches!(&blocks[0], ContentBlock::Text { text } if text == "real content"));
        assert!(matches!(&blocks[1], ContentBlock::ToolUse { .. }));
    }

    // --- SSE parser tests ---

    /// Helper: feed lines into an SseParser and return the result.
    fn parse_sse(lines: &[&str]) -> Result<(Vec<ContentBlock>, StopReason), AgentError> {
        let mut parser = SseParser::default();
        for line in lines {
            parser.process_line(line)?;
        }
        parser.finish()
    }

    #[test]
    fn sse_text_response() {
        let (blocks, stop) = parse_sse(&[
            r#"event: content_block_start"#,
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            r#"event: content_block_delta"#,
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
            r#"event: content_block_delta"#,
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" world"}}"#,
            r#"event: content_block_stop"#,
            r#"data: {"type":"content_block_stop","index":0}"#,
            r#"event: message_delta"#,
            r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#,
            r#"event: message_stop"#,
            r#"data: {"type":"message_stop"}"#,
        ])
        .unwrap();
        assert_eq!(stop, StopReason::EndTurn);
        assert_eq!(blocks.len(), 1);
        assert!(matches!(&blocks[0], ContentBlock::Text { text } if text == "Hello world"));
    }

    #[test]
    fn sse_tool_use_response() {
        let (blocks, stop) = parse_sse(&[
            r#"event: content_block_start"#,
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t1","name":"bash"}}"#,
            r#"event: content_block_delta"#,
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"comm"}}"#,
            r#"event: content_block_delta"#,
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"and\":\"ls\"}"}}"#,
            r#"event: content_block_stop"#,
            r#"data: {"type":"content_block_stop","index":0}"#,
            r#"event: message_delta"#,
            r#"data: {"type":"message_delta","delta":{"stop_reason":"tool_use"}}"#,
            r#"event: message_stop"#,
            r#"data: {"type":"message_stop"}"#,
        ])
        .unwrap();
        assert_eq!(stop, StopReason::ToolUse);
        assert_eq!(blocks.len(), 1);
        if let ContentBlock::ToolUse { id, name, input } = &blocks[0] {
            assert_eq!(id, "t1");
            assert_eq!(name, "bash");
            assert_eq!(input["command"], "ls");
        } else {
            panic!("expected ToolUse");
        }
    }

    #[test]
    fn sse_mixed_text_and_tool() {
        let (blocks, stop) = parse_sse(&[
            r#"event: content_block_start"#,
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            r#"event: content_block_delta"#,
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Let me check"}}"#,
            r#"event: content_block_stop"#,
            r#"data: {"type":"content_block_stop","index":0}"#,
            r#"event: content_block_start"#,
            r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"t1","name":"read_file"}}"#,
            r#"event: content_block_delta"#,
            r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"src/main.rs\"}"}}"#,
            r#"event: content_block_stop"#,
            r#"data: {"type":"content_block_stop","index":1}"#,
            r#"event: message_delta"#,
            r#"data: {"type":"message_delta","delta":{"stop_reason":"tool_use"}}"#,
        ])
        .unwrap();
        assert_eq!(stop, StopReason::ToolUse);
        assert_eq!(blocks.len(), 2);
        assert!(matches!(&blocks[0], ContentBlock::Text { text } if text == "Let me check"));
        assert!(matches!(&blocks[1], ContentBlock::ToolUse { name, .. } if name == "read_file"));
    }

    #[test]
    fn sse_unknown_block_type_filtered() {
        // Unknown block types (like thinking) should produce placeholder blocks
        // that are filtered out in finish()
        let (blocks, stop) = parse_sse(&[
            r#"event: content_block_start"#,
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
            r#"event: content_block_stop"#,
            r#"data: {"type":"content_block_stop","index":0}"#,
            r#"event: content_block_start"#,
            r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}"#,
            r#"event: content_block_delta"#,
            r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"visible"}}"#,
            r#"event: content_block_stop"#,
            r#"data: {"type":"content_block_stop","index":1}"#,
            r#"event: message_delta"#,
            r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#,
        ])
        .unwrap();
        assert_eq!(stop, StopReason::EndTurn);
        assert_eq!(blocks.len(), 1);
        assert!(matches!(&blocks[0], ContentBlock::Text { text } if text == "visible"));
    }

    #[test]
    fn sse_missing_index_skipped() {
        // Delta events without an index field should be silently skipped
        let (blocks, _) = parse_sse(&[
            r#"event: content_block_start"#,
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            r#"event: content_block_delta"#,
            r#"data: {"type":"content_block_delta","delta":{"type":"text_delta","text":"no index"}}"#,
            r#"event: content_block_delta"#,
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"has index"}}"#,
            r#"event: content_block_stop"#,
            r#"data: {"type":"content_block_stop","index":0}"#,
            r#"event: message_delta"#,
            r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#,
        ])
        .unwrap();
        assert_eq!(blocks.len(), 1);
        assert!(matches!(&blocks[0], ContentBlock::Text { text } if text == "has index"));
    }

    #[test]
    fn sse_out_of_bounds_index_safe() {
        // Index beyond blocks array should not panic
        let (blocks, _) = parse_sse(&[
            r#"event: content_block_start"#,
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            r#"event: content_block_delta"#,
            r#"data: {"type":"content_block_delta","index":99,"delta":{"type":"text_delta","text":"orphan"}}"#,
            r#"event: content_block_delta"#,
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"ok"}}"#,
            r#"event: content_block_stop"#,
            r#"data: {"type":"content_block_stop","index":0}"#,
            r#"event: message_delta"#,
            r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#,
        ])
        .unwrap();
        assert_eq!(blocks.len(), 1);
        assert!(matches!(&blocks[0], ContentBlock::Text { text } if text == "ok"));
    }

    #[test]
    fn sse_stream_error_event() {
        let err = parse_sse(&[
            r#"event: content_block_start"#,
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            r#"event: error"#,
            r#"data: {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
        ]);
        assert!(err.is_err());
        let msg = err.unwrap_err().to_string();
        assert!(msg.contains("Overloaded"), "error message: {msg}");
    }

    #[test]
    fn sse_incomplete_stream_detected() {
        // Stream ends without message_delta or message_stop — connection dropped
        let err = parse_sse(&[
            r#"event: content_block_start"#,
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            r#"event: content_block_delta"#,
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"partial"}}"#,
        ]);
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("stop_reason"));
    }

    #[test]
    fn sse_message_stop_fallback() {
        // message_stop without message_delta should default to EndTurn
        let (_, stop) = parse_sse(&[
            r#"event: content_block_start"#,
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            r#"event: content_block_delta"#,
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}"#,
            r#"event: content_block_stop"#,
            r#"data: {"type":"content_block_stop","index":0}"#,
            r#"event: message_stop"#,
            r#"data: {"type":"message_stop"}"#,
        ])
        .unwrap();
        assert_eq!(stop, StopReason::EndTurn);
    }

    #[test]
    fn sse_max_tokens_stop() {
        let (_, stop) = parse_sse(&[
            r#"event: content_block_start"#,
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            r#"event: content_block_delta"#,
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"truncated"}}"#,
            r#"event: message_delta"#,
            r#"data: {"type":"message_delta","delta":{"stop_reason":"max_tokens"}}"#,
        ])
        .unwrap();
        assert_eq!(stop, StopReason::MaxTokens);
    }

    #[test]
    fn sse_corrupt_tool_json_produces_null_input() {
        let (blocks, _) = parse_sse(&[
            r#"event: content_block_start"#,
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t1","name":"bash"}}"#,
            r#"event: content_block_delta"#,
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"broken"}}"#,
            r#"event: content_block_stop"#,
            r#"data: {"type":"content_block_stop","index":0}"#,
            r#"event: message_delta"#,
            r#"data: {"type":"message_delta","delta":{"stop_reason":"tool_use"}}"#,
        ])
        .unwrap();
        assert_eq!(blocks.len(), 1);
        if let ContentBlock::ToolUse { input, .. } = &blocks[0] {
            assert!(input.is_null(), "corrupt JSON should produce null input");
        } else {
            panic!("expected ToolUse");
        }
    }

    #[test]
    fn sse_empty_lines_ignored() {
        let mut parser = SseParser::default();
        parser.process_line("").unwrap();
        parser.process_line("").unwrap();
        // Feeding a complete response after empty lines
        parser.process_line("event: message_delta").unwrap();
        parser
            .process_line(r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#)
            .unwrap();
        parser.process_line("event: message_stop").unwrap();
        parser
            .process_line(r#"data: {"type":"message_stop"}"#)
            .unwrap();
        let (blocks, stop) = parser.finish().unwrap();
        assert_eq!(stop, StopReason::EndTurn);
        assert!(blocks.is_empty());
    }

    #[test]
    fn sse_non_sse_lines_ignored() {
        let mut parser = SseParser::default();
        parser.process_line(":comment").unwrap();
        parser.process_line("random garbage").unwrap();
        parser.process_line("event: message_delta").unwrap();
        parser
            .process_line(r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#)
            .unwrap();
        let (_, stop) = parser.finish().unwrap();
        assert_eq!(stop, StopReason::EndTurn);
    }
}
