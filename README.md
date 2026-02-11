# Forgeflare

> [!CAUTION]
> **Forgeflare is a research project. If your name is not basher83 then do not use.**
>
> This software is experimental, unstable, and under active development. APIs will change without notice. Features may be incomplete or broken. There is no support, no documentation guarantees, and no warranty of any kind. Use at your own risk.

A Rust coding agent that uses Claude to edit code, run commands, and search repositories through a stateful conversation loop. Single binary, ~900 lines of production code, 153 tests.

## Tools

The agent exposes five tools to Claude:

- `read_file` -- file contents with line numbers (1 MB limit, binary detection)
- `list_files` -- directory listing with optional recursion (auto-skips .git, node_modules, target, etc.)
- `edit_file` -- surgical text replacement with exact-match default or `replace_all` for bulk changes, plus create/append
- `bash` -- shell command execution with real-time output streaming (120 s timeout, 100 KB output cap, blocked destructive patterns)
- `code_search` -- regex search via ripgrep (50-match limit, file type filtering)

## Install

Pre-built binaries for macOS (Apple Silicon) and Linux (x86_64) are available on the [releases page](https://github.com/basher83/forgeflare/releases/latest). Download the tarball for your platform, extract, and put `forgeflare` on your PATH:

```bash
tar xzf forgeflare-v*-<target>.tar.gz
sudo mv forgeflare /usr/local/bin/
xattr -d com.apple.quarantine /usr/local/bin/forgeflare
```

Or build from source:

```bash
cargo build --release
# Binary at target/release/forgeflare
```

## Quick Start

```bash
export ANTHROPIC_API_KEY=sk-...
forgeflare
```

Accepts interactive input or piped prompts (`echo "explain main.rs" | forgeflare`).

## Usage

```yaml
forgeflare [OPTIONS]

Options:
  --model <MODEL>          Claude model [default: claude-opus-4-6]
  --max-tokens <TOKENS>    Response token limit [default: 16384]
  --verbose                Show tool execution details
```

## How It Works

The agent runs a streaming conversation loop: user prompt goes to the Anthropic API, Claude responds (potentially requesting tool calls), the agent dispatches tools and feeds results back, repeating until Claude ends its turn. Conversation context is managed with sliding-window trimming (~180 K token budget) that preserves tool_use/tool_result pairs at exchange boundaries.

Safety guards block 37 destructive bash patterns (force push, rm -rf /, fork bombs, etc.), enforce file size limits, detect binary files, and cap tool iterations at 50 per turn.

## Project Structure

```text
src/
  main.rs    -- CLI, REPL, conversation loop, context management
  api.rs     -- Anthropic HTTP client, SSE streaming parser
  tools/     -- Tool definitions, dispatch, safety guards
```

## Requirements

- Rust 2024 edition
- `ANTHROPIC_API_KEY` environment variable
- `rg` (ripgrep) on PATH for `code_search`
