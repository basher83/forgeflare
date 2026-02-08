#!/bin/bash
# Ralph Wiggum Loop — Rust Coding Agent
# Usage: ./loop.sh [--json] [plan] [max_iterations]
# Examples:
#   ./loop.sh              # Build mode, human output, unlimited
#   ./loop.sh --json       # Build mode, JSON output
#   ./loop.sh 20           # Build mode, max 20 iterations
#   ./loop.sh plan         # Plan mode, unlimited
#   ./loop.sh plan 5       # Plan mode, max 5 iterations
#   ./loop.sh --json plan  # Plan mode, JSON output

# Parse --json flag
OUTPUT_FORMAT=""
if [ "$1" = "--json" ]; then
    OUTPUT_FORMAT="--output-format=stream-json"
    shift
fi

# Parse mode and iterations
if [ "$1" = "plan" ]; then
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
        break
    fi

    # Run Ralph iteration
    # Background + wait pattern enables signal handling during execution
    # -p: Headless mode (non-interactive, reads from stdin)
    # --dangerously-skip-permissions: Auto-approve all tool calls
    # --model opus: Opus for task selection/prioritization
    cat "$PROMPT_FILE" | claude -p \
        --dangerously-skip-permissions \
        $OUTPUT_FORMAT \
        --model opus \
        --verbose &
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
