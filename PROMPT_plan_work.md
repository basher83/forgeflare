# Work-Scoped Planning Prompt — Rust Coding Agent

0a. Study `specs/*` with up to 250 parallel Sonnet subagents to learn all active specifications.

0b. Study @IMPLEMENTATION_PLAN.md (if present) to understand the plan so far.

0c. Source code is in `src/*` (Rust agent) and `.github/workflows/` (CI/CD). The Go source is pinned at `reference/go-source/` — use it as a pattern reference.

1. You are creating a SCOPED implementation plan for work: "${WORK_SCOPE}". Study @IMPLEMENTATION_PLAN.md (if present; it may be incorrect) and use up to 500 Sonnet subagents to study existing source code in `src/*` and `.github/workflows/*` and compare against active specs in `specs/`. Use an Opus subagent to analyze findings, prioritize tasks, and create/update @IMPLEMENTATION_PLAN.md as a bullet point list sorted in priority of items yet to be implemented. Ultrathink. Consider searching for TODO, minimal implementations, placeholders, skipped/flaky tests, and inconsistent patterns. Study the Go source in `reference/go-source/` (specifically `edit_tool.go` lines 126-214) to understand the canonical event loop pattern and tool dispatch flow.

IMPORTANT: This is SCOPED PLANNING for "${WORK_SCOPE}" only. Create a plan containing ONLY tasks directly related to this work scope. Be conservative — if uncertain whether a task belongs to this work, exclude it. The plan can be regenerated if too narrow. Plan only. Do NOT implement anything. Do NOT assume functionality is missing; confirm with code search first. The Go source in `reference/` is the pin — understand it deeply, then pattern-match the Rust implementation against it.

ULTIMATE GOAL: We want to achieve the scoped work "${WORK_SCOPE}". Consider missing elements related to this work and plan accordingly. If an element is missing, search first to confirm it doesn't exist, then if needed author the specification at specs/FILENAME.md. If you create a new element then document the plan to implement it in @IMPLEMENTATION_PLAN.md using a subagent.

999999999. Keep @IMPLEMENTATION_PLAN.md current with learnings using a subagent — future work depends on this to avoid duplicating efforts.

9999999999. When you learn something new about how to run the application, update @AGENTS.md using a subagent but keep it brief.

99999999999. For any bugs you notice, document them in @IMPLEMENTATION_PLAN.md using a subagent even if unrelated to the current planning work.

999999999999. IMPORTANT: Keep @AGENTS.md operational only — status updates and progress notes belong in IMPLEMENTATION_PLAN.md. A bloated AGENTS.md pollutes every future loop's context.
