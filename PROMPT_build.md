# Building Prompt — Rust Coding Agent

0a. Study `specs/*` with up to 200 parallel Sonnet subagents to learn the coding agent specification.

0b. Study @IMPLEMENTATION_PLAN.md.

0c. For reference, the source code is in `src/*`. The Go source is pinned at `reference/go-source/` — use it as a pattern reference.

1. Your task is to implement functionality per the specification using parallel subagents. Follow @IMPLEMENTATION_PLAN.md and choose the most important item to address. Before making changes, search the codebase (don't assume not implemented) using Sonnet subagents. You may use up to 200 parallel Sonnet subagents for searches/reads and only 1 Sonnet subagent for build/tests. Use Opus subagents when complex reasoning is needed (debugging, architectural decisions, event loop design). Study `reference/go-source/edit_tool.go` (lines 126-214) to understand the canonical event loop pattern.

2. After implementing functionality or resolving problems, run the tests for that unit of code that was improved. If functionality is missing then it's your job to add it as per the application specifications. Ultrathink.

3. When you discover issues, immediately update @IMPLEMENTATION_PLAN.md with your findings using a subagent. When resolved, update and remove the item.

4. When the tests pass, update @IMPLEMENTATION_PLAN.md, then `git add -A` then `git commit` with a message describing the changes. After the commit, `git push`.

99999. Important: When authoring documentation, capture the why — tests and implementation importance.

999999. Important: Single sources of truth, no migrations/adapters. If tests unrelated to your work fail, resolve them as part of the increment.

9999999. As soon as there are no build or test errors create a git tag. If there are no git tags start at 0.0.0 and increment patch by 1.

99999999. You may add extra logging if required to debug issues.

999999999. Keep @IMPLEMENTATION_PLAN.md current with learnings using a subagent — future work depends on this to avoid duplicating efforts.

9999999999. When you learn something new about how to run the application, update @AGENTS.md using a subagent but keep it brief.

99999999999. For any bugs you notice, resolve them or document them in @IMPLEMENTATION_PLAN.md using a subagent even if unrelated to the current work.

999999999999. Implement functionality completely. Placeholders and stubs waste efforts and time redoing the same work.

9999999999999. When @IMPLEMENTATION_PLAN.md becomes large periodically clean out the items that are completed from the file using a subagent.

99999999999999. If you find inconsistencies in the specs/* then use an Opus subagent with 'ultrathink' requested to update the specs.

999999999999999. IMPORTANT: Keep @AGENTS.md operational only — status updates and progress notes belong in IMPLEMENTATION_PLAN.md. A bloated AGENTS.md pollutes every future loop's context.
