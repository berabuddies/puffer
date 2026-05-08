"""Harbor agent that runs Puffer's hidden benchmark command inside TB2 tasks."""

from __future__ import annotations

import json
import os
import shlex
from pathlib import Path
from typing import Any

from harbor.agents.installed.base import BaseInstalledAgent, CliFlag, with_prompt_template
from harbor.environments.base import BaseEnvironment
from harbor.models.agent.context import AgentContext


class PufferBenchAgent(BaseInstalledAgent):
    """Run the local Puffer binary against a Harbor task workspace."""

    SUPPORTS_ATIF: bool = True

    CLI_FLAGS = [
        CliFlag("provider", cli="--provider", type="str", default="openai"),
        CliFlag("effort", cli="--effort", type="str", default="xhigh"),
        CliFlag("fast", cli="--fast", type="bool", default=True),
    ]

    def __init__(
        self,
        logs_dir: Path,
        puffer_bin_path: str = "/opt/puffer/puffer",
        resources_dir: str = "/opt/puffer/resources",
        codex_dir: str = "/opt/puffer/codex",
        *args: Any,
        **kwargs: Any,
    ) -> None:
        self._puffer_bin_path = puffer_bin_path
        self._resources_dir = resources_dir
        self._codex_dir = codex_dir
        super().__init__(logs_dir, *args, **kwargs)

    @staticmethod
    def name() -> str:
        """Return Harbor's display name for this custom agent."""
        return "puffer-benchmark"

    def get_version_command(self) -> str | None:
        """Skip Harbor version autodetection for the mounted local binary."""
        return None

    async def install(self, environment: BaseEnvironment) -> None:
        """Validate mounts and mirror the host Codex config into the container home."""
        quoted_bin = shlex.quote(self._puffer_bin_path)
        quoted_codex_dir = shlex.quote(self._codex_dir)
        await self.exec_as_agent(
            environment,
            command=(
                "set -euo pipefail; "
                f"test -x {quoted_bin}; "
                'agent_home="${HOME:-/root}"; '
                'mkdir -p "$agent_home/.codex"; '
                f"if [ -d {quoted_codex_dir} ]; then "
                f"  if [ -f {quoted_codex_dir}/auth.json ]; then "
                f'    cp {quoted_codex_dir}/auth.json "$agent_home/.codex/auth.json"; '
                "  fi; "
                f"  if [ -f {quoted_codex_dir}/config.toml ]; then "
                f'    cp {quoted_codex_dir}/config.toml "$agent_home/.codex/config.toml"; '
                "  fi; "
                "fi; "
                f"{quoted_bin} benchmark-run --help >/dev/null"
            ),
        )

    def populate_context_post_run(self, context: AgentContext) -> None:
        """Populate Harbor metadata from Puffer's result artifact when available."""
        result_path = self.logs_dir / "result.json"
        if not result_path.exists():
            return

        try:
            payload = json.loads(result_path.read_text())
        except (OSError, json.JSONDecodeError):
            return

        context.metadata = {
            "assistant_text": payload.get("assistant_text"),
            "effort": payload.get("effort"),
            "fast_mode": payload.get("fast_mode"),
            "model": payload.get("model"),
            "provider": payload.get("provider"),
            "success": payload.get("success"),
            # Categorical failure tag (currently only quota-family).
            # `run_tb2.py` reads this to delay retry on quota events
            # instead of burning the budget back-to-back. See
            # `crates/puffer-core/runtime/quota.rs`.
            "error_kind": payload.get("error_kind"),
        }

    @with_prompt_template
    async def run(
        self,
        instruction: str,
        environment: BaseEnvironment,
        context: AgentContext,
    ) -> None:
        """Execute one unattended Puffer benchmark turn inside the task workspace."""
        if not self.model_name:
            raise ValueError("Model name is required")

        env: dict[str, str] = {}
        for key in (
            "OPENAI_API_KEY",
            "OPENAI_BASE_URL",
            "PUFFER_OPENAI_STREAM_READ_TIMEOUT_MS",
        ):
            value = os.environ.get(key, "")
            if value:
                env[key] = value

        prompt_path = "/tmp/puffer-benchmark-prompt.txt"
        quoted_instruction = shlex.quote(instruction)
        quoted_bin = shlex.quote(self._puffer_bin_path)
        quoted_model = shlex.quote(self.model_name)
        quoted_resources = shlex.quote(self._resources_dir)
        cli_flags = self.build_cli_flags()
        flag_suffix = f" {cli_flags}" if cli_flags else ""

        await self.exec_as_agent(
            environment,
            command=(
                "set -euo pipefail; "
                f"printf '%s' {quoted_instruction} > {prompt_path}; "
                'agent_home="${HOME:-/root}"; '
                'export PUFFER_BENCHMARK_WORKING_DIRS="'
                '/app:/data:/tests:/workspace:/etc:/usr:/var:/tmp:${agent_home}/.claude"; '
                f"export PUFFER_BUILTIN_RESOURCES_DIR={quoted_resources}; "
                f"{quoted_bin} benchmark-run "
                f"--prompt-file {prompt_path} "
                f"--model {quoted_model}"
                f"{flag_suffix} "
                "--result-json /logs/agent/result.json "
                "--trajectory-json /logs/agent/trajectory.json "
                "2>&1 | tee /logs/agent/puffer.txt"
            ),
            env=env,
        )
