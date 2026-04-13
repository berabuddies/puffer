# Terminal Bench 2.0

This directory holds the tracked Puffer TB2 harness and ignores generated benchmark artifacts.

## Setup

```bash
python3.12 -m venv benchmark/.venv-harbor
benchmark/.venv-harbor/bin/pip install harbor
benchmark/.venv-harbor/bin/harbor dataset download terminal-bench/terminal-bench-2 --output-dir benchmark/harbor-cache/tasks
```

## Run 5 Random Tasks

```bash
python3 benchmark/run_tb2.py --count 5 --parallelism 1
```

Add `--seed <n>` for a reproducible sample, or `--task <slug>` to run specific tasks.
