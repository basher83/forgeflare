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

Binary: `forgeflare`

## Project Structure

```
src/
  main.rs         — CLI loop, user interface, system prompt (build_system_prompt)
  api.rs          — Anthropic client (reqwest + SSE), Usage struct
  session.rs      — Session transcript persistence (Entire-compatible JSONL)
  tools/mod.rs    — 5 tools: read, list, bash (streaming), edit (replace_all), search
.github/workflows/
  ci.yml          — CI pipeline: lint, audit, test, build (4 parallel jobs)
  release.yml     — Release builds: macOS aarch64 + Linux x86_64 tarballs (tag-triggered)
```

153 tests

## CI/CD

Workflow validation: review YAML structure against `specs/release-workflow.md`. No local runner — workflows are validated on push. Pinned action SHAs required (same convention as ci.yml).

## Dependencies

reqwest 0.13, thiserror 2, futures-util 0.3, serde/serde_json, tokio, clap, uuid 1, chrono 0.4
