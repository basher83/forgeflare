# Implementation Plan

## Current State

All requirements (R1-R8) are fully implemented with hardened tool safety and robust SSE error handling. The codebase has ~879 production lines across 3 source files with 117 unit tests.

Core features: SSE streaming with explicit `stop_reason` parsing, unknown block type handling, mid-stream error detection, incomplete stream detection. CLI supports `--verbose`, `--model`, `--max-tokens` flags and stdin pipe detection. Piped stdin reads all input as a single prompt. Conversation context management with truncation safety valve. API error recovery preserves conversation alternation invariant including orphaned tool_use cleanup. All terminal color output respects the NO_COLOR convention.

Tool safety: bash command guard blocks destructive patterns (rm -rf /, fork bombs, dd to block devices including /dev/sd, /dev/nvme, /dev/vd, /dev/hd, mkfs, chmod 777) with whitespace normalization and expanded flag ordering coverage. Redirect-to-device patterns cover all four device families. Timeout drain thread leak fixed. walk() has depth limit (MAX_WALK_DEPTH=20) and skips permission-denied entries. SSE parser validates tool_use blocks have non-empty id/name fields. search_exec applies 50-line cap before 100KB byte cap.

System prompt dynamically built at startup, injecting cwd and platform info with structured tool guidance. reqwest client has explicit timeouts (connect 30s, request 300s). Tool schema descriptions enriched with operational limits. Tool error display and result visibility in non-verbose mode.

Build status: `cargo fmt --check` passes, `cargo clippy -- -D warnings` passes, `cargo build --release` passes, `cargo test` passes with 117 unit tests.

File structure:
- src/main.rs (~321 production lines)
- src/api.rs (~254 production lines)
- src/tools/mod.rs (~304 production lines)
- Total: ~879 production lines

## Architectural Decisions

Streaming from day one. The spec lists batch mode as a non-goal. The API client implements only `send_message` with SSE streaming.

The event loop mirrors the Go reference pattern from `edit_tool.go:79-214`. Outer loop reads user input, inner loop processes content blocks and dispatches tools, tool results are collected into a single user message, inner loop continues until `stop_reason` indicates `end_turn`. The Rust implementation checks `stop_reason` from the `message_delta` SSE event (R7 compliance) rather than inferring from empty tool results.

`send_message` returns `(Vec<ContentBlock>, StopReason)` — the StopReason enum (`EndTurn` | `ToolUse` | `MaxTokens`) is parsed from the `message_delta` SSE event's `stop_reason` field. This makes the tool dispatch loop explicit: break on EndTurn, dispatch tools on ToolUse, warn and break on MaxTokens, with a defensive `tool_results.is_empty()` fallback.

Conversation context management uses a sliding window that trims at exchange boundaries. The Anthropic API requires every `ToolUse` block to have a corresponding `ToolResult` in the immediately following user message. Naively dropping messages would break this invariant. The `trim_conversation` function identifies safe cut points where a User message starts with `ContentBlock::Text` (not `ToolResult`), ensuring tool_use/tool_result pairs are never split. Budget is 720KB (~180K tokens at ~4 chars/token), estimated from JSON serialization size since that's what goes over the wire.

SSE buffer uses `buf.drain(..nl + 1)` instead of `buf[nl + 1..].to_string()` to avoid O(N^2) allocation per line during streaming.

Stdin pipe detection uses `std::io::IsTerminal`. When stdin is not a terminal (piped input), the interactive prompt and banner are suppressed (R5 compliance). Piped stdin is read entirely into a single string before entering the conversation loop, so multi-line prompts from `cat prompt.txt | agent` are sent as one coherent message instead of line-by-line.

Hand-written JSON schemas via `serde_json::json!()` inline in the `tools!` macro. The Go reference uses `jsonschema.Reflector` which drops the `required` array (a bug). The Rust version uses explicit `json!()` literals with proper `required` arrays.

Manual SSE parsing handles `content_block_start`, `content_block_delta`, `content_block_stop`, and `message_delta` events. The `message_delta` handler extracts `stop_reason` for R7 compliance.

Tool errors become `tool_result` text, not Rust errors. Following Go at `edit_tool.go:149-183`, tool execution failures return as `tool_result` blocks with `is_error: true`.

tools! macro generates both `all_tool_schemas()` and `dispatch_tool()` from one definition, preventing schema/dispatch divergence.

System prompt is built dynamically at startup in main.rs via `build_system_prompt()` and passed to `send_message` as a parameter. This injects the working directory and platform (os/arch) at runtime. The prompt includes structured tool behavioral guidance (rg semantics, edit_file exact-match rules, bash timeout/truncation limits) and safety rules (read-before-edit, no destructive ops without approval). Moving the prompt from a static string in api.rs to a parameter keeps the API module clean while enabling runtime context injection.

## Key Learnings

Rust 2024 edition forbids explicit `ref mut` in implicitly-borrowing patterns. Indexed access with `if let Some(Value::Object(map)) = tool_inputs.get_mut(index)` is required instead.

rustfmt fights against compressed single-line code. Method chains like `child.stdout.take().unwrap().read_to_string(&mut stdout).ok()` get expanded to 6 lines. rustfmt actively expands compressed patterns. Single-line `if x { continue; }` blocks and compressed multi-condition guards all get expanded. The only reliable way to reduce line count is through structural changes (combining match arms, extracting helpers, using combinators) — not cosmetic compression that rustfmt will undo.

The `message_delta` SSE event contains `stop_reason` in `p["delta"]["stop_reason"]`, not at the top level. This differs from the batch API where `stop_reason` is a top-level field on the response.

`std::io::IsTerminal` is stable in Rust 2024 edition (stabilized in Rust 1.70). No external crate needed for terminal detection.

Bash non-zero exit codes now correctly return `Err(...)` so `is_error: true` is set on the tool result. This matches the Anthropic API protocol where tool execution failures should signal `is_error` to give the model a proper protocol-level signal. A timeout is also `Err(...)` with `is_error: true` since the command was killed mid-execution.

The Anthropic API documents three stop reasons: `end_turn`, `max_tokens`, `tool_use`. The Go reference only checks for tool_use presence implicitly. The Rust version explicitly parses all three from the `message_delta` SSE event, which is more correct and enables user-facing warnings on truncation.

Pipe buffer deadlock in bash_exec: the original implementation called `wait_timeout()` before reading stdout/stderr. When a command produces >64KB of output, the pipe buffer fills, blocking the child process, while `wait_timeout` blocks waiting for the child to exit. Fixed by draining stdout/stderr in separate threads before waiting. This pattern is essential for any synchronous process I/O where the output size is unbounded.

list_files directory filter must compare directory names, not relative paths. Using `path.file_name()` catches `.git` at any depth; comparing `rel == ".git"` only catches it at the top level.

Conversation trimming must respect tool_use/tool_result pairing. The solution identifies "exchange boundaries" — User messages starting with Text (not ToolResult) — and only cuts at those positions. This ensures complete tool exchanges are either fully preserved or fully dropped.

Token estimation via JSON serialization size is pragmatic. Rather than importing a tokenizer library, estimating tokens at ~4 chars/token from the serialized JSON payload provides a good approximation since that's the actual wire format. The 720KB budget (~180K token) leaves headroom for system prompt, tool schemas, and the response within the 200K token context window.

SSE content_block_start can receive unknown block types (thinking, server_tool_use). The blocks[] and fragments[] parallel arrays MUST stay in sync by index — any mismatch causes data corruption for all subsequent blocks in the same response. The solution is to push a placeholder Text block for unknown types, then filter empty text blocks before returning from send_message.

Anthropic SSE stream can send error events mid-stream during overload or rate-limiting. These must be explicitly handled with a dedicated match arm rather than being swallowed by a catch-all `_ => {}` pattern.

On max_tokens truncation, tool_use blocks may be incomplete with null input because the `content_block_stop` event never fires. These corrupt blocks must be stripped from conversation history before breaking the loop to prevent API errors on subsequent calls.

SSE stream completeness must be verified. The `stop_reason` was previously defaulted to `EndTurn`, meaning a dropped connection would silently be treated as a successful response. Fixed by using `Option<StopReason>` and tracking `message_stop` events.

SSE event index fields should be validated, not defaulted. Using `unwrap_or(0)` for missing SSE index fields silently corrupts block 0 when events have malformed/missing indices. Skip events with missing index instead of defaulting.

Malformed JSON fragments in tool_use inputs should be detected at parse time. Using `unwrap_or(Value::Null)` on parse failure produces misleading "parameter is required" errors from tool dispatch. Logging the actual parse error makes debugging stream corruption much easier.

Conversation trimming must handle single-exchange overflow. When a single exchange contains a large tool result (e.g. near-1MB read_file), the conversation budget (720KB) can be exceeded even after trimming to one exchange. A last-resort truncation of oversized content blocks (>10KB) prevents API "request too large" errors.

API errors must not corrupt conversation alternation. The Anthropic API requires strict User/Assistant alternation. The user message is pushed before `send_message`, so when the API call fails, the conversation ends with a trailing User message. The fix: pop the trailing User message on API error. This handles both first-iteration failures (dangling user text) and mid-tool-loop failures (dangling tool results). Also pop orphaned Assistant messages when the popped User message contained tool_results.

SseParser extraction enables testability without HTTP mocks. By pulling the SSE event processing out of `send_message` into a struct with `process_line()` and `finish()` methods, the parser state machine becomes directly testable. This added ~15 net production lines but enabled 14 new SSE tests.

`list_files` output must be sorted for deterministic results. `fs::read_dir` returns entries in arbitrary filesystem order, unlike Go's `filepath.Walk` which returns lexical order. Adding `files.sort()` ensures consistent output across runs and platforms.

Tool loop iteration limit prevents runaway agent behavior. A safety limit of 50 iterations on the inner tool dispatch loop catches infinite loops where Claude keeps requesting tools. The limit is high enough for complex multi-step tasks but prevents unbounded API costs. Must call recover_conversation on limit break to maintain alternation.

Generic `drain<R: Read>` helper eliminates bash stdout/stderr thread spawn duplication. Both stdout and stderr need identical drain-in-thread logic but have different types (`ChildStdout` vs `ChildStderr`). A generic inner function handles both with a single implementation.

Piped stdin must be read as a single prompt, not line-by-line. The Go reference processes piped input line-by-line (each line becomes a separate API call), which silently produces wrong behavior when users pipe multi-line prompts. The fix reads all of stdin into one string before entering the conversation loop.

`Option::take()` with match arms eliminates boolean flags for one-shot patterns. Using `piped_input.take()` with `None if !interactive => break` is cleaner — the Option itself tracks consumption state.

SSE buffer residual data after stream end. The SSE parsing loop only processes lines terminated by `\n`. If the stream's final chunk doesn't end with a newline, the last event (often `message_delta` with `stop_reason`) is silently dropped. Fixed by processing the trailing buffer after the stream loop exits.

`reqwest::Client::new()` has no default timeout. Without explicit timeouts, a hung API connection blocks the agent forever. Adding `connect_timeout(30s)` and `timeout(300s)` via `ClientBuilder` prevents indefinite hangs.

`response.clone()` was unnecessary in the main loop. Moving the response into conversation first (no clone) and iterating `conversation.last().unwrap().content` eliminates the allocation.

`list_files` with recursive=true on large trees has no output cap. Added MAX_LIST_ENTRIES (1000) to prevent unbounded context consumption. Similarly, `search_exec` only had a 50-line cap but no byte-size limit — added MAX_BASH_OUTPUT (100KB) truncation for consistency.

API error mid-tool-loop leaves orphaned tool_use. Fixed by also popping the orphaned Assistant message when the popped User message contained tool_results. Uses `Vec::pop_if` (stable since Rust 1.86).

`LazyLock` provides zero-cost NO_COLOR support. A `static USE_COLOR: LazyLock<bool>` initialized from `std::env::var_os("NO_COLOR").is_none()` is checked once on first access and cached forever.

Corrupt tool_use blocks must receive error tool_results, not be skipped. The Anthropic API requires every tool_use to have a matching tool_result. The MaxTokens path correctly strips partial tool_use from conversation, but the ToolUse dispatch path must send error results for blocks with Value::Null input.

edit_file needs the same 1MB size guard as read_file. Without it, the agent could attempt to read/modify files that are too large, causing unbounded memory usage during string operations.

System prompt is the highest-leverage improvement for a coding agent. Infrastructure (SSE parsing, conversation management, tool safety) is necessary but not sufficient — the model's behavior is shaped by the system prompt. A compact, structured prompt with tool-specific guidance produces better results than verbose prose.

Dynamic system prompt injection (cwd, platform) eliminates tool call waste. Without environment context, the model spends early turns discovering basic facts like the current directory and OS.

`SubagentContext` was dead code — removed per engineering philosophy (no hypothetical future requirements). R8 is explicitly a non-goal in the spec.

`send_message` system_prompt parameter is cleaner than embedding the prompt in api.rs. The API module should not know about runtime context (cwd, platform). Passing the prompt as a parameter follows information hiding.

System prompt structure matters for agent effectiveness. Tool-per-section layout with explicit when-to-use guidance, error recovery hints, and anti-patterns gives the model actionable decision-making context at every step.

bash cwd parameter doesn't persist between calls — each invocation is a fresh shell. Documenting this in the system prompt prevents a common misconception where the model chains cd + command across separate bash calls expecting state to persist.

bash stdout/stderr concatenation needs a separator. A conditional labeled separator ('--- stderr ---') prevents invisible boundaries between streams while preserving compact output when only one stream is active.

SSE tool_use blocks with empty id or name cause downstream API errors. The fix validates both fields are present and non-empty before creating a `ToolUse` block; otherwise, a placeholder `Text` block is pushed to maintain index alignment (filtered by `finish()`).

`unwrap()` on `child.stdout.take()` / `child.stderr.take()` violates the no-panics contract for tools. Replacing with `.ok_or("...")` makes the invariant explicit.

Thread `join()` with `unwrap_or_default()` silently hides panics in stdout/stderr drain threads. Using `map_err(|_| "...thread panicked")?` surfaces the failure as a tool error instead.

Unbounded recursion in walk() risks stack overflow on deep directory trees or symlink loops. A depth limit (MAX_WALK_DEPTH=20) is a simple safety net.

Labeled stdout/stderr separator helps the model distinguish between streams. Using `--- stderr ---` as a structural marker makes the boundary unambiguous.

OOB index on content_block_stop silently drops tool input. Logging the OOB index at the point of occurrence makes debugging much easier.

Bash command guard deny-list blocks destructive patterns (rm -rf /, fork bombs, dd to block devices) at the tool dispatch level before shell execution. The system prompt's soft instruction 'never run destructive ops without approval' is not enforcement. The guard provides hard enforcement. Patterns are checked case-insensitively against the command string.

Bash command guard must cover flag ordering variations. `rm -rf /` is blocked but `rm -fr /` (reversed flag order) performs the same operation and was not caught. Added `-fr` variants, separate flags (`rm -r -f`), long flags (`rm --recursive --force`), and `chmod 777 /` without `-R`.

Bash command guard whitespace normalization closes a bypass vector. The model could generate `rm  -rf  /` (double space) or `rm\t-rf\t/` (tabs). Normalizing the command (lowercase + split_whitespace + join with single space) before pattern matching catches these variations.

Bash timeout drain thread leak. When a command times out, the child is killed but the drain threads reading stdout/stderr were not joined — they were dropped detached. Fixed by joining drain threads after kill+wait on the timeout path.

walk() error resilience: skip vs abort on permission-denied. Changed to `let Ok(entry) = entry else { continue }` to silently skip inaccessible entries instead of aborting the entire directory listing.

Tool error display in non-verbose mode improves UX without flooding output. Shows error results (is_error: true) with 200-char truncation, keeping successful tool calls quiet.

--max-tokens as a CLI parameter (default 16384) removes the hardcoded constant from api.rs. The parameter flows from Cli struct through main to send_message.

search_exec truncation order was wrong: byte-size cap before line-count cap. Fixed by applying the 50-line cap first, then the byte cap as a safety net.

Tool result visibility in non-verbose mode matches Go reference pattern. Adding result size display (e.g. 'result: 1234 chars') gives users tool execution feedback without flooding output.

Tool schema descriptions should encode operational limits. Enriching schemas ensures the model sees constraints regardless of which source it attends to.

edit_file schema description was missing the 1MB size limit that other tools included. Schema descriptions must match the operational limits enforced in code.

Retry-After header on 429 responses surfaces actionable info. Extracting and displaying this header value gives users the wait time instead of a generic 429 error.

Bash command guard device coverage must span all common device families. Initially only `/dev/sd` and `/dev/nvme` were covered for `dd of=` patterns, and only `/dev/sd` for redirect (`>`). Virtual disk devices (`/dev/vd` for KVM/QEMU) and legacy IDE devices (`/dev/hd`) are equally destructive targets. Redirect patterns must mirror `dd of=` coverage to prevent shell redirect bypasses.

Tool schema descriptions must match the actual skip directory list. The `list_files` schema said "Skips .git, node_modules, target, .venv, vendor" but `SKIP_DIRS` also includes `.devenv`. The model could make decisions based on the schema description alone, leading to confusion when `.devenv` directories are unexpectedly filtered. Schema descriptions are a contract with the model.

Production line count reduction requires rustfmt-aware refactoring. rustfmt expands compressed `format!` calls and method chains beyond ~90 characters. Safe approaches: inlining temporary variables into `ok_or_else` closures, combining doc comment lines, eliminating intermediate String allocations (`push_str(&format!(...))` → direct `format!`). Unsafe approaches: single-line `format!` calls with positional args (rustfmt splits), `.then()` chains (rustfmt expands).

## Future Work

Subagent dispatch (spec R8). The SubagentContext type was removed as dead code. StopReason enum remains for dispatch loop control. Integration point comments removed from main.rs. Actual dispatch logic remains unimplemented per spec's non-goals.

Ralph-guard integration. Hook wiring exists per commit b8974bd. Integration points: activity logging, guard policy enforcement, audit trail.

Performance profiling. No benchmarking done. Areas: ripgrep spawn latency, conversation memory growth patterns.

Error recovery. No retry logic for transient API failures. Spec explicitly states "No automatic retry; failures return to user for decision."

## Spec Alignment

The specification has been updated to reflect implementation decisions:

- R3 updated: enum example replaced with tools! macro pattern that matches implementation.
- R4 hardened: read_file enforces 1MB size limit and detects binary files. list_files supports optional `recursive` parameter (default: false), output capped at 1000 entries. bash_exec truncates output at 100KB. Directory filter works at any depth. R4 now uses `code_search` tool name. bash_exec validates commands against a deny-list of destructive patterns before execution.
- R5 updated: stdin pipe detection via `std::io::IsTerminal`, prompts suppressed in non-interactive mode. Piped stdin reads all input as a single prompt. --max-tokens CLI flag added (default 16384). send_message signature extended with max_tokens: u32 parameter.
- R6 clarified: only api.rs uses thiserror for error types. Tools return raw string errors in tool_result blocks with is_error: true.
- R7 implemented: `StopReason` enum parsed from `message_delta` SSE event. Inner loop breaks on `EndTurn`, warns on `MaxTokens`. Partial tool_use blocks filtered on truncation.
- R8 updated: SubagentContext removed as dead code per engineering philosophy.
- SSE hardened: Unknown block types handled via placeholder blocks. Mid-stream error events explicitly detected. Empty text blocks filtered. Incomplete streams detected via missing stop_reason.
- System prompt upgraded from static string to dynamic `build_system_prompt()` with cwd, platform injection and structured tool/safety guidance.
- `send_message` signature extended with `system_prompt: &str` parameter.
- specs/README.md line count corrected (<870 → <880) to match coding-agent.md.
- Production line counting: main.rs 321 + api.rs 255 + tools/mod.rs 300 = 876 total.
- Production line counting standardized: all lines before #[cfg(test)] in each source file.
- Tool descriptions enriched to match Go reference quality with usage guidance.
- max_tokens increased from 8192 to 16384 for better Opus performance (API supports up to 128K).
- SSE parser now has 17 unit tests covering the full event processing state machine.
- Tool dispatch handles corrupt tool_use blocks with null input by sending error tool_results.
- Implementation Notes expanded with conversation trimming, dynamic system prompt, NO_COLOR, reqwest timeouts, bash command guard, API error recovery, tool loop iteration limit.
- Bash command guard expanded: redirect-to-device patterns (`> /dev/nvme`, `> /dev/vd`, `> /dev/hd`) and dd-to-device patterns (`of=/dev/vd`, `of=/dev/hd`) cover all common device families.
- list_files schema description updated to include `.devenv` to match SKIP_DIRS constant.

## Verification Checklist

[x] All 5 tools implemented: read_file, list_files, bash, edit_file, code_search
[x] SSE streaming with real-time output and stop_reason parsing (R7)
[x] SSE unknown block type handling, mid-stream error events, index validation
[x] Partial tool_use blocks with null input filtered on MaxTokens truncation
[x] CLI: --verbose, --model, --max-tokens flags, stdin pipe detection (R5)
[x] Event loop follows Go reference structure with explicit stop_reason check
[x] Tool dispatch with is_error propagation
[x] Bash non-zero exit/timeout returns is_error: true
[x] Bash pipe deadlock prevention (threaded stdout/stderr draining)
[x] Bash output truncation (100KB limit)
[x] System prompt: structured workflow instructions, tool guidance, safety rules
[x] StopReason enum: EndTurn, ToolUse, MaxTokens
[x] Non-interactive mode: suppresses prompts when stdin is piped
[x] read_file: 1MB size limit, binary detection via null byte check
[x] list_files: recursive parameter, SKIP_DIRS filter, 1000 entry cap, sorted output
[x] edit_file: 1MB size limit, schema documents create/append mode for empty old_str
[x] Conversation context management: trim at exchange boundaries, 720KB budget, oversized block truncation
[x] SSE incomplete stream detection, trailing buffer processing
[x] cargo fmt/clippy/build/test passes (117 unit tests)
[x] API error recovery: pop trailing User message + orphaned tool_use
[x] SSE parser extracted with 17 tests
[x] Tool loop iteration limit (50) with recover_conversation call
[x] Piped stdin reads all input as single prompt
[x] NO_COLOR convention respected
[x] Corrupt tool_use blocks produce error tool_results
[x] SSE parser validates non-empty id/name on tool_use blocks
[x] bash_exec: unwrap() eliminated, thread panics surfaced as errors
[x] walk() depth limit (MAX_WALK_DEPTH=20), permission-denied entries skipped
[x] bash stdout/stderr labeled separator ('--- stderr ---')
[x] Bash command guard: deny-list blocks rm -rf, fork bombs, dd-to-device (sd/nvme/vd/hd), mkfs, chmod 777
[x] Bash command guard: expanded flag orderings (rm -r -f, --recursive --force, chmod 777 /)
[x] Bash command guard: whitespace normalization
[x] Bash timeout: drain threads joined on timeout path
[x] Tool errors displayed in non-verbose mode (200-char truncation)
[x] Tool result visibility in non-verbose mode (shows result size)
[x] Tool schema descriptions enriched with operational limits
[x] Retry-After header surfaced on 429 rate limit responses
[x] Dynamic system prompt: cwd, platform, tool guidance
[x] send_message accepts system_prompt and max_tokens parameters
[x] reqwest client: connect_timeout (30s) and request timeout (300s)
[x] search_exec: 50-line cap applied before 100KB byte cap
[x] Bash command guard: redirect-to-device patterns cover all four device families (/dev/sd, /dev/nvme, /dev/vd, /dev/hd)
[x] list_files schema description includes .devenv in skip list
[x] ~879 production lines (321 main.rs + 254 api.rs + 304 tools/mod.rs)
