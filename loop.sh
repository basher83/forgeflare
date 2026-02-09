#!/bin/bash
# Ralph Wiggum Loop — Rust Coding Agent
# Usage: ./loop.sh [--json] [plan|plan-work "description"] [max_iterations]
# Examples:
#   ./loop.sh              # Build mode, human output, unlimited
#   ./loop.sh --json       # Build mode, JSON output
#   ./loop.sh 20           # Build mode, max 20 iterations
#   ./loop.sh plan         # Plan mode, unlimited
#   ./loop.sh plan 5       # Plan mode, max 5 iterations
#   ./loop.sh --json plan  # Plan mode, JSON output
#   ./loop.sh plan-work "user auth with OAuth"  # Scoped plan for work branch

# Parse --json flag
OUTPUT_FORMAT=""
if [ "$1" = "--json" ]; then
    OUTPUT_FORMAT="--output-format=stream-json"
    shift
fi

# Parse mode and iterations
if [ "$1" = "plan-work" ]; then
    if [ -z "$2" ]; then
        echo "Error: plan-work requires a work description"
        echo "Usage: ./loop.sh plan-work \"description of the work\""
        exit 1
    fi
    MODE="plan-work"
    PROMPT_FILE="PROMPT_plan_work.md"
    export WORK_SCOPE="$2"
    MAX_ITERATIONS=${3:-5}
elif [ "$1" = "plan" ]; then
    MODE="plan"
    PROMPT_FILE="PROMPT_plan.md"
    MAX_ITERATIONS=${2:-0}
elif [[ "$1" =~ ^[0-9]+$ ]]; then
    MODE="build"
    PROMPT_FILE="PROMPT_build.md"
    MAX_ITERATIONS=$1
else
    MODE="build"
    PROMPT_FILE="PROMPT_build.md"
    MAX_ITERATIONS=0
fi

ITERATION=0
CURRENT_BRANCH=$(git branch --show-current)
CLAUDE_PID=""

# Validate branch for plan-work mode
if [ "$MODE" = "plan-work" ]; then
    if [ "$CURRENT_BRANCH" = "main" ] || [ "$CURRENT_BRANCH" = "master" ]; then
        echo "Error: plan-work should be run on a work branch, not main/master"
        echo "Create a work branch first: git checkout -b ralph/your-work"
        exit 1
    fi
fi

# --- Sandbox pre-flight check ---
# Warn if no sandbox boundary is detected. Does not block — the operator
# may have a sandbox mechanism this check cannot detect.
check_sandbox() {
    # Container indicators
    [ -f /.dockerenv ] && return 0
    [ "${CONTAINER:-}" = "true" ] && return 0
    grep -qE '/docker/|/lxc/' /proc/1/cgroup 2>/dev/null && return 0
    # Claude Code native sandbox (bubblewrap/seatbelt)
    command -v bwrap >/dev/null 2>&1 && return 0
    [ "$(uname)" = "Darwin" ] && return 0  # Seatbelt available on macOS
    return 1
}
if ! check_sandbox; then
    echo "⚠  WARNING: No sandbox boundary detected."
    echo "   The loop uses --dangerously-skip-permissions (all tool calls auto-approved)."
    echo "   Recommended: enable Claude Code native sandbox (/sandbox command)"
    echo "   or run inside a container (docker sandbox run claude)."
    echo ""
    echo "   Continuing in 5 seconds... (Ctrl+C to abort)"
    sleep 5
fi

# Signal handler — kill claude process and exit cleanly
cleanup() {
    echo -e "\n\nCaught signal, stopping..."
    if [ -n "$CLAUDE_PID" ] && kill -0 "$CLAUDE_PID" 2>/dev/null; then
        kill -TERM "$CLAUDE_PID" 2>/dev/null
        sleep 0.5
        kill -9 "$CLAUDE_PID" 2>/dev/null
    fi
    exit 130
}
trap cleanup SIGINT SIGTERM SIGQUIT

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Ralph Wiggum Loop — Rust Coding Agent"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Mode:   $MODE"
echo "Prompt: $PROMPT_FILE"
echo "Output: ${OUTPUT_FORMAT:-human}"
echo "Branch: $CURRENT_BRANCH"
[ "$MODE" = "plan-work" ] && echo "Scope:  $WORK_SCOPE"
[ $MAX_ITERATIONS -gt 0 ] && echo "Max:    $MAX_ITERATIONS iterations"
echo "Stop:   Ctrl+C"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# Verify prompt file exists
if [ ! -f "$PROMPT_FILE" ]; then
    echo "Error: $PROMPT_FILE not found"
    exit 1
fi

while true; do
    if [ $MAX_ITERATIONS -gt 0 ] && [ $ITERATION -ge $MAX_ITERATIONS ]; then
        echo "Reached max iterations: $MAX_ITERATIONS"
        if [ "$MODE" = "plan-work" ]; then
            echo ""
            echo "Scoped plan created for: $WORK_SCOPE"
            echo "To build: ./loop.sh"
        fi
        break
    fi

    # Run Ralph iteration
    # Background + wait pattern enables signal handling during execution
    # -p: Headless mode (non-interactive, reads from stdin)
    # --dangerously-skip-permissions: Auto-approve all tool calls
    # --model opus: Opus for task selection/prioritization
    # plan-work mode: envsubst substitutes ${WORK_SCOPE} in the prompt template
    if [ "$MODE" = "plan-work" ]; then
        envsubst < "$PROMPT_FILE" | claude -p \
            --dangerously-skip-permissions \
            $OUTPUT_FORMAT \
            --model opus \
            --verbose &
    else
        cat "$PROMPT_FILE" | claude -p \
            --dangerously-skip-permissions \
            $OUTPUT_FORMAT \
            --model opus \
            --verbose &
    fi
    CLAUDE_PID=$!
    wait $CLAUDE_PID
    CLAUDE_PID=""

    # Push changes after each iteration
    git push origin "$CURRENT_BRANCH" 2>/dev/null || {
        echo "Creating remote branch..."
        git push -u origin "$CURRENT_BRANCH"
    }

    # Convergence detection (build mode only)
    if [ "$MODE" = "build" ] && [ -f ".claude/hooks/convergence-check.sh" ]; then
        if ! bash .claude/hooks/convergence-check.sh; then
            echo "Loop auto-terminated: convergence detected after $((ITERATION + 1)) iterations"
            break
        fi
    fi

    ITERATION=$((ITERATION + 1))
    echo -e "\n\n════════════════════ LOOP $ITERATION ════════════════════\n"
done
