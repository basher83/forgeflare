# Rust Coding Agent — Operations Guide

## Build & Run

```bash
# Run
ANTHROPIC_API_KEY=... cargo run -- [--verbose] [--model claude-opus-4-6]

# Test
cargo test

# Lint
cargo clippy -- -D warnings

# Format
cargo fmt --check

# Build
cargo build --release

# Full validation
cargo fmt --check && cargo clippy -- -D warnings && cargo test && cargo build
```

Binary: `agent`

## Project Structure

```typescript
src/
  main.rs         — CLI loop, user interface
  api.rs          — Anthropic client (reqwest + SSE)
  tools/
    mod.rs        — Tool registry (merged from registry.rs)
    read.rs       — read_file tool
    list.rs       — list_files tool
    bash.rs       — bash tool
    edit.rs       — edit_file tool
    search.rs     — ripgrep wrapper
```

## Dependencies

- reqwest 0.13
- thiserror 2
- futures-util 0.3
- wait-timeout 0.2
- serde, serde_json
- tokio
- clap
- anyhow

## Code Patterns

**Error Handling**
- Define error types in each module with `thiserror`
- Use `anyhow` for propagation in `main.rs`
- Tool errors returned as text in Anthropic API responses

**Async**
- `tokio` runtime with full features
- Async for HTTP (reqwest) and command execution (tokio::process)
- Main loop is synchronous; blocks on async operations

**CLI**
- `clap` with derive macros
- Flags: `--verbose`, `--model` (default: claude-opus-4-6)
- Interactive REPL or read from stdin

**HTTP Client**
- Roll own with `reqwest`
- POST to `https://api.anthropic.com/v1/messages`
- SSE decoding for streaming responses
- Parse `stop_reason` to detect tool_use vs end_turn

**JSON**
- `serde` + `serde_json` with derive macros
- Tool dispatch follows Anthropic tool_use specification
- Context accumulates in conversation array
