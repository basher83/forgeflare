mod api;
mod tools;

use api::{AnthropicClient, ContentBlock, Message, Role, StopReason, color};
use clap::Parser;
use std::io::{IsTerminal, Write};
use tools::{all_tool_schemas, dispatch_tool};

fn build_system_prompt() -> String {
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| ".".into());
    format!(
        "You are a coding agent. Environment: {cwd} on {os}/{arch}\n\
         \n\
         # Tools\n\
         \n\
         read_file(path): Returns file contents with line numbers. 1MB limit. Detects binary files.\n\
         - Use BEFORE editing any file. Never edit blind.\n\
         - Prefer over bash cat/head — gives line numbers for precise edits.\n\
         \n\
         list_files(path?, recursive?): Lists files/dirs. Default: non-recursive. 1000 entry cap.\n\
         - Skips: .git, node_modules, target, .venv, vendor, .devenv\n\
         - Use to orient in unfamiliar directories before diving into files.\n\
         \n\
         bash(command, cwd?): Executes shell command. 120s timeout, 100KB output cap.\n\
         - Non-zero exit = is_error. Use for builds, tests, git, installs.\n\
         - Working directory resets each call — use cwd param or absolute paths.\n\
         - Never run destructive ops (rm -rf, force push, reset --hard) without user approval.\n\
         \n\
         edit_file(path, old_str, new_str): Surgical text replacement.\n\
         - old_str must match EXACTLY once (whitespace, indentation, everything).\n\
         - old_str != new_str (no-op rejected).\n\
         - Empty old_str + existing file = append. Empty old_str + missing file = create (with mkdir).\n\
         - On 'not found': re-read the file — likely whitespace/indentation mismatch.\n\
         - On 'found N times': include more surrounding context to make old_str unique.\n\
         - Always verify: read_file after editing to confirm the change.\n\
         \n\
         code_search(pattern, path?, file_type?, case_sensitive?): Wraps ripgrep.\n\
         - Regex patterns, case-insensitive by default. file_type: \"rust\", \"js\", \"py\", etc.\n\
         - 50 match limit. Prefer over bash grep/find for code search.\n\
         - Use to find definitions, call sites, patterns before making changes.\n\
         \n\
         # Workflow\n\
         \n\
         1. Understand the request — ask for clarification if ambiguous.\n\
         2. Explore first: code_search/read_file to understand existing code before changes.\n\
         3. Plan your approach, then execute. For multi-file changes, work in dependency order.\n\
         4. Verify every edit by reading the file back.\n\
         5. Run tests/build after changes to confirm nothing is broken.\n\
         \n\
         # Rules\n\
         \n\
         - Minimal, focused changes. No unrelated refactoring or cleanups.\n\
         - On failure, analyze the error. Retrying the same action without changes is wasteful.\n\
         - Be concise in explanations. Show, don't tell.",
        os = std::env::consts::OS,
        arch = std::env::consts::ARCH,
    )
}

const MAX_CONVERSATION_BYTES: usize = 720_000; // ~180K tokens at ~4 chars/token
const MAX_TOOL_ITERATIONS: usize = 50; // Safety limit for tool dispatch loop

/// Pop trailing User message on API error; if it was tool_results, also pop orphaned tool_use.
fn recover_conversation(conversation: &mut Vec<Message>) {
    let was_tool_results = conversation
        .pop_if(|m| matches!(m.role, Role::User))
        .is_some_and(|m| matches!(m.content.first(), Some(ContentBlock::ToolResult { .. })));
    if was_tool_results {
        conversation.pop_if(|m| matches!(m.role, Role::Assistant));
    }
}

/// Trim conversation at exchange boundaries, preserving tool_use/tool_result pairs.
fn trim_conversation(conversation: &mut Vec<Message>, max_bytes: usize) {
    let sizes: Vec<usize> = conversation
        .iter()
        .map(|m| serde_json::to_string(m).map_or(0, |s| s.len()))
        .collect();
    let total: usize = sizes.iter().sum();
    if total <= max_bytes {
        return;
    }
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
        truncate_oversized_blocks(conversation, max_bytes);
        return;
    }
    let (c, r) = (color("\x1b[93m"), color("\x1b[0m"));
    for &cut in &boundaries[1..=keep_last] {
        let prefix: usize = sizes[..cut].iter().sum();
        if total - prefix <= max_bytes {
            eprintln!("{c}[context]{r} Trimmed {cut} messages ({prefix} bytes) to fit context");
            conversation.drain(..cut);
            return;
        }
    }
    let dropped = boundaries[keep_last];
    eprintln!("{c}[context]{r} Trimmed to last exchange ({dropped} messages dropped)");
    conversation.drain(..dropped);
    truncate_oversized_blocks(conversation, max_bytes);
}

fn truncate_oversized_blocks(conversation: &mut [Message], max_bytes: usize) {
    let total: usize = conversation
        .iter()
        .map(|m| serde_json::to_string(m).map_or(0, |s| s.len()))
        .sum();
    if total <= max_bytes {
        return;
    }
    let mut remaining = total - max_bytes;
    for block in conversation.iter_mut().flat_map(|m| &mut m.content) {
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
            let end = text.floor_char_boundary(keep);
            remaining = remaining.saturating_sub(text.len() - end);
            text.truncate(end);
            text.push_str("\n... (truncated to fit context window)");
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
    #[arg(long, default_value = "16384")]
    max_tokens: u32,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let client = AnthropicClient::new().unwrap_or_else(|e| {
        eprintln!("Error: {e}");
        std::process::exit(1);
    });
    let schemas = all_tool_schemas();
    let system_prompt = build_system_prompt();
    if cli.verbose {
        eprintln!("[verbose] Initialized {} tools", schemas.len());
    }
    let interactive = std::io::stdin().is_terminal();
    if interactive {
        println!("Chat with Claude (type 'exit' or Ctrl-D to quit)");
    }
    let mut conversation: Vec<Message> = Vec::new();
    let mut piped_input = if !interactive {
        let mut buf = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf).ok();
        Some(buf.trim().to_string()).filter(|s| !s.is_empty())
    } else {
        None
    };
    loop {
        let input = match piped_input.take() {
            Some(p) => p,
            None if !interactive => break,
            None => {
                let (c, r) = (color("\x1b[94m"), color("\x1b[0m"));
                print!("{c}You{r}: ");
                std::io::stdout().flush().ok();
                let mut line = String::new();
                if std::io::stdin()
                    .read_line(&mut line)
                    .map_or(true, |n| n == 0)
                {
                    break;
                }
                let t = line.trim().to_string();
                match t.as_str() {
                    "" => continue,
                    "exit" => break,
                    _ => t,
                }
            }
        };
        if cli.verbose {
            eprintln!("[verbose] User: {input}");
        }
        conversation.push(Message {
            role: Role::User,
            content: vec![ContentBlock::Text { text: input }],
        });
        let mut tool_iterations = 0usize;
        loop {
            if tool_iterations >= MAX_TOOL_ITERATIONS {
                let (c, r) = (color("\x1b[93m"), color("\x1b[0m"));
                eprintln!(
                    "{c}[warning]{r} Tool loop hit {MAX_TOOL_ITERATIONS} iterations, breaking"
                );
                recover_conversation(&mut conversation);
                break;
            }
            if cli.verbose {
                let n = conversation.len();
                eprintln!("[verbose] Sending message, conversation len: {n}");
            }
            trim_conversation(&mut conversation, MAX_CONVERSATION_BYTES);
            let (response, stop_reason) = match client
                .send_message(
                    &conversation,
                    &schemas,
                    &cli.model,
                    &system_prompt,
                    cli.max_tokens,
                )
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    let (c, r) = (color("\x1b[91m"), color("\x1b[0m"));
                    eprintln!("{c}Error{r}: {e}");
                    recover_conversation(&mut conversation);
                    break;
                }
            };
            if cli.verbose {
                let n = response.len();
                eprintln!("[verbose] Received {n} blocks, stop: {stop_reason:?}");
            }
            conversation.push(Message {
                role: Role::Assistant,
                content: response,
            });
            if stop_reason != StopReason::ToolUse {
                if stop_reason == StopReason::MaxTokens {
                    let (c, r) = (color("\x1b[93m"), color("\x1b[0m"));
                    eprintln!("{c}[warning]{r} Response truncated (max_tokens reached)");
                    if let Some(msg) = conversation.last_mut() {
                        msg.content.retain(|b| {
                            !matches!(b, ContentBlock::ToolUse { input, .. } if input.is_null())
                        });
                    }
                }
                break;
            }
            let mut tool_results: Vec<ContentBlock> = Vec::new();
            for block in &conversation.last().unwrap().content {
                if let ContentBlock::ToolUse { id, name, input } = block {
                    if input.is_null() {
                        let (c, r) = (color("\x1b[93m"), color("\x1b[0m"));
                        eprintln!("{c}[warning]{r} Tool {name}: corrupt input (null)");
                        tool_results.push(ContentBlock::ToolResult {
                            tool_use_id: id.clone(),
                            content: "tool input was corrupt (JSON parse failed)".into(),
                            is_error: Some(true),
                        });
                        continue;
                    }
                    let (c, r) = (color("\x1b[96m"), color("\x1b[0m"));
                    if cli.verbose {
                        eprintln!("{c}tool{r}: {name}({input})");
                    } else {
                        eprintln!("{c}tool{r}: {name}");
                    }
                    let result = dispatch_tool(name, input.clone(), id);
                    if let ContentBlock::ToolResult {
                        ref content,
                        ref is_error,
                        ..
                    } = result
                    {
                        let (label, clr) = if is_error == &Some(true) {
                            ("error", color("\x1b[91m"))
                        } else {
                            ("result", color("\x1b[92m"))
                        };
                        let r = color("\x1b[0m");
                        if is_error == &Some(true) || cli.verbose {
                            let t: String = content.chars().take(200).collect();
                            eprintln!("{clr}{label}{r}: {t}");
                        } else {
                            eprintln!("{clr}{label}{r}: {} chars", content.len());
                        }
                    }
                    tool_results.push(result);
                }
            }
            if tool_results.is_empty() {
                break;
            }
            tool_iterations += 1;
            if cli.verbose {
                let n = tool_results.len();
                eprintln!("[verbose] Sending {n} tool results (iteration {tool_iterations})");
            }
            conversation.push(Message {
                role: Role::User,
                content: tool_results,
            });
        }
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
    fn truncate_oversized_multibyte_char_boundary() {
        // Regression: truncate_oversized_blocks used to panic when `keep` fell on the
        // second byte of a multi-byte UTF-8 character. 'é' is 2 bytes (0xC3 0xA9);
        // if the computed keep position lands on 0xA9, String::truncate panics.
        // The fix uses floor_char_boundary to snap to the nearest valid boundary.
        let text = "é".repeat(6_000); // 12KB of 2-byte chars → eligible (>10KB)
        let mut conv = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text { text }],
        }];
        // Budget tiny enough to force truncation, should not panic
        truncate_oversized_blocks(&mut conv, 100);
        if let ContentBlock::Text { text } = &conv[0].content[0] {
            assert!(text.contains("truncated"), "should have truncation marker");
            // Verify the truncated text is valid UTF-8 (no panic on iteration)
            assert!(text.chars().count() > 0);
        } else {
            panic!("expected Text block");
        }
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

    #[test]
    fn api_error_recovery_pops_dangling_user_text() {
        // send_message fails on first inner-loop iteration: pop the user's text message
        let mut conv = vec![
            user_text("first question"),
            assistant_text("first answer"),
            user_text("second question"),
        ];
        recover_conversation(&mut conv);
        assert_eq!(conv.len(), 2);
        assert!(matches!(conv.last().unwrap().role, Role::Assistant));
    }

    #[test]
    fn api_error_recovery_pops_tool_results_and_orphaned_tool_use() {
        // send_message fails mid-tool-loop: pop tool_results AND the orphaned tool_use.
        // Without this, the API rejects because tool_use has no matching tool_result.
        let mut conv = vec![
            user_text("do something"),
            assistant_tool_use(),
            user_tool_result("tool output"),
        ];
        recover_conversation(&mut conv);
        assert_eq!(conv.len(), 1);
        assert!(
            matches!(&conv[0].content[0], ContentBlock::Text { text } if text == "do something")
        );
    }

    #[test]
    fn api_error_recovery_noop_when_last_is_assistant() {
        let mut conv = vec![user_text("hello"), assistant_text("hi")];
        recover_conversation(&mut conv);
        assert_eq!(conv.len(), 2);
        assert!(matches!(conv.last().unwrap().role, Role::Assistant));
    }

    #[test]
    fn api_error_recovery_empty_conversation() {
        let mut conv: Vec<Message> = Vec::new();
        recover_conversation(&mut conv); // should not panic
        assert!(conv.is_empty());
    }

    #[test]
    fn trim_all_tool_result_exchanges_falls_through() {
        // When every User message starts with ToolResult (no text boundaries after index 0),
        // trim_conversation should fall through to truncate_oversized_blocks since there
        // are no safe exchange boundaries to cut at (except the very first message).
        let mut conv = vec![
            user_text("initial question"),
            assistant_tool_use(),
            user_tool_result("result 1"),
            assistant_tool_use(),
            user_tool_result("result 2"),
            assistant_text("done"),
        ];
        // Set budget too small to hold everything
        let budget = conversation_bytes(&conv[..2]); // only fits first exchange
        trim_conversation(&mut conv, budget);
        // The only boundary is at index 0; keep_last = 0 → falls through to truncation
        // Conversation should be preserved (single boundary can't trim)
        assert!(!conv.is_empty());
    }

    #[test]
    fn tool_iteration_limit_recovery_cleans_trailing_tool_results() {
        // When the tool loop hits MAX_TOOL_ITERATIONS, the last message is a User
        // message with tool_results (no matching Assistant reply). recover_conversation
        // must pop both the tool_results AND the orphaned tool_use to prevent
        // consecutive User messages that the API would reject with 400.
        let mut conv = vec![
            user_text("start task"),
            assistant_tool_use(),
            user_tool_result("iteration result"),
        ];
        // Simulates what happens at the iteration limit break
        recover_conversation(&mut conv);
        assert_eq!(
            conv.len(),
            1,
            "should pop tool_result and orphaned tool_use"
        );
        assert!(matches!(&conv[0].content[0], ContentBlock::Text { text } if text == "start task"));
    }

    #[test]
    fn tool_iteration_limit_recovery_with_assistant_text_only() {
        // If the loop breaks when the last message is an Assistant text (no pending
        // tool results), recover_conversation should be a no-op.
        let mut conv = vec![user_text("question"), assistant_text("answer")];
        recover_conversation(&mut conv);
        assert_eq!(conv.len(), 2, "should not modify clean conversation");
    }

    #[test]
    fn system_prompt_contains_environment_info() {
        let prompt = build_system_prompt();
        assert!(prompt.contains(std::env::consts::OS), "should contain OS");
        assert!(
            prompt.contains(std::env::consts::ARCH),
            "should contain arch"
        );
        assert!(prompt.contains("read_file"), "should list tools");
        assert!(
            prompt.contains("edit_file"),
            "should contain edit_file guidance"
        );
        assert!(
            prompt.contains("Never edit blind"),
            "should contain safety rules"
        );
    }
}
