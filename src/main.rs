mod api;
mod tools;

use api::{AnthropicClient, ContentBlock, Message, Role, StopReason};
use clap::Parser;
use std::io::{IsTerminal, Write};
use tools::{all_tool_schemas, dispatch_tool};

const MAX_CONVERSATION_BYTES: usize = 720_000; // ~180K tokens at ~4 chars/token

/// Trim conversation to stay within token budget. Removes oldest exchanges first,
/// preserving tool_use/tool_result pairing by only cutting at user-text boundaries.
fn trim_conversation(conversation: &mut Vec<Message>, max_bytes: usize) {
    let sizes: Vec<usize> = conversation
        .iter()
        .map(|m| serde_json::to_string(m).map_or(0, |s| s.len()))
        .collect();
    let total: usize = sizes.iter().sum();
    if total <= max_bytes {
        return;
    }
    // Exchange boundaries: user messages starting with Text (not ToolResult)
    let boundaries: Vec<usize> = conversation
        .iter()
        .enumerate()
        .filter(|(_, m)| {
            matches!(m.role, Role::User)
                && m.content
                    .first()
                    .is_some_and(|b| matches!(b, ContentBlock::Text { .. }))
        })
        .map(|(i, _)| i)
        .collect();
    let keep_last = boundaries.len().saturating_sub(1);
    if keep_last == 0 {
        // Single exchange over budget — truncate oversized content blocks
        truncate_oversized_blocks(conversation, max_bytes);
        return;
    }
    for &cut in &boundaries[1..=keep_last] {
        let prefix: usize = sizes[..cut].iter().sum();
        if total - prefix <= max_bytes {
            eprintln!(
                "\x1b[93m[context]\x1b[0m Trimmed {cut} messages ({prefix} bytes) to fit context window"
            );
            conversation.drain(..cut);
            return;
        }
    }
    // Last exchange still over budget — trim to it anyway to avoid unbounded growth
    eprintln!(
        "\x1b[93m[context]\x1b[0m Trimmed to last exchange ({} messages dropped)",
        boundaries[keep_last]
    );
    conversation.drain(..boundaries[keep_last]);
    // Safety valve: if a single exchange exceeds budget (e.g. 1MB read_file result),
    // truncate the largest text/tool_result content blocks to fit
    truncate_oversized_blocks(conversation, max_bytes);
}

/// Last-resort truncation of oversized content blocks when a single exchange exceeds budget.
fn truncate_oversized_blocks(conversation: &mut [Message], max_bytes: usize) {
    let total: usize = conversation
        .iter()
        .map(|m| serde_json::to_string(m).map_or(0, |s| s.len()))
        .sum();
    if total <= max_bytes {
        return;
    }
    let mut remaining = total - max_bytes;
    for msg in conversation.iter_mut() {
        for block in &mut msg.content {
            if remaining == 0 {
                return;
            }
            let text = match block {
                ContentBlock::ToolResult { content, .. } => content,
                ContentBlock::Text { text } => text,
                _ => continue,
            };
            if text.len() > 10_000 {
                let keep = text.len().saturating_sub(remaining).max(1_000);
                let end = (keep..text.len())
                    .find(|&i| text.is_char_boundary(i))
                    .unwrap_or(keep);
                remaining = remaining.saturating_sub(text.len() - end);
                text.truncate(end);
                text.push_str("\n... (truncated to fit context window)");
            }
        }
    }
}

#[derive(Parser)]
#[command(name = "agent", about = "Rust coding agent")]
struct Cli {
    #[arg(short, long)]
    verbose: bool,
    #[arg(short, long, default_value = "claude-opus-4-6")]
    model: String,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let client = AnthropicClient::new().unwrap_or_else(|e| {
        eprintln!("Error: {e}");
        std::process::exit(1);
    });
    let schemas = all_tool_schemas();
    if cli.verbose {
        eprintln!("[verbose] Initialized {} tools", schemas.len());
    }
    let interactive = std::io::stdin().is_terminal();
    if interactive {
        println!("Chat with Claude (type 'exit' or Ctrl-D to quit)");
    }
    let mut conversation: Vec<Message> = Vec::new();
    let stdin = std::io::stdin();
    loop {
        if interactive {
            print!("\x1b[94mYou\x1b[0m: ");
            std::io::stdout().flush().ok();
        }
        let mut input = String::new();
        if stdin.read_line(&mut input).map_or(true, |n| n == 0) {
            break;
        }
        let input = input.trim();
        if input.is_empty() {
            continue;
        }
        if input == "exit" {
            break;
        }
        if cli.verbose {
            eprintln!("[verbose] User: {input}");
        }
        conversation.push(Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: input.to_string(),
            }],
        });
        // Inner loop: send → dispatch tools → repeat (R8: subagent dispatch integration point)
        loop {
            if cli.verbose {
                eprintln!(
                    "[verbose] Sending message, conversation len: {}",
                    conversation.len()
                );
            }
            trim_conversation(&mut conversation, MAX_CONVERSATION_BYTES);
            let (response, stop_reason) = match client
                .send_message(&conversation, &schemas, &cli.model)
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("\x1b[91mError\x1b[0m: {e}");
                    break;
                }
            };
            if cli.verbose {
                eprintln!(
                    "[verbose] Received {} blocks, stop: {stop_reason:?}",
                    response.len()
                );
            }
            conversation.push(Message {
                role: Role::Assistant,
                content: response.clone(),
            });
            if stop_reason == StopReason::EndTurn {
                break;
            }
            if stop_reason == StopReason::MaxTokens {
                eprintln!("\x1b[93m[warning]\x1b[0m Response truncated (max_tokens reached)");
                // Remove partial ToolUse blocks with null input from conversation
                // to prevent corrupt tool_use blocks in future API calls
                if let Some(msg) = conversation.last_mut() {
                    msg.content.retain(
                        |b| !matches!(b, ContentBlock::ToolUse { input, .. } if input.is_null()),
                    );
                }
                break;
            }
            let mut tool_results: Vec<ContentBlock> = Vec::new();
            for block in &response {
                if let ContentBlock::ToolUse { id, name, input } = block {
                    eprintln!(
                        "\x1b[96mtool\x1b[0m: {name}{}",
                        if cli.verbose {
                            format!("({input})")
                        } else {
                            String::new()
                        }
                    );
                    let result = dispatch_tool(name, input.clone(), id);
                    if cli.verbose
                        && let ContentBlock::ToolResult { ref content, .. } = result
                    {
                        let truncated: String = content.chars().take(200).collect();
                        eprintln!("\x1b[92mresult\x1b[0m: {truncated}");
                    }
                    tool_results.push(result);
                }
            }
            if tool_results.is_empty() {
                break;
            }
            if cli.verbose {
                eprintln!("[verbose] Sending {} tool results", tool_results.len());
            }
            conversation.push(Message {
                role: Role::User,
                content: tool_results,
            });
        }
    }
    if cli.verbose {
        eprintln!("[verbose] Chat session ended");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn user_text(s: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: s.to_string(),
            }],
        }
    }

    fn assistant_text(s: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text {
                text: s.to_string(),
            }],
        }
    }

    fn assistant_tool_use() -> Message {
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "t1".into(),
                name: "bash".into(),
                input: serde_json::json!({"command": "ls"}),
            }],
        }
    }

    fn user_tool_result(content: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "t1".into(),
                content: content.to_string(),
                is_error: None,
            }],
        }
    }

    fn conversation_bytes(msgs: &[Message]) -> usize {
        msgs.iter()
            .map(|m| serde_json::to_string(m).unwrap().len())
            .sum()
    }

    #[test]
    fn trim_no_op_when_under_budget() {
        let mut conv = vec![user_text("hello"), assistant_text("hi")];
        let original_len = conv.len();
        trim_conversation(&mut conv, 100_000);
        assert_eq!(conv.len(), original_len);
    }

    #[test]
    fn trim_removes_oldest_exchange() {
        let mut conv = vec![
            user_text("first question"),
            assistant_text("first answer"),
            user_text("second question"),
            assistant_text("second answer"),
            user_text("third question"),
            assistant_text("third answer"),
        ];
        // Set budget to fit 2 exchanges but not 3
        let two_exchange_size = conversation_bytes(&conv[2..]);
        trim_conversation(&mut conv, two_exchange_size);
        assert_eq!(conv.len(), 4); // exchanges 2 and 3 remain
        assert!(
            matches!(&conv[0].content[0], ContentBlock::Text { text } if text == "second question")
        );
    }

    #[test]
    fn trim_preserves_tool_use_pairing() {
        // Exchange 1: user text -> assistant tool_use -> user tool_result -> assistant text
        // Exchange 2: user text -> assistant text
        let mut conv = vec![
            user_text("read the file"),
            assistant_tool_use(),
            user_tool_result("file contents here"),
            assistant_text("I see the file"),
            user_text("thanks"),
            assistant_text("you're welcome"),
        ];
        // Budget fits only exchange 2
        let last_exchange_size = conversation_bytes(&conv[4..]);
        trim_conversation(&mut conv, last_exchange_size);
        assert_eq!(conv.len(), 2);
        assert!(matches!(&conv[0].content[0], ContentBlock::Text { text } if text == "thanks"));
    }

    #[test]
    fn trim_never_splits_tool_exchange() {
        // Ensure tool_use and tool_result stay together
        let mut conv = vec![
            user_text("q1"),
            assistant_tool_use(),
            user_tool_result("result1"),
            assistant_text("a1"),
            user_text("q2"),
            assistant_tool_use(),
            user_tool_result("result2"),
            assistant_text("a2"),
        ];
        // Budget fits exchange 2 but not both
        let exchange2_size = conversation_bytes(&conv[4..]);
        trim_conversation(&mut conv, exchange2_size);
        // Should cut at index 4 (user text "q2"), keeping tool pair intact
        assert_eq!(conv.len(), 4);
        assert!(matches!(&conv[0].content[0], ContentBlock::Text { text } if text == "q2"));
        assert!(matches!(&conv[1].content[0], ContentBlock::ToolUse { .. }));
        assert!(matches!(
            &conv[2].content[0],
            ContentBlock::ToolResult { .. }
        ));
    }

    #[test]
    fn trim_single_exchange_untouched() {
        let mut conv = vec![
            user_text(&"x".repeat(10_000)),
            assistant_text(&"y".repeat(10_000)),
        ];
        // Budget smaller than the single exchange — can't trim further
        trim_conversation(&mut conv, 100);
        assert_eq!(conv.len(), 2); // preserved, nothing to cut
    }

    #[test]
    fn trim_large_tool_result_triggers_trim() {
        let big_result = "x".repeat(500_000);
        let mut conv = vec![
            user_text("old question"),
            assistant_text("old answer"),
            user_text("read big file"),
            assistant_tool_use(),
            user_tool_result(&big_result),
            assistant_text("that's a big file"),
            user_text("now what"),
            assistant_text("let me help"),
        ];
        // Budget that can't hold everything but can hold last 2 exchanges
        let last_two_size = conversation_bytes(&conv[2..]);
        let budget = last_two_size; // fits exchanges 2+3 but not 1+2+3
        trim_conversation(&mut conv, budget);
        assert!(conv.len() < 8); // something was trimmed
        // First remaining message should be a user text (exchange boundary)
        assert!(matches!(&conv[0].content[0], ContentBlock::Text { .. }));
    }

    #[test]
    fn trim_fallback_when_everything_huge() {
        let huge = "x".repeat(400_000);
        let mut conv = vec![
            user_text(&huge),
            assistant_text(&huge),
            user_text("small"),
            assistant_text("small"),
        ];
        // Budget too small for even the last exchange of the big ones
        trim_conversation(&mut conv, 1000);
        // Should trim to last exchange
        assert_eq!(conv.len(), 2);
        assert!(matches!(&conv[0].content[0], ContentBlock::Text { text } if text == "small"));
    }

    #[test]
    fn trim_empty_conversation() {
        let mut conv: Vec<Message> = Vec::new();
        trim_conversation(&mut conv, 100); // should not panic
        assert!(conv.is_empty());
    }

    #[test]
    fn truncate_oversized_single_exchange() {
        // When a single exchange has a huge tool result that exceeds the budget,
        // trim_conversation falls back to truncating content blocks
        let huge_result = "x".repeat(800_000); // 800KB > 720KB budget
        let mut conv = vec![
            user_text("read the big file"),
            assistant_tool_use(),
            user_tool_result(&huge_result),
            assistant_text("got it"),
        ];
        // Budget much smaller than the huge result
        let budget = 100_000;
        trim_conversation(&mut conv, budget);
        // Conversation should still have all 4 messages (single exchange can't be split)
        assert_eq!(conv.len(), 4);
        // But the huge tool result should be truncated
        if let ContentBlock::ToolResult { content, .. } = &conv[2].content[0] {
            assert!(
                content.len() < huge_result.len(),
                "content should be truncated"
            );
            assert!(
                content.contains("truncated to fit context window"),
                "should have truncation marker"
            );
        } else {
            panic!("expected ToolResult");
        }
    }

    #[test]
    fn truncate_oversized_skips_small_blocks() {
        // Only blocks >10KB should be eligible for truncation
        let mut conv = vec![
            user_text("small text that should not be truncated"),
            assistant_text(&"y".repeat(5_000)), // 5KB — below threshold
        ];
        // Even with tiny budget, small blocks should not be truncated
        truncate_oversized_blocks(&mut conv, 100);
        assert!(
            matches!(&conv[0].content[0], ContentBlock::Text { text } if text == "small text that should not be truncated")
        );
    }

    #[test]
    fn partial_tool_use_filtered_on_truncation() {
        // Simulates what happens when MaxTokens truncates mid-tool_use:
        // the ToolUse block has input: Value::Null because content_block_stop never fired
        let mut msg = Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Text {
                    text: "Let me check".into(),
                },
                ContentBlock::ToolUse {
                    id: "t1".into(),
                    name: "bash".into(),
                    input: serde_json::json!({"command": "ls"}),
                },
                ContentBlock::ToolUse {
                    id: "t2".into(),
                    name: "read_file".into(),
                    input: Value::Null, // partial — never completed
                },
            ],
        };
        // The same filter used in the MaxTokens handler
        msg.content
            .retain(|b| !matches!(b, ContentBlock::ToolUse { input, .. } if input.is_null()));
        assert_eq!(msg.content.len(), 2);
        assert!(matches!(&msg.content[0], ContentBlock::Text { .. }));
        assert!(matches!(&msg.content[1], ContentBlock::ToolUse { name, .. } if name == "bash"));
    }
}
