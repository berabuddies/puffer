#!/usr/bin/env bash
# bench-observe.sh — Real-time benchmark observation dashboard
#
# Usage:
#   ./scripts/bench-observe.sh              # watch latest run
#   ./scripts/bench-observe.sh run8-xxx     # watch specific run
#   ./scripts/bench-observe.sh --list       # list all runs
#
# Requires: runs on the benchmark host (flcs), or via:
#   ssh flcs 'bash -s' < scripts/bench-observe.sh

set -euo pipefail

TRAJECTORY_ROOT="${PUFFER_TRAJECTORY_ROOT:-/home/jerry/puffer/.worktree/bench/benchmark/tb2-trajectory}"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

list_runs() {
    echo -e "${BOLD}Available runs:${NC}"
    for dir in "$TRAJECTORY_ROOT"/*/; do
        [ -d "$dir" ] || continue
        tag=$(basename "$dir")
        tasks=$(find "$dir" -maxdepth 1 -mindepth 1 -type d | wc -l)
        summary="$dir/run-summary.json"
        if [ -f "$summary" ]; then
            solved=$(python3 -c "import json; d=json.load(open('$summary')); print(d.get('solved',0))" 2>/dev/null || echo "?")
            total=$(python3 -c "import json; d=json.load(open('$summary')); print(d.get('total',0))" 2>/dev/null || echo "?")
            echo -e "  ${CYAN}$tag${NC}  tasks=$tasks  solved=$solved/$total"
        else
            echo -e "  ${YELLOW}$tag${NC}  tasks=$tasks  (in progress)"
        fi
    done
}

find_run_dir() {
    if [ -n "${1:-}" ] && [ "$1" != "--list" ]; then
        echo "$TRAJECTORY_ROOT/$1"
    else
        ls -td "$TRAJECTORY_ROOT"/*/ 2>/dev/null | head -1
    fi
}

show_task_status() {
    local task_dir="$1"
    local task_name
    task_name=$(basename "$task_dir")

    echo -e "\n${BOLD}━━━ Task: ${CYAN}$task_name${NC} ${BOLD}━━━${NC}"

    # Check result
    if [ -f "$task_dir/result.json" ]; then
        local solved
        solved=$(python3 -c "
import json
d = json.load(open('$task_dir/result.json'))
ei = d.get('exception_info') or {}
if ei.get('exception_type'):
    print('ERROR: ' + ei['exception_type'])
elif d.get('status') == 'completed':
    print('COMPLETED')
else:
    print(d.get('status', 'unknown'))
" 2>/dev/null || echo "unknown")
        echo -e "  Result: ${GREEN}$solved${NC}"
    else
        echo -e "  Result: ${YELLOW}in progress${NC}"
    fi

    # Check rewards
    if [ -f "$task_dir/rewards.json" ]; then
        python3 -c "
import json
d = json.load(open('$task_dir/rewards.json'))
for k, v in d.items():
    print(f'  Reward {k}: {v}')
" 2>/dev/null
    fi

    # Docker container status
    local container_name="${task_name}-main-1"
    if docker ps --format '{{.Names}} {{.Status}}' 2>/dev/null | grep -q "$container_name"; then
        local status
        status=$(docker ps --format '{{.Status}}' --filter "name=$container_name" 2>/dev/null)
        echo -e "  Container: ${GREEN}$status${NC}"

        # Check puffer process
        local puffer_pid
        puffer_pid=$(docker exec "$container_name" pidof puffer 2>/dev/null || echo "")
        if [ -n "$puffer_pid" ]; then
            echo -e "  Puffer PID: ${GREEN}$puffer_pid${NC}"

            # TCP connections
            local established
            established=$(docker exec "$container_name" sh -c 'cat /proc/net/tcp 2>/dev/null | grep -c " 01 "' 2>/dev/null || echo 0)
            echo -e "  Active connections: $established"

            # Generated files
            local re_size
            re_size=$(docker exec "$container_name" sh -c 'wc -c /app/re.json 2>/dev/null' 2>/dev/null || echo "N/A")
            echo -e "  /app/re.json: $re_size"
        else
            echo -e "  Puffer: ${RED}not running${NC}"
        fi
    else
        echo -e "  Container: ${RED}not running${NC}"
    fi

    # Agent logs
    local agent_dir="$task_dir/agent"
    if [ -d "$agent_dir" ]; then
        # puffer.txt
        local puffer_size
        puffer_size=$(wc -c < "$agent_dir/puffer.txt" 2>/dev/null || echo 0)
        echo -e "  puffer.txt: ${puffer_size} bytes"

        # Incremental trajectory
        local incr="$agent_dir/trajectory.incremental.jsonl"
        if [ -f "$incr" ]; then
            local tool_count
            tool_count=$(wc -l < "$incr")
            echo -e "  Tool invocations: ${BOLD}$tool_count${NC}"

            # Show tool timeline with details
            python3 -c "
import json, sys, textwrap

with open('$incr') as f:
    lines = f.readlines()
if not lines:
    sys.exit()

first_ts = None
prev_ts = None
for i, line in enumerate(lines):
    d = json.loads(line.strip())
    ts = d.get('timestamp', 0)
    if first_ts is None:
        first_ts = ts
    elapsed = (ts - first_ts) / 1000
    gap = (ts - prev_ts) / 1000 if prev_ts else 0
    prev_ts = ts

    tool = d.get('tool_id', '?')
    ok = '\033[0;32m✓\033[0m' if d.get('success') else '\033[0;31m✗\033[0m'
    gap_str = f'(+{gap:.0f}s)' if gap > 2 else ''

    # Parse input for context
    inp = d.get('input', '')
    detail = ''
    try:
        inp_obj = json.loads(inp) if inp.startswith('{') else {}
        if tool == 'Read':
            detail = inp_obj.get('file_path', inp_obj.get('path', ''))
        elif tool == 'Write':
            detail = inp_obj.get('file_path', '')
        elif tool == 'Edit':
            detail = inp_obj.get('file_path', '')
        elif tool == 'Bash':
            cmd = inp_obj.get('command', '')
            detail = cmd[:60] + ('...' if len(cmd) > 60 else '')
        elif tool == 'Grep':
            detail = inp_obj.get('pattern', '')[:40]
        elif tool == 'Glob':
            detail = inp_obj.get('pattern', '')[:40]
        else:
            detail = inp[:50]
    except:
        detail = inp[:50] if inp else ''

    # Output preview for failures
    out_info = ''
    if not d.get('success'):
        out = d.get('output', '')[:100]
        if out:
            out_info = f' \033[0;31m{out}\033[0m'

    # Truncation indicator
    trunc = ' [truncated]' if d.get('output_truncated') else ''

    print(f'    {elapsed:7.1f}s  {ok} {tool:<6} {detail}{gap_str}{trunc}{out_info}')
" 2>/dev/null
        fi

        # result.json
        if [ -f "$agent_dir/result.json" ]; then
            echo -e "  Agent result: ${GREEN}exists${NC}"
            python3 -c "
import json
d = json.load(open('$agent_dir/result.json'))
print(f\"    success={d.get('success')}  tools={len(d.get('tool_invocations',[]))}  text={len(d.get('assistant_text',''))} chars\")
" 2>/dev/null
        fi
    fi

    # Session file (benchmark writes to ~/.puffer/sessions/ or container's /app/.puffer/sessions/)
    local session_jsonl=""
    # Try container first
    if docker ps --format '{{.Names}}' 2>/dev/null | grep -q "$container_name"; then
        session_jsonl=$(docker exec "$container_name" sh -c 'ls -t /root/.puffer/sessions/*.jsonl /app/.puffer/sessions/*.jsonl 2>/dev/null | head -1' 2>/dev/null || true)
        if [ -n "$session_jsonl" ]; then
            echo -e "  ${BOLD}Session transcript:${NC} (in container: $session_jsonl)"
            docker exec "$container_name" cat "$session_jsonl" 2>/dev/null | python3 -c "
import sys, json
for i, line in enumerate(sys.stdin, 1):
    line = line.strip()
    if not line: continue
    try:
        d = json.loads(line)
        t = d.get('type', '?')
        text = d.get('text', '')
        if t == 'user_message':
            preview = text.replace(chr(10), ' ')[:80]
            print(f'    {i:3d} \033[0;36m[user]\033[0m {preview}')
        elif t == 'assistant_message':
            preview = text.replace(chr(10), ' ')[:80]
            print(f'    {i:3d} \033[0;32m[asst]\033[0m {preview}')
        elif t == 'system_message':
            if text.startswith('Tool '):
                parts = text.split(chr(10), 2)
                tool_line = parts[0][:60]
                print(f'    {i:3d} \033[0;33m[tool]\033[0m {tool_line}')
            else:
                preview = text.replace(chr(10), ' ')[:60]
                print(f'    {i:3d} \033[0;90m[sys]\033[0m  {preview}')
        else:
            print(f'    {i:3d} [{t}]')
    except: pass
" 2>/dev/null
        fi
    fi

    # Exception
    if [ -f "$task_dir/exception.txt" ]; then
        echo -e "  ${RED}Exception:${NC}"
        tail -3 "$task_dir/exception.txt" | sed 's/^/    /'
    fi

    # Harbor output tail
    if [ -f "$task_dir/harbor-output.txt" ]; then
        local harbor_size
        harbor_size=$(wc -c < "$task_dir/harbor-output.txt")
        echo -e "  harbor-output.txt: $harbor_size bytes"
    fi
}

# Main
if [ "${1:-}" = "--list" ]; then
    list_runs
    exit 0
fi

RUN_DIR=$(find_run_dir "${1:-}")
if [ -z "$RUN_DIR" ] || [ ! -d "$RUN_DIR" ]; then
    echo -e "${RED}No runs found in $TRAJECTORY_ROOT${NC}"
    echo "Usage: $0 [run-tag|--list]"
    exit 1
fi

RUN_TAG=$(basename "$RUN_DIR")
echo -e "${BOLD}━━━ Benchmark Run: ${CYAN}$RUN_TAG${NC} ${BOLD}━━━${NC}"
echo -e "  Directory: $RUN_DIR"

# Selection info
if [ -f "$RUN_DIR/selection.json" ]; then
    python3 -c "
import json
d = json.load(open('$RUN_DIR/selection.json'))
print(f\"  Tasks: {len(d.get('tasks', []))}  Model: {d.get('model','?')}  Effort: {d.get('effort','?')}\")
" 2>/dev/null
fi

# Summary if complete
if [ -f "$RUN_DIR/run-summary.json" ]; then
    python3 -c "
import json
d = json.load(open('$RUN_DIR/run-summary.json'))
s = d.get('solved', 0)
t = d.get('total', 0)
f = d.get('agent_failures', [])
print(f'  Solved: {s}/{t}')
if f:
    print(f'  Failures: {\", \".join(f)}')
" 2>/dev/null
fi

# Each task
for task_dir in "$RUN_DIR"/*/; do
    [ -d "$task_dir" ] || continue
    # Skip non-task dirs
    [ -f "$task_dir/config.json" ] || continue
    show_task_status "$task_dir"
done

echo ""
