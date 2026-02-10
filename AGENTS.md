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

```
src/
  main.rs         — CLI loop, user interface, system prompt (build_system_prompt)
  api.rs          — Anthropic client (reqwest + SSE)
  tools/mod.rs    — 5 tools: read, list, bash (streaming), edit (replace_all), search
```

135 tests

## Dependencies

reqwest 0.13, thiserror 2, futures-util 0.3, serde/serde_json, tokio, clap
