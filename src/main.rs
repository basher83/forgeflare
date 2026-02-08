mod api;
mod tools;

use api::{AnthropicClient, ContentBlock, Message, Role};
use clap::Parser;
use std::io::Write;
use tools::{all_tools, dispatch_tool, tools_as_schemas};

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
    let client = match AnthropicClient::new() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };
    let tools = all_tools();
    let schemas = tools_as_schemas(&tools);
    println!("Chat with Claude (type 'exit' or Ctrl-D to quit)");
    let mut conversation: Vec<Message> = Vec::new();
    let stdin = std::io::stdin();
    loop {
        print!("\x1b[94mYou\x1b[0m: ");
        std::io::stdout().flush().ok();
        let mut input = String::new();
        match stdin.read_line(&mut input) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => {
                eprintln!("Input error: {e}");
                break;
            }
        }
        let input = input.trim();
        if input.is_empty() {
            continue;
        }
        if input == "exit" {
            break;
        }
        conversation.push(Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: input.to_string(),
            }],
        });
        let response = match client
            .send_message(&conversation, &schemas, &cli.model)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                eprintln!("\x1b[91mError\x1b[0m: {e}");
                continue;
            }
        };
        conversation.push(Message {
            role: Role::Assistant,
            content: response.content.clone(),
        });
        let mut current_response = response;
        loop {
            let mut tool_results: Vec<ContentBlock> = Vec::new();
            for block in &current_response.content {
                if let ContentBlock::ToolUse { id, name, input } = block {
                    if cli.verbose {
                        eprintln!("\x1b[96mtool\x1b[0m: {name}({input})");
                    } else {
                        eprintln!("\x1b[96mtool\x1b[0m: {name}");
                    }
                    let result = dispatch_tool(&tools, name, input.clone(), id);
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
            conversation.push(Message {
                role: Role::User,
                content: tool_results,
            });
            current_response = match client
                .send_message(&conversation, &schemas, &cli.model)
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("\x1b[91mError\x1b[0m: {e}");
                    break;
                }
            };
            conversation.push(Message {
                role: Role::Assistant,
                content: current_response.content.clone(),
            });
        }
    }
    // TODO: subagent dispatch integration point (spec R8)
}
