# Rust Coding Agent — Operations Guide

## Current Work

Building a unified Rust coding agent: single binary with streaming Anthropic API, 6 tools (read, list, bash, edit, search, registry), under 500 lines total.

## Build & Run

```bash
# Build
cargo build

# Test
cargo test

# Lint
cargo clippy -- -D warnings

# Format
cargo fmt --check

# Full validation
cargo fmt --check && cargo clippy -- -D warnings && cargo build && cargo test
```

## Project Structure

```
src/
  main.rs         — CLI loop, user interface
  api.rs          — Anthropic client (reqwest + SSE)
  tools/
    mod.rs        — Tool registry pattern
    read.rs       — read_file tool
    list.rs       — list_files tool
    bash.rs       — bash tool
    edit.rs       — edit_file tool
    search.rs     — ripgrep wrapper
    registry.rs   — tool introspection

reference/
  go-source/      — Go workshop code (pin)

specs/
  coding-agent.md — Unified agent specification
  README.md       — Spec index
```

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

## Reference Material

`reference/go-source/` contains the Go workshop (pin). Study `edit_tool.go` (lines 126-214) for the canonical event loop pattern. Same structure applies to Rust: API call → check response → dispatch tools → send results → repeat.
