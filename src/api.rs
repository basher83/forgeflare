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
    #[error("IO: {0}")]
    Io(#[from] std::io::Error),
    #[error("ANTHROPIC_API_KEY not set")]
    MissingApiKey,
    #[error("stream: {0}")]
    StreamParse(String),
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

pub struct MessageResponse {
    pub content: Vec<ContentBlock>,
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
        tools: &[ToolSchema],
        model: &str,
    ) -> Result<MessageResponse, AgentError> {
        let body = serde_json::json!({
            "model": model, "max_tokens": 8096, "stream": true,
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
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AgentError::StreamParse(format!(
                "API returned {status}: {body}"
            )));
        }

        let mut stream = response.bytes_stream();
        let (mut buffer, mut current_event) = (String::new(), String::new());
        let mut content_blocks: Vec<ContentBlock> = Vec::new();
        let mut json_fragments: Vec<String> = Vec::new();

        while let Some(chunk) = stream.next().await {
            buffer.push_str(&String::from_utf8_lossy(&chunk?));
            while let Some(nl) = buffer.find('\n') {
                let line = buffer[..nl].trim_end().to_string();
                buffer = buffer[nl + 1..].to_string();
                if line.is_empty() {
                    continue;
                }
                if let Some(ev) = line.strip_prefix("event: ") {
                    current_event = ev.to_string();
                    continue;
                }
                let Some(data) = line.strip_prefix("data: ") else {
                    continue;
                };
                match current_event.as_str() {
                    "content_block_start" => {
                        let parsed: Value = serde_json::from_str(data)?;
                        let block = &parsed["content_block"];
                        match block["type"].as_str() {
                            Some("text") => content_blocks.push(ContentBlock::Text {
                                text: String::new(),
                            }),
                            Some("tool_use") => content_blocks.push(ContentBlock::ToolUse {
                                id: block["id"].as_str().unwrap_or_default().to_string(),
                                name: block["name"].as_str().unwrap_or_default().to_string(),
                                input: Value::Null,
                            }),
                            _ => {}
                        }
                        json_fragments.push(String::new());
                    }
                    "content_block_delta" => {
                        let parsed: Value = serde_json::from_str(data)?;
                        let idx = parsed["index"].as_u64().unwrap_or(0) as usize;
                        let delta = &parsed["delta"];
                        match delta["type"].as_str() {
                            Some("text_delta") => {
                                let t = delta["text"].as_str().unwrap_or_default();
                                print!("{t}");
                                std::io::stdout().flush().ok();
                                if let Some(ContentBlock::Text { text }) =
                                    content_blocks.get_mut(idx)
                                {
                                    text.push_str(t);
                                }
                            }
                            Some("input_json_delta") => {
                                if let Some(frag) = json_fragments.get_mut(idx) {
                                    frag.push_str(
                                        delta["partial_json"].as_str().unwrap_or_default(),
                                    );
                                }
                            }
                            _ => {}
                        }
                    }
                    "content_block_stop" => {
                        let parsed: Value = serde_json::from_str(data)?;
                        let idx = parsed["index"].as_u64().unwrap_or(0) as usize;
                        if let Some(ContentBlock::ToolUse { input, .. }) =
                            content_blocks.get_mut(idx)
                            && let Some(frag) = json_fragments.get(idx)
                            && !frag.is_empty()
                        {
                            *input = serde_json::from_str(frag).unwrap_or(Value::Null);
                        }
                        if matches!(content_blocks.get(idx), Some(ContentBlock::Text { .. })) {
                            println!();
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(MessageResponse {
            content: content_blocks,
        })
    }
}
