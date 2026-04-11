#!/usr/bin/env bash
# bench-run.sh — Launch a benchmark run on the remote server
#
# Usage:
#   ./scripts/bench-run.sh                          # default: regex-chess, gpt-5.4, xhigh
#   ./scripts/bench-run.sh --task my-task           # specific task
#   ./scripts/bench-run.sh --model openai/gpt-5.4   # specific model
#   ./scripts/bench-run.sh --effort high            # specific effort
#   ./scripts/bench-run.sh --watch                  # launch + auto-observe every 15s
#
# Environment (set before running, or in ~/.bench-env):
#   BENCH_HOST         SSH host alias (default: flcs)
#   OPENAI_BASE_URL    API proxy URL
#   OPENAI_API_KEY     API key
#   PUFFER_DIR         Remote puffer directory (default: /home/jerry/puffer)

set -euo pipefail

# Load env file if present
[ -f ~/.bench-env ] && source ~/.bench-env

BENCH_HOST="${BENCH_HOST:-flcs}"
PUFFER_DIR="${PUFFER_DIR:-/home/jerry/puffer}"
OPENAI_BASE_URL="${OPENAI_BASE_URL:-http://84.32.32.146:8317/v1}"
OPENAI_API_KEY="${OPENAI_API_KEY:-}"

# Defaults
TASK="regex-chess"
MODEL="openai/gpt-5.4"
EFFORT="xhigh"
PROVIDER="openai"
PARALLELISM=1
RETRIES=0
WATCH=false

# Parse args
while [[ $# -gt 0 ]]; do
    case "$1" in
        --task)     TASK="$2"; shift 2 ;;
        --model)    MODEL="$2"; shift 2 ;;
        --effort)   EFFORT="$2"; shift 2 ;;
        --provider) PROVIDER="$2"; shift 2 ;;
        --watch)    WATCH=true; shift ;;
        --help|-h)
            head -12 "$0" | tail -10
            exit 0 ;;
        *) echo "Unknown arg: $1"; exit 1 ;;
    esac
done

if [ -z "$OPENAI_API_KEY" ]; then
    echo "Error: OPENAI_API_KEY not set. Export it or put in ~/.bench-env"
    exit 1
fi

TIME_TAG="run-$(date -u +%Y%m%dT%H%M%SZ)"

echo "━━━ Benchmark Launch ━━━"
echo "  Host:     $BENCH_HOST"
echo "  Task:     $TASK"
echo "  Model:    $MODEL"
echo "  Effort:   $EFFORT"
echo "  Tag:      $TIME_TAG"
echo ""

# Step 1: Pull latest + rebuild
echo "→ Pulling latest code and building..."
ssh "$BENCH_HOST" "cd $PUFFER_DIR && git pull origin master 2>&1 | tail -2 && source \$HOME/.cargo/env && cargo build --release --target x86_64-unknown-linux-musl 2>&1 | tail -2"

# Step 2: Launch benchmark
echo ""
echo "→ Launching benchmark..."
ssh "$BENCH_HOST" "cd $PUFFER_DIR/.worktree/bench && \
source /home/jerry/venv/bin/activate && \
export OPENAI_BASE_URL='$OPENAI_BASE_URL' && \
export OPENAI_API_KEY='$OPENAI_API_KEY' && \
nohup python3 benchmark/run_tb2.py \
  --task '$TASK' \
  --parallelism $PARALLELISM \
  --max-agent-retries $RETRIES \
  --time-tag '$TIME_TAG' \
  --puffer-bin '$PUFFER_DIR/target/x86_64-unknown-linux-musl/release/puffer' \
  --resources-dir '$PUFFER_DIR/resources' \
  --codex-dir /root/.codex \
  --model '$MODEL' \
  --provider '$PROVIDER' \
  --effort '$EFFORT' \
  > /tmp/bench-$TIME_TAG.log 2>&1 &
echo \$!"

echo ""
echo "→ Benchmark launched in background."
echo "  Log:  ssh $BENCH_HOST 'tail -f /tmp/bench-$TIME_TAG.log'"
echo "  Observe: ssh $BENCH_HOST 'bash -s' < scripts/bench-observe.sh"
echo ""

if [ "$WATCH" = true ]; then
    echo "→ Watching progress (Ctrl+C to stop)..."
    echo ""
    while true; do
        clear
        ssh "$BENCH_HOST" "bash -s" < "$(dirname "$0")/bench-observe.sh" 2>/dev/null || true
        echo ""
        echo "  ⟳ Refreshing in 15s... (Ctrl+C to stop)"
        sleep 15
    done
fi
