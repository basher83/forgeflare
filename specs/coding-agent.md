# Unified Rust Coding Agent Specification

**Status:** Active
**Target:** Single binary, streaming, subagent-aware, <850 production lines
**Pin:** Go source at `/reference/go-source/` — pattern-match against working code

---

## Requirements

**R1. Core Loop**
- REPL: read user input → call Claude API → dispatch tools → repeat
- Streaming responses (SSE, not batch) from day 1
- Conversation history management (add user input, add assistant message to context array)

**R2. HTTP Client**
- Build own with `reqwest` (not third-party Anthropic crate)
- Anthropic API: POST to `/v1/messages`
- Handle SSE for streaming responses
- Expose `ANTHROPIC_API_KEY` env var

**R3. Tool Registry Pattern**
```rust
// tools! macro generates both all_tool_schemas() and dispatch_tool() from one definition
tools! {
    "read_file", "Read a file", schema, read_exec;
    "list_files", "List files", schema, list_exec;
    // ...
}
```
- 5 tools total; each follows Anthropic tool_use spec
- Tool schemas are sent with every API request, providing introspection natively
- Single macro generates schema and dispatch, preventing divergence

**R4. Five Tools**
1. **Read** — read_file(path) → file contents (handle binary, size limits)
2. **List** — list_files(path, recursive?) → [files]
3. **Bash** — bash(command, cwd?) → stdout/stderr (timeout)
4. **Edit** — edit_file(path, old_str, new_str) → success/error (exact match semantics)
5. **Search** — code_search(pattern, path?, file_type?, case_sensitive?) → matches (shell out to `rg`)

**R5. CLI Interface**
- Single binary, no subcommands required
- `--verbose` flag for debug output
- `--model` flag (default: claude-opus-4-6)
- Read prompts/context from stdin if available, interactive prompt otherwise
- Exit gracefully on EOF or "exit" command

**R6. Error Handling**
- `thiserror` for structured error types in api.rs (`AgentError`)
- Tool errors returned as `Result<String, String>` — raw strings flow into tool_result text
- Display errors to user, continue loop (don't panic)

**R7. Streaming Architecture**
- Collect SSE events into a response buffer
- Check `stop_reason` for "tool_use" vs "end_turn"
- If tool_use: extract tool calls, execute, send results back
- If end_turn: display response to user, prompt for next input

**R8. Subagent Awareness (Deferred)**
- Explicitly deferred per non-goals; no SubagentContext types in codebase
- StopReason enum is the only surviving artifact (used for tool dispatch loop control)
- Future subagent dispatch would add a StopReason variant or context field

---

## Architecture

```
src/
  main.rs         — CLI, loop, error handling
  api.rs          — Anthropic client (reqwest + SSE)
  tools/
    mod.rs        — All 5 tools with tools! macro (read, list, bash, edit, search)

reference/
  go-source/      — Cloned Go workshop code (pin)
```

---

## Success Criteria

- [x] Binary compiles (`cargo build`)
- [x] Tests pass (`cargo test`)
- [x] Clippy clean (`cargo clippy -- -D warnings`)
- [x] Formatted (`cargo fmt --check`)
- [x] Can chat with Claude
- [x] Can read files
- [x] Can list directories
- [x] Can run bash commands
- [x] Can edit files (exact-match semantics)
- [x] Can search code
- [x] <850 production lines (808 actual: 300 main.rs + 247 api.rs + 261 tools/mod.rs)
- [x] Streaming responses visible to user in real-time

---

## Dependencies

- `reqwest` — HTTP client (with stream feature)
- `serde` + `serde_json` — JSON serialization
- `tokio` — async runtime (full features)
- `clap` — CLI parsing (derive feature)
- `thiserror` — error types
- `futures-util` — stream consumption for SSE parsing
- `wait-timeout` — bash command timeout protection

---

## Non-Goals

- Progressive binaries (Phases 1-3 merged into single unified agent)
- Batch mode (streaming from day 1)
- Provider abstraction (Anthropic only)
- Interactive line editing (simple readline via stdin)
- Subagent execution (StopReason enum retained for loop control, SubagentContext removed)

---

## Implementation Notes

- Use exact match for `edit_file` (one old_str appearance exactly, new_str differs)
- Tool dispatch is synchronous; async only for HTTP and command execution
- Context accumulates in memory; no persistence layer
- No automatic retry; failures return to user for decision
- Search tool shells out to `rg` (must be installed)

---

## Reference: Go Source

The Go workshop (`reference/go-source/`) contains 6 progressive versions:
- `chat.go` — bare event loop
- `read.go` — +read_file tool
- `list_files.go` — +list_files tool
- `bash_tool.go` — +bash tool
- `edit_tool.go` — +edit_file tool
- `code_search_tool.go` — +code_search tool

Study the event loop in `edit_tool.go` (lines 126-214) as the canonical loop pattern. The Rust implementation should follow the same structure: API call → check response → dispatch tools → send results → repeat.
