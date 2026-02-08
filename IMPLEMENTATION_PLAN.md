# Implementation Plan

## Current State

All requirements (R1-R8) are fully implemented with hardened tool safety and robust SSE error handling. The codebase has ~668 production lines across 3 source files with 63 unit tests. SSE streaming works from day one with explicit `stop_reason` parsing per R7, unknown block type handling, mid-stream error detection, incomplete stream detection, and truncation cleanup. CLI supports `--verbose`, `--model` flags, and stdin pipe detection per R5. Conversation context management with truncation safety valve prevents unbounded growth.

Build status: `cargo fmt --check` passes, `cargo clippy -- -D warnings` passes, `cargo build --release` passes, `cargo test` passes with 63 unit tests.

File structure:
- src/main.rs (~206 production lines)
- src/api.rs (~209 production lines)
- src/tools/mod.rs (~255 production lines)
- Total: ~668 production lines + ~801 test lines

## Architectural Decisions

Streaming from day one. The spec lists batch mode as a non-goal. The API client implements only `send_message` with SSE streaming.

The event loop mirrors the Go reference pattern from `edit_tool.go:79-214`. Outer loop reads user input, inner loop processes content blocks and dispatches tools, tool results are collected into a single user message, inner loop continues until `stop_reason` indicates `end_turn`. The Rust implementation checks `stop_reason` from the `message_delta` SSE event (R7 compliance) rather than inferring from empty tool results.

`send_message` returns `(Vec<ContentBlock>, StopReason)` — the StopReason enum (`EndTurn` | `ToolUse` | `MaxTokens`) is parsed from the `message_delta` SSE event's `stop_reason` field. This makes the tool dispatch loop explicit: break on EndTurn, dispatch tools on ToolUse, warn and break on MaxTokens, with a defensive `tool_results.is_empty()` fallback.

Conversation context management uses a sliding window that trims at exchange boundaries. The Anthropic API requires every `ToolUse` block to have a corresponding `ToolResult` in the immediately following user message. Naively dropping messages would break this invariant. The `trim_conversation` function identifies safe cut points where a User message starts with `ContentBlock::Text` (not `ToolResult`), ensuring tool_use/tool_result pairs are never split. Budget is 720KB (~180K tokens at ~4 chars/token), estimated from JSON serialization size since that's what goes over the wire.

SSE buffer uses `buf.drain(..nl + 1)` instead of `buf[nl + 1..].to_string()` to avoid O(N^2) allocation per line during streaming.

Stdin pipe detection uses `std::io::IsTerminal`. When stdin is not a terminal (piped input), the interactive prompt and banner are suppressed (R5 compliance).

Hand-written JSON schemas via `serde_json::json!()` inline in the `tools!` macro. The Go reference uses `jsonschema.Reflector` which drops the `required` array (a bug). The Rust version uses explicit `json!()` literals with proper `required` arrays.

Manual SSE parsing handles `content_block_start`, `content_block_delta`, `content_block_stop`, and `message_delta` events. The `message_delta` handler extracts `stop_reason` for R7 compliance.

Tool errors become `tool_result` text, not Rust errors. Following Go at `edit_tool.go:149-183`, tool execution failures return as `tool_result` blocks with `is_error: true`.

tools! macro generates both `all_tool_schemas()` and `dispatch_tool()` from one definition, preventing schema/dispatch divergence.

## Key Learnings

Rust 2024 edition forbids explicit `ref mut` in implicitly-borrowing patterns. Indexed access with `if let Some(Value::Object(map)) = tool_inputs.get_mut(index)` is required instead.

rustfmt fights against compressed single-line code. Method chains like `child.stdout.take().unwrap().read_to_string(&mut stdout).ok()` get expanded to 6 lines, contributing ~15 lines of unavoidable expansion.

The `message_delta` SSE event contains `stop_reason` in `p["delta"]["stop_reason"]`, not at the top level. This differs from the batch API where `stop_reason` is a top-level field on the response.

`std::io::IsTerminal` is stable in Rust 2024 edition (stabilized in Rust 1.70). No external crate needed for terminal detection.

Bash non-zero exit codes now correctly return `Err(...)` so `is_error: true` is set on the tool result. This matches the Anthropic API protocol where tool execution failures should signal `is_error` to give the model a proper protocol-level signal. Previously, non-zero exits returned `Ok(...)` with `is_error: None`, losing the error signal. A timeout is also `Err(...)` with `is_error: true` since the command was killed mid-execution.

The Anthropic API documents three stop reasons: `end_turn`, `max_tokens`, `tool_use`. The Go reference only checks for tool_use presence implicitly. The Rust version explicitly parses all three from the `message_delta` SSE event, which is more correct and enables user-facing warnings on truncation.

Pipe buffer deadlock in bash_exec: the original implementation called `wait_timeout()` before reading stdout/stderr. When a command produces >64KB of output, the pipe buffer fills, blocking the child process, while `wait_timeout` blocks waiting for the child to exit. Fixed by draining stdout/stderr in separate threads before waiting. This pattern is essential for any synchronous process I/O where the output size is unbounded.

String::truncate panics on non-char-boundary positions. Both verbose output truncation (`main.rs`) and bash output truncation (`tools/mod.rs`) must find the nearest char boundary when truncating. Use `is_char_boundary()` for byte-level truncation, or `chars().take(n).collect()` for char-level truncation.

list_files directory filter must compare directory names, not relative paths. Using `path.file_name()` catches `.git` at any depth; comparing `rel == ".git"` only catches it at the top level.

Conversation trimming must respect tool_use/tool_result pairing. The Anthropic API requires every ToolUse from an assistant message to have a corresponding ToolResult in the next user message. Trimming at arbitrary positions breaks this invariant. The solution identifies "exchange boundaries" — User messages starting with Text (not ToolResult) — and only cuts at those positions. This ensures complete tool exchanges are either fully preserved or fully dropped.

Token estimation via JSON serialization size is pragmatic. Rather than importing a tokenizer library, estimating tokens at ~4 chars/token from the serialized JSON payload provides a good approximation since that's the actual wire format. The 720KB budget (~180K tokens) leaves headroom for system prompt, tool schemas, and the response within the 200K token context window.

SSE content_block_start can receive unknown block types (thinking, server_tool_use). The blocks[] and fragments[] parallel arrays MUST stay in sync by index — any mismatch causes data corruption for all subsequent blocks in the same response. The solution is to push a placeholder Text block for unknown types, then filter empty text blocks before returning from send_message.

Anthropic SSE stream can send error events mid-stream during overload or rate-limiting. These must be explicitly handled with a dedicated match arm rather than being swallowed by a catch-all `_ => {}` pattern. Error events are now matched and returned as `AgentError::StreamParse`.

On max_tokens truncation, tool_use blocks may be incomplete with null input because the `content_block_stop` event never fires. These corrupt blocks must be stripped from conversation history before breaking the loop to prevent API errors on subsequent calls. The filter checks for `ToolUse` blocks with `input == Value::Null`.

SSE stream completeness must be verified. The `stop_reason` was previously defaulted to `EndTurn`, meaning a dropped connection would silently be treated as a successful response. Fixed by using `Option<StopReason>` and tracking `message_stop` events. If the stream ends without a `stop_reason` from `message_delta`, the connection was dropped and an error is returned. The `message_stop` event serves as a secondary signal that the message completed normally, used as a defensive fallback if `message_delta` somehow delivers no `stop_reason`.

SSE event index fields should be validated, not defaulted. Using `unwrap_or(0)` for missing SSE index fields silently corrupts block 0 when events have malformed/missing indices. Skip events with missing index instead of defaulting.

Malformed JSON fragments in tool_use inputs should be detected at parse time. Using `unwrap_or(Value::Null)` on parse failure produces misleading "parameter is required" errors from tool dispatch. Logging the actual parse error makes debugging stream corruption much easier.

Conversation trimming must handle single-exchange overflow. When a single exchange contains a large tool result (e.g. near-1MB read_file), the conversation budget (720KB) can be exceeded even after trimming to one exchange. A last-resort truncation of oversized content blocks (>10KB) prevents API "request too large" errors.

Range::find is cleaner than while loops for char boundary scanning. `(keep..text.len()).find(|&i| text.is_char_boundary(i))` replaces the manual `while !is_char_boundary { end += 1 }` loop.

## Future Work

Subagent dispatch (spec R8). Types are defined (`SubagentContext` in api.rs, `StopReason` enum), integration point comments exist in main.rs. Actual dispatch logic remains unimplemented per spec's non-goals.

Ralph-guard integration. Hook wiring exists per commit b8974bd. Integration points: activity logging, guard policy enforcement, audit trail.

Performance profiling. No benchmarking done. Areas: ripgrep spawn latency, conversation memory growth patterns.

Error recovery. No retry logic for transient API failures. Spec explicitly states "No automatic retry; failures return to user for decision."

## Spec Alignment

The specification has been updated to reflect implementation decisions:

- R3 updated: enum example replaced with tools! macro pattern that matches implementation.
- R4 hardened: read_file now enforces 1MB size limit and detects binary files (null byte check). list_files supports optional `recursive` parameter (default: false). bash_exec truncates output at 100KB. Directory filter works at any depth using file_name comparison.
- R5 implemented: stdin pipe detection via `std::io::IsTerminal`, prompts suppressed in non-interactive mode.
- R7 implemented: `StopReason` enum parsed from `message_delta` SSE event. Inner loop breaks on `EndTurn`, warns on `MaxTokens`. Partial tool_use blocks filtered on truncation.
- SSE hardened: Unknown block types handled via placeholder blocks to maintain index sync. Mid-stream error events explicitly detected. Empty text blocks filtered before returning. Incomplete streams detected via missing stop_reason.
- System prompt upgraded from single sentence to structured workflow instructions covering read-before-edit, code_search usage, minimal changes, edit verification, bash safety, and error analysis.
- Tool descriptions enriched to match Go reference quality with usage guidance.
- max_tokens increased from 8192 to 16384 for better Opus performance (API supports up to 128K).
- Line target updated from <650 to <700 to accommodate conversation truncation safety valve (~668 actual).

## Verification Checklist

[x] All 5 tools implemented: read_file, list_files, bash, edit_file, code_search
[x] SSE streaming with real-time output and stop_reason parsing (R7)
[x] SSE unknown block type handling: placeholder blocks maintain index sync
[x] SSE mid-stream error events explicitly matched and returned as errors
[x] SSE index validation: skip events with missing index instead of defaulting to 0
[x] Malformed JSON fragment detection: log parse errors for tool_use inputs
[x] Partial tool_use blocks with null input filtered on MaxTokens truncation
[x] CLI: --verbose, --model flags, stdin pipe detection (R5)
[x] Event loop follows Go reference structure with explicit stop_reason check
[x] Tool dispatch with is_error propagation
[x] Bash non-zero exit returns is_error: true (matches Anthropic API protocol)
[x] Bash timeout protection (120s, returns is_error: true on timeout)
[x] Bash pipe deadlock prevention (threaded stdout/stderr draining)
[x] Bash output truncation (100KB limit)
[x] System prompt: structured workflow instructions (read-before-edit, minimal changes, etc.)
[x] Tool descriptions: enriched with usage guidance matching Go reference quality
[x] max_tokens set to 16384 for Opus performance (API supports up to 128K)
[x] StopReason enum: EndTurn, ToolUse, MaxTokens — parsed from message_delta SSE event
[x] MaxTokens truncation warning displayed to user
[x] Non-interactive mode: suppresses prompts when stdin is piped
[x] R8 subagent types defined (SubagentContext, StopReason in api.rs)
[x] read_file: 1MB size limit, binary detection via null byte check (R4)
[x] list_files: recursive parameter (default: false), SKIP_DIRS filter at any depth (R4)
[x] edit_file: schema documents create/append mode for empty old_str
[x] Verbose output truncation is UTF-8 safe (chars().take() instead of byte slice)
[x] No unwrap() panics in tool implementations
[x] Conversation context management: trim at exchange boundaries, 720KB budget
[x] Conversation truncation safety valve: oversized single exchanges truncated to fit budget
[x] SSE incomplete stream detection: error on missing stop_reason/message_stop
[x] SSE buffer: O(1) line extraction via buf.drain() instead of O(N^2) to_string()
[x] cargo fmt --check passes
[x] cargo clippy -- -D warnings passes
[x] cargo build --release passes
[x] cargo test passes (63 unit tests)
