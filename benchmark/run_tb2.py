#!/usr/bin/env python3
"""Run Harbor Terminal Bench trials against the local Puffer binary."""

from __future__ import annotations

import argparse
import json
import os
import random
import shlex
import shutil
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import asdict, dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


REPO_ROOT = Path(__file__).resolve().parents[1]
HARBOR_BIN = REPO_ROOT / "benchmark/.venv-harbor/bin/harbor"
TRAJECTORY_ROOT = REPO_ROOT / "benchmark/tb2-trajectory"
TASK_ROOT_CANDIDATES = [
    REPO_ROOT / "benchmark/harbor-cache/tasks/terminal-bench",
    REPO_ROOT / "benchmark/harbor-cache/tasks/terminal-bench-2",
]
PUFFER_AGENT_IMPORT = "benchmark.puffer_harbor_agent:PufferBenchAgent"
BURBOT_AGENT_IMPORT = "benchmark.burbot_harbor_agent:BurbotBenchAgent"
DATASET_DOWNLOAD_COMMAND = (
    "benchmark/.venv-harbor/bin/harbor dataset download "
    "terminal-bench/terminal-bench-2 --output-dir benchmark/harbor-cache/tasks"
)


@dataclass(frozen=True)
class TaskEntry:
    """One cached TB2 task available for execution."""

    slug: str
    task_dir: Path


@dataclass
class TrialSummary:
    """Outcome details for one task trial."""

    slug: str
    task_dir: str
    trial_dir: str
    attempts: int
    return_code: int
    solved: bool
    rewards: dict[str, float | int] | None
    exception_type: str | None
    exception_message: str | None
    retry_exhausted: bool
    burbot_status: str | None
    burbot_run_id: str | None
    burbot_open_actions: int | None
    burbot_selected_artifact_action: dict[str, Any] | None
    burbot_debug_dir: str | None
    process_return_code: int | None
    agent_log: str


def parse_args() -> argparse.Namespace:
    """Parse CLI arguments for a TB2 sample run."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--count", type=int, default=5, help="Number of tasks to sample.")
    parser.add_argument(
        "--parallelism",
        type=int,
        default=5,
        help="Maximum number of concurrent Harbor trials.",
    )
    parser.add_argument(
        "--max-agent-retries",
        type=int,
        default=5,
        help="Retries for Harbor command failures before batch backoff.",
    )
    parser.add_argument(
        "--allow-unsolved",
        action="store_true",
        help="Exit zero for completed but unsolved benchmark attempts.",
    )
    parser.add_argument(
        "--sleep-after-exhausted-seconds",
        type=int,
        default=60,
        help="Sleep duration before re-running failed tasks with lower parallelism.",
    )
    parser.add_argument(
        "--time-tag",
        default=datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ"),
        help="Output directory tag under benchmark/tb2-trajectory/.",
    )
    parser.add_argument(
        "--seed",
        type=int,
        default=None,
        help="Random seed for sampling. Defaults to a time-derived seed.",
    )
    parser.add_argument(
        "--task",
        action="append",
        default=[],
        help="Specific task slug(s) to run instead of random sampling.",
    )
    parser.add_argument(
        "--agent",
        choices=["puffer", "burbot"],
        default="puffer",
        help="Benchmark agent adapter to mount and run.",
    )
    parser.add_argument(
        "--model",
        default=os.environ.get("PUFFER_BENCH_MODEL", "openai/gpt-5.4"),
        help=(
            "Model selector recorded by Harbor and passed to the agent. "
            "Defaults to PUFFER_BENCH_MODEL or openai/gpt-5.4."
        ),
    )
    parser.add_argument(
        "--effort",
        default="xhigh",
        help="Puffer effort level for benchmark-run.",
    )
    parser.add_argument(
        "--provider",
        default="openai",
        help="Provider passed to Puffer benchmark-run.",
    )
    parser.add_argument(
        "--fast",
        dest="fast",
        action="store_true",
        default=True,
        help="Enable Puffer fast mode.",
    )
    parser.add_argument(
        "--no-fast",
        dest="fast",
        action="store_false",
        help="Disable Puffer fast mode.",
    )
    parser.add_argument(
        "--puffer-bin",
        default=None,
        help="Host path to the Puffer binary to mount into Harbor environments.",
    )
    parser.add_argument(
        "--burbot-bin",
        default=None,
        help="Host path to the Burbot binary to mount into Harbor environments.",
    )
    parser.add_argument(
        "--burbot-auth-source",
        default="local-codex",
        choices=("local-codex", "codex", "codex-oauth", "local-codex-oauth", "env"),
        help="OpenAI auth source for Burbot runs. Default uses the local Codex credential.",
    )
    parser.add_argument(
        "--burbot-yolo",
        action="store_true",
        default=False,
        help="Bypass Burbot's safety gate inside the trial container.",
    )
    parser.add_argument(
        "--resources-dir",
        default=str(REPO_ROOT / "resources"),
        help="Host path to Puffer resources.",
    )
    parser.add_argument(
        "--codex-dir",
        default=str(Path.home() / ".codex"),
        help="Host path to the local Codex config directory.",
    )
    parser.add_argument(
        "--override-cpus",
        type=int,
        default=None,
        help="Override Harbor environment CPUs. Defaults to detected Docker CPUs.",
    )
    parser.add_argument(
        "--override-memory-mb",
        type=int,
        default=None,
        help="Override Harbor environment memory in MB.",
    )
    parser.add_argument(
        "--override-storage-mb",
        type=int,
        default=None,
        help="Override Harbor environment storage in MB.",
    )
    parser.add_argument(
        "--override-gpus",
        type=int,
        default=None,
        help="Override Harbor environment GPU count.",
    )
    return parser.parse_args()


def resolve_puffer_bin(explicit_path: str | None) -> Path:
    """Resolve the mounted Puffer binary path."""
    if explicit_path:
        path = Path(explicit_path).expanduser().resolve()
        if not path.is_file():
            raise FileNotFoundError(f"Puffer binary not found: {path}")
        return path

    candidates = [
        REPO_ROOT / "target/release/puffer",
        REPO_ROOT / "target/debug/puffer",
    ]
    for path in candidates:
        if path.is_file():
            return path
    raise FileNotFoundError("No Puffer binary found in target/release or target/debug")


def resolve_burbot_bin(explicit_path: str | None) -> Path:
    """Resolve the mounted Burbot binary path."""
    if explicit_path:
        path = Path(explicit_path).expanduser().resolve()
        if not path.is_file():
            raise FileNotFoundError(f"Burbot binary not found: {path}")
        return path

    candidates = [
        REPO_ROOT / "target/release/burbot",
        REPO_ROOT / "target/debug/burbot",
    ]
    for path in candidates:
        if path.is_file():
            return path
    raise FileNotFoundError("No Burbot binary found in target/release or target/debug")


def resolve_harbor_bin() -> Path:
    """Resolve the local Harbor CLI path used to launch trials."""
    if HARBOR_BIN.is_file():
        return HARBOR_BIN
    raise FileNotFoundError(
        "Harbor CLI not found at "
        f"{HARBOR_BIN}. Install it with:\n"
        "python3.12 -m venv benchmark/.venv-harbor\n"
        "benchmark/.venv-harbor/bin/pip install harbor"
    )


def discover_tasks() -> list[TaskEntry]:
    """Return all locally cached Terminal Bench task directories."""
    tasks_by_dir: dict[Path, TaskEntry] = {}
    for root in TASK_ROOT_CANDIDATES:
        for task_toml in sorted(root.glob("*/task.toml")):
            task_dir = task_toml.parent
            tasks_by_dir[task_dir] = TaskEntry(slug=task_dir.name, task_dir=task_dir)
        for task_toml in sorted(root.glob("*/*/task.toml")):
            task_dir = task_toml.parent
            tasks_by_dir[task_dir] = TaskEntry(slug=task_dir.parent.name, task_dir=task_dir)
    tasks = sorted(tasks_by_dir.values(), key=lambda task: (task.slug, str(task.task_dir)))
    if not tasks:
        checked = "\n".join(str(root) for root in TASK_ROOT_CANDIDATES)
        raise FileNotFoundError(
            f"No cached tasks found under:\n{checked}\n"
            "Download the dataset with:\n"
            f"{DATASET_DOWNLOAD_COMMAND}"
        )
    return tasks


def select_tasks(
    all_tasks: list[TaskEntry],
    requested_tasks: list[str],
    count: int,
    seed: int,
) -> list[TaskEntry]:
    """Choose either the requested tasks or a random sample."""
    by_slug = {task.slug: task for task in all_tasks}
    if requested_tasks:
        missing = [slug for slug in requested_tasks if slug not in by_slug]
        if missing:
            raise KeyError(f"Unknown task slug(s): {', '.join(sorted(missing))}")
        return [by_slug[slug] for slug in requested_tasks]

    if count > len(all_tasks):
        raise ValueError(f"Requested {count} tasks but only found {len(all_tasks)}")

    rng = random.Random(seed)
    return sorted(rng.sample(all_tasks, count), key=lambda task: task.slug)


def build_mounts(
    agent: str,
    puffer_bin: Path,
    burbot_bin: Path | None,
    resources_dir: Path,
    codex_dir: Path,
) -> list[dict[str, Any]]:
    """Build Docker bind mounts for Harbor's task container."""
    mounts: list[dict[str, Any]] = []
    if agent == "burbot":
        if burbot_bin is None:
            raise ValueError("Burbot binary is required")
        mounts.extend(
            [
                {
                    "type": "bind",
                    "source": str(burbot_bin),
                    "target": "/opt/puffer/burbot",
                    "read_only": True,
                },
            ]
        )
    else:
        mounts.append(
            {
                "type": "bind",
                "source": str(puffer_bin),
                "target": "/opt/puffer/puffer",
                "read_only": True,
            }
        )
    mounts.append(
        {
            "type": "bind",
            "source": str(resources_dir),
            "target": "/opt/puffer/resources",
            "read_only": True,
        }
    )
    if codex_dir.is_dir():
        mounts.append(
            {
                "type": "bind",
                "source": str(codex_dir),
                "target": "/opt/puffer/codex",
                "read_only": True,
                "bind": {"create_host_path": False},
            }
        )
    return mounts


def load_trial_result(trial_dir: Path) -> tuple[dict[str, Any] | None, dict[str, Any] | None]:
    """Load Harbor's result artifact and the verifier reward summary when present."""
    result_path = trial_dir / "result.json"
    if not result_path.is_file():
        return None, None
    result = json.loads(result_path.read_text())
    rewards = ((result.get("verifier_result") or {}).get("rewards")) or None
    return result, rewards


def harbor_attempt_failed(result: dict[str, Any] | None) -> bool:
    """Treat Harbor trial exceptions and explicit agent failures as retryable failures."""
    if not result:
        return False
    if result.get("exception_info"):
        return True
    metadata = ((result.get("agent_result") or {}).get("metadata")) or {}
    return metadata.get("success") is False


def burbot_metadata(result: dict[str, Any] | None) -> dict[str, Any]:
    """Return Burbot adapter telemetry from Harbor agent metadata when present."""
    if not result:
        return {}
    metadata = ((result.get("agent_result") or {}).get("metadata")) or {}
    if not isinstance(metadata, dict):
        return {}
    return metadata


def solved_from_rewards(rewards: dict[str, float | int] | None) -> bool:
    """Treat a task as solved when every reported reward is positive."""
    if not rewards:
        return False
    return all(float(value) > 0 for value in rewards.values())


def backup_trial_dir(trial_dir: Path, attempt_index: int) -> None:
    """Move an existing failed trial directory aside before retrying."""
    if not trial_dir.exists():
        return
    backup_dir = trial_dir.with_name(f"{trial_dir.name}--retry-{attempt_index:02d}")
    while backup_dir.exists():
        attempt_index += 1
        backup_dir = trial_dir.with_name(f"{trial_dir.name}--retry-{attempt_index:02d}")
    shutil.move(str(trial_dir), str(backup_dir))


def run_single_task(
    task: TaskEntry,
    trial_root: Path,
    harbor_env: dict[str, str],
    mounts_json: str,
    args: argparse.Namespace,
) -> TrialSummary:
    """Run one Harbor trial with retry-on-error semantics for agent failures."""
    trial_dir = trial_root / task.slug
    agent_import_path = (
        BURBOT_AGENT_IMPORT if args.agent == "burbot" else PUFFER_AGENT_IMPORT
    )

    for attempt in range(1, args.max_agent_retries + 2):
        if attempt > 1:
            backup_trial_dir(trial_dir, attempt - 1)

        command = [
            str(resolve_harbor_bin()),
            "trial",
            "start",
            "--path",
            str(task.task_dir),
            "--trial-name",
            task.slug,
            "--trials-dir",
            str(trial_root),
            "--agent-import-path",
            agent_import_path,
            "--model",
            args.model,
            "--agent-kwarg",
            f"provider={args.provider}",
            "--agent-kwarg",
            f"effort={args.effort}",
            "--agent-kwarg",
            f"fast={'true' if args.fast else 'false'}",
            "--agent-kwarg",
            "codex_dir=/opt/puffer/codex",
            "--environment-type",
            "docker",
            "--no-force-build",
            "--delete",
            "--agent-setup-timeout",
            "180",
            "--mounts-json",
            mounts_json,
        ]
        if args.agent == "burbot":
            command.extend(
                [
                    "--agent-kwarg",
                    "burbot_bin_path=/opt/puffer/burbot",
                    "--agent-kwarg",
                    "tools_dir=/opt/puffer/resources/tools",
                    "--agent-kwarg",
                    f"auth_source={args.burbot_auth_source}",
                    "--agent-kwarg",
                    f"yolo={'true' if args.burbot_yolo else 'false'}",
                ]
            )
        else:
            command.extend(
                [
                    "--agent-kwarg",
                    "puffer_bin_path=/opt/puffer/puffer",
                    "--agent-kwarg",
                    "resources_dir=/opt/puffer/resources",
                ]
            )
        if args.override_cpus is not None:
            command.extend(["--override-cpus", str(args.override_cpus)])
        if args.override_memory_mb is not None:
            command.extend(["--override-memory-mb", str(args.override_memory_mb)])
        if args.override_storage_mb is not None:
            command.extend(["--override-storage-mb", str(args.override_storage_mb)])
        if args.override_gpus is not None:
            command.extend(["--override-gpus", str(args.override_gpus)])
        completed = subprocess.run(
            command,
            cwd=REPO_ROOT,
            env=harbor_env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            check=False,
        )

        trial_dir.mkdir(parents=True, exist_ok=True)
        (trial_dir / "harbor-command.txt").write_text(shlex.join(command) + "\n")
        (trial_dir / "harbor-output.txt").write_text(completed.stdout)

        result, rewards = load_trial_result(trial_dir)
        if completed.returncode == 0 and not harbor_attempt_failed(result):
            exception_info = (result or {}).get("exception_info") or {}
            metadata = burbot_metadata(result)
            return TrialSummary(
                slug=task.slug,
                task_dir=str(task.task_dir),
                trial_dir=str(trial_dir),
                attempts=attempt,
                return_code=0,
                solved=solved_from_rewards(rewards),
                rewards=rewards,
                exception_type=exception_info.get("exception_type"),
                exception_message=exception_info.get("exception_message"),
                retry_exhausted=False,
                burbot_status=metadata.get("burbot_status"),
                burbot_run_id=metadata.get("burbot_run_id"),
                burbot_open_actions=metadata.get("burbot_open_actions"),
                burbot_selected_artifact_action=metadata.get(
                    "burbot_selected_artifact_action"
                ),
                burbot_debug_dir=metadata.get("burbot_debug_dir"),
                process_return_code=metadata.get("process_return_code"),
                agent_log=str(trial_dir / "agent" / "burbot.txt"),
            )

        if (
            completed.returncode == 0
            and harbor_attempt_failed(result)
            and attempt <= args.max_agent_retries
        ):
            continue

        if attempt > args.max_agent_retries:
            result, rewards = load_trial_result(trial_dir)
            exception_info = (result or {}).get("exception_info") or {}
            metadata = burbot_metadata(result)
            return TrialSummary(
                slug=task.slug,
                task_dir=str(task.task_dir),
                trial_dir=str(trial_dir),
                attempts=attempt,
                return_code=completed.returncode or 1,
                solved=False,
                rewards=rewards,
                exception_type=exception_info.get("exception_type"),
                exception_message=exception_info.get("exception_message"),
                retry_exhausted=True,
                burbot_status=metadata.get("burbot_status"),
                burbot_run_id=metadata.get("burbot_run_id"),
                burbot_open_actions=metadata.get("burbot_open_actions"),
                burbot_selected_artifact_action=metadata.get(
                    "burbot_selected_artifact_action"
                ),
                burbot_debug_dir=metadata.get("burbot_debug_dir"),
                process_return_code=metadata.get("process_return_code"),
                agent_log=str(trial_dir / "agent" / "burbot.txt"),
            )

    raise AssertionError("unreachable")


def run_batch(
    tasks: list[TaskEntry],
    trial_root: Path,
    harbor_env: dict[str, str],
    mounts_json: str,
    args: argparse.Namespace,
    parallelism: int,
) -> list[TrialSummary]:
    """Run a batch of tasks with bounded parallelism."""
    summaries: list[TrialSummary] = []
    with ThreadPoolExecutor(max_workers=parallelism) as executor:
        futures = {
            executor.submit(
                run_single_task,
                task,
                trial_root,
                harbor_env,
                mounts_json,
                args,
            ): task.slug
            for task in tasks
        }
        for future in as_completed(futures):
            summaries.append(future.result())
    return sorted(summaries, key=lambda summary: summary.slug)


def write_manifest(
    path: Path,
    agent: str,
    seed: int,
    parallelism: int,
    tasks: list[TaskEntry],
    puffer_bin: Path,
    burbot_bin: Path | None,
    resources_dir: Path,
    codex_dir: Path,
    model: str,
    effort: str,
    fast: bool,
    override_cpus: int | None,
    override_memory_mb: int | None,
    override_storage_mb: int | None,
    override_gpus: int | None,
    burbot_auth_source: str,
) -> None:
    """Persist the sampled task set and run configuration."""
    payload = {
        "agent": agent,
        "seed": seed,
        "parallelism": parallelism,
        "tasks": [
            {
                "slug": task.slug,
                "task_dir": str(task.task_dir),
            }
            for task in tasks
        ],
        "puffer_bin": str(puffer_bin),
        "burbot_bin": str(burbot_bin) if burbot_bin else None,
        "resources_dir": str(resources_dir),
        "codex_dir": str(codex_dir),
        "model": model,
        "effort": effort,
        "fast": fast,
        "override_cpus": override_cpus,
        "override_memory_mb": override_memory_mb,
        "override_storage_mb": override_storage_mb,
        "override_gpus": override_gpus,
        "burbot_auth_source": burbot_auth_source,
    }
    path.write_text(json.dumps(payload, indent=2) + "\n")


def main() -> int:
    """Run the requested TB2 sample and persist the batch summary."""
    args = parse_args()
    seed = args.seed if args.seed is not None else int(time.time())
    resolve_harbor_bin()
    puffer_bin = resolve_puffer_bin(args.puffer_bin)
    burbot_bin = (
        resolve_burbot_bin(args.burbot_bin) if args.agent == "burbot" else None
    )
    resources_dir = Path(args.resources_dir).expanduser().resolve()
    codex_dir = Path(args.codex_dir).expanduser().resolve()
    all_tasks = discover_tasks()
    selected_tasks = select_tasks(all_tasks, args.task, args.count, seed)

    trial_root = TRAJECTORY_ROOT / args.time_tag
    trial_root.mkdir(parents=True, exist_ok=True)
    write_manifest(
        trial_root / "selection.json",
        args.agent,
        seed,
        args.parallelism,
        selected_tasks,
        puffer_bin,
        burbot_bin,
        resources_dir,
        codex_dir,
        args.model,
        args.effort,
        args.fast,
        args.override_cpus,
        args.override_memory_mb,
        args.override_storage_mb,
        args.override_gpus,
        args.burbot_auth_source,
    )

    harbor_env = os.environ.copy()
    existing_pythonpath = harbor_env.get("PYTHONPATH")
    harbor_env["PYTHONPATH"] = (
        f"{REPO_ROOT}:{existing_pythonpath}" if existing_pythonpath else str(REPO_ROOT)
    )

    mounts_json = json.dumps(
        build_mounts(
            args.agent,
            puffer_bin,
            burbot_bin,
            resources_dir,
            codex_dir,
        )
    )

    parallelism = max(1, args.parallelism)
    remaining = selected_tasks
    all_summaries: dict[str, TrialSummary] = {}

    while remaining:
        batch = run_batch(
            remaining,
            trial_root,
            harbor_env,
            mounts_json,
            args,
            parallelism,
        )
        for summary in batch:
            all_summaries[summary.slug] = summary

        exhausted = [summary for summary in batch if summary.retry_exhausted]
        if not exhausted or parallelism == 1:
            break

        time.sleep(args.sleep_after_exhausted_seconds)
        parallelism = max(1, parallelism // 2)
        remaining = [
            next(task for task in selected_tasks if task.slug == summary.slug)
            for summary in exhausted
        ]

    ordered_summaries = [all_summaries[task.slug] for task in selected_tasks]
    solved_count = sum(1 for summary in ordered_summaries if summary.solved)
    total_count = len(ordered_summaries)
    failed = [
        summary
        for summary in ordered_summaries
        if summary.return_code != 0 and summary.retry_exhausted
    ]
    unsolved = [summary for summary in ordered_summaries if not summary.solved]
    summary_payload = {
        "parallelism_final": parallelism,
        "solved_count": solved_count,
        "total_count": total_count,
        "solve_rate": solved_count / total_count if total_count else 0.0,
        "agent_failure_count": len(failed),
        "unsolved_count": len(unsolved),
        "missing_reward_count": sum(
            1 for summary in ordered_summaries if summary.rewards is None
        ),
        "mean_attempts": (
            sum(summary.attempts for summary in ordered_summaries) / total_count
            if total_count
            else 0.0
        ),
        "burbot_status_by_task": {
            summary.slug: summary.burbot_status
            for summary in ordered_summaries
            if summary.burbot_status is not None
        },
        "summaries": [asdict(summary) for summary in ordered_summaries],
    }
    (trial_root / "run-summary.json").write_text(
        json.dumps(summary_payload, indent=2) + "\n"
    )

    for summary in summary_payload["summaries"]:
        status = "solved" if summary["solved"] else "unsolved"
        attempts = summary["attempts"]
        reason = summary.get("burbot_status") or summary.get("exception_type") or "reward=0"
        print(f"{summary['slug']}: {status} (attempts={attempts}, reason={reason})")
    if failed:
        print(
            "agent_failures="
            + ",".join(sorted(summary.slug for summary in failed)),
            file=sys.stderr,
        )
        return 1
    if unsolved and not args.allow_unsolved:
        print(
            "unsolved=" + ",".join(sorted(summary.slug for summary in unsolved)),
            file=sys.stderr,
        )
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
