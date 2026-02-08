mod api;
mod tools;

use api::{AnthropicClient, ContentBlock, Message, Role};
use clap::Parser;
use std::io::Write;
use tools::{all_tool_schemas, dispatch_tool};

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
    println!("Chat with Claude (type 'exit' or Ctrl-D to quit)");
    let mut conversation: Vec<Message> = Vec::new();
    let stdin = std::io::stdin();
    loop {
        print!("\x1b[94mYou\x1b[0m: ");
        std::io::stdout().flush().ok();
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
        // Inner loop: send message, dispatch tools, repeat until no tool_use
        loop {
            if cli.verbose {
                eprintln!(
                    "[verbose] Sending message, conversation length: {}",
                    conversation.len()
                );
            }
            let response = match client
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
                eprintln!("[verbose] Received {} content blocks", response.len());
            }
            conversation.push(Message {
                role: Role::Assistant,
                content: response.clone(),
            });
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
                        eprintln!(
                            "\x1b[92mresult\x1b[0m: {}",
                            &content[..content.len().min(200)]
                        );
                    }
                    tool_results.push(result);
                }
            }
            if tool_results.is_empty() {
                break;
            }
            if cli.verbose {
                eprintln!(
                    "[verbose] Sending {} tool results back to Claude",
                    tool_results.len()
                );
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
