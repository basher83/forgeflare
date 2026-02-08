# Implementation Plan

## Current State

All phases complete. The codebase is fully implemented with comprehensive test coverage. SSE streaming works from day one. CLI supports `--verbose` and `--model` flags via clap derive. All 6 tools are operational: read_file, list_files, bash, edit_file, code_search, registry.

Build status: `cargo fmt --check` passes, `cargo clippy -- -D warnings` passes, `cargo build --release` passes, `cargo test` passes with 43 unit tests.

The implementation correctly follows the Go reference patterns from `edit_tool.go:79-214` for the event loop and tool dispatch. The two-level nested loop structure is preserved: outer loop for user turns, inner loop for tool-use continuation.

## Test Coverage

43 unit tests across all modules verify correctness:

- api.rs: 10 tests covering serialization/deserialization of ContentBlock, Message, Role, and ToolSchema types
- tools/mod.rs: 6 tests for tool registry, dispatch, and schema generation
- tools/read.rs: 4 tests for line numbering, missing paths, nonexistent files, and empty files
- tools/list.rs: 4 tests for directory listing, .git exclusion, directory suffix markers, and nonexistent directories
- tools/bash.rs: 5 tests for command execution, missing commands, failing commands, working directory persistence, and stderr capture
- tools/edit.rs: 8 tests for replace/create/append operations, error handling (not found, duplicate matches, same old/new strings), parent directory creation, and missing fields
- tools/search.rs: 6 tests for pattern matching, no matches, missing/empty patterns, case insensitivity, and file type filtering

## Architectural Decisions

Streaming from day one. The spec lists batch mode as a non-goal (spec line 122). The API client implements only `send_message` with SSE streaming.

The event loop mirrors the Go reference with one transport difference. The Go loop in `edit_tool.go:79-214` follows: outer loop reads user input, inner loop processes content blocks and dispatches tools, tool results are collected into a single user message, inner loop continues until no `tool_use` blocks appear. The Rust implementation follows the same structure. The difference: Go's `runInference` (line 223) returns a complete `Message` from a batch call; the Rust equivalent assembles the same `MessageResponse` from accumulated SSE events. The loop code above the transport layer is structurally identical.

Hand-written JSON schemas via `serde_json::json!()` with a `schema!()` macro to reduce boilerplate. The Go reference uses `jsonschema.Reflector` with generics to auto-derive schemas. That approach loses the `required` array (a confirmed bug in the Go code — `GenerateSchema` only extracts `Properties`, dropping `Required`). The Rust version uses explicit `json!()` literals with proper `required` arrays, which is more correct.

Manual SSE parsing. Anthropic's SSE format uses `event:` and `data:` line pairs separated by blank lines. The relevant events are `content_block_start`, `content_block_delta`, `content_block_stop`, `message_delta`, and `message_stop`. Parsing is ~30 lines, less than the weight of an SSE crate.

Tool errors become `tool_result` text, not Rust errors. Following Go at `edit_tool.go:149-183`, tool execution failures return as `tool_result` blocks with `is_error: true`. Only API/network/deserialization failures propagate through `Result`. The bash tool is a special case: command failures return `Ok(error_text)` (not `Err`), matching Go at `bash_tool.go:383-386`.

Multi-file layout per spec. `main.rs`, `api.rs`, and `tools/{mod,read,list,bash,edit,search}.rs`. The registry tool was merged into `tools/mod.rs` to save lines.

Bash timeout protection (spec R4). Commands are killed after 120 seconds using the `wait-timeout` crate. The `wait_timeout::ChildExt` trait extends `std::process::Child` with timeout capabilities since stable Rust doesn't provide this natively.

## Implementation Optimizations Applied

Schema macro. A `schema!()` macro was added to reduce JSON schema boilerplate by ~40 lines across all tool definitions.

Direct Value field access. Input structs were replaced with direct `Value` field access (`input["path"].as_str()`) to save ~30 lines.

Struct elimination. `MessageRequest` struct was inlined as `serde_json::json!()` to save ~10 lines. Dead fields (id, stop_reason, usage) were removed from `MessageResponse` to save ~5 lines.

Registry merge. `registry.rs` was merged into `tools/mod.rs` to save file overhead (~15 lines).

## Key Learnings

Rust 2024 edition forbids explicit `ref mut` in implicitly-borrowing patterns. The original Go pattern using mutable block references needed adaptation. The Rust equivalent uses indexed access with `if let Some(Value::Object(map)) = tool_inputs.get_mut(index)`.

rustfmt fights against compressed single-line code. The SSE parsing and tool dispatch logic expanded due to rustfmt's formatting rules.

The schema!() macro approach works well for reducing JSON schema boilerplate. It eliminated ~40 lines of repetitive `serde_json::json!()` structure.

`std::process::Child` doesn't have `wait_timeout` in stable Rust; the `wait-timeout` crate provides it via the `ChildExt` trait.

`str::lines()` does not produce a trailing empty string for content ending in `\n` — important for the read_file tool's line numbering behavior.

ripgrep searches include test source files themselves, so test patterns must be unique or use isolated temp directories to avoid false matches.

Fixed max_tokens from 8096 (typo) to 8192. Added system prompt that tells Claude it's a coding agent with available tools.

## Future Work

Subagent dispatch (spec R8). No implementation exists. The main loop has a comment placeholder: `// TODO: subagent dispatch integration point (spec R8)`.

Ralph-guard integration. Hook wiring exists per git commit b8974bd, but ForgeFlare operates independently. Future integration points:
- Activity logging for all tool executions
- Guard policy enforcement for file operations
- Audit trail for bash command execution

Performance profiling. No benchmarking has been done. Areas to measure:
- SSE parsing overhead vs dedicated crate
- Ripgrep subprocess spawn latency
- Conversation history memory growth over long sessions

Error recovery improvements. Current behavior:
- API errors print to stderr and continue REPL
- Tool errors return as `tool_result` blocks
- Network failures abort the current turn but keep the session alive
- No retry logic for transient failures

Configuration file support. All settings are CLI flags or environment variables. No `.forgeflare.toml` or equivalent.

## Verification Checklist

[x] All 6 tools implemented: read_file, list_files, bash, edit_file, code_search, registry
[x] SSE streaming functional with real-time output
[x] CLI parsing with --verbose and --model flags
[x] Event loop follows Go reference structure
[x] Tool dispatch with is_error propagation
[x] Bash timeout protection (120 seconds via wait-timeout crate)
[x] System prompt configured
[x] max_tokens set to 8192
[x] cargo fmt --check passes
[x] cargo clippy -- -D warnings passes
[x] cargo build --release passes
[x] cargo test passes (43 unit tests)
[x] Integration tested: chat, read (with line numbers), list (.git excluded, dirs suffixed /), bash (failures as text, timeout protection), edit (create/append/replace), search (rg wrapper, 50-line truncation)
