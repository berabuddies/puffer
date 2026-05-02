"""Harbor agent that runs Burbot inside Terminal Bench tasks."""

from __future__ import annotations

import json
import os
import shlex
from pathlib import Path
from typing import Any

from harbor.agents.installed.base import BaseInstalledAgent, CliFlag, with_prompt_template
from harbor.environments.base import BaseEnvironment
from harbor.models.agent.context import AgentContext


class BurbotBenchAgent(BaseInstalledAgent):
    """Run the local Burbot binary against a Harbor task workspace."""

    SUPPORTS_ATIF: bool = True

    CLI_FLAGS = [
        CliFlag("provider", cli="--provider", type="str", default="openai"),
        CliFlag("effort", cli="--effort", type="str", default="xhigh"),
        CliFlag("fast", cli="--fast", type="bool", default=True),
        CliFlag("auth_source", cli="--auth-source", type="str", default="local-codex"),
        CliFlag("yolo", cli="--yolo", type="bool", default=False),
    ]

    def __init__(
        self,
        logs_dir: Path,
        burbot_bin_path: str = "/opt/puffer/burbot",
        tools_dir: str = "/opt/puffer/resources/tools",
        codex_dir: str = "/opt/puffer/codex",
        auth_source: str = "local-codex",
        provider: str = "openai",
        effort: str = "xhigh",
        fast: bool = True,
        yolo: bool = False,
        *args: Any,
        **kwargs: Any,
    ) -> None:
        super().__init__(logs_dir, *args, **kwargs)
        self._burbot_bin_path = burbot_bin_path
        self._tools_dir = tools_dir
        self._codex_dir = codex_dir
        self.auth_source = auth_source
        self.provider = provider
        self.effort = effort
        self.fast = fast
        self.yolo = yolo

    @staticmethod
    def name() -> str:
        """Return Harbor's display name for this custom agent."""
        return "burbot-benchmark"

    def get_version_command(self) -> str | None:
        """Skip Harbor version autodetection for the mounted local binary."""
        return None

    async def install(self, environment: BaseEnvironment) -> None:
        """Validate mounts and mirror the host Codex config into the container home."""
        quoted_bin = shlex.quote(self._burbot_bin_path)
        quoted_tools = shlex.quote(self._tools_dir)
        quoted_codex_dir = shlex.quote(self._codex_dir)
        await self.exec_as_agent(
            environment,
            command=(
                "set -euo pipefail; "
                f"test -x {quoted_bin}; "
                f"test -d {quoted_tools}; "
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
                f"{quoted_bin} run --help >/dev/null"
            ),
        )

    @staticmethod
    def _parse_burbot_summary(text: str) -> dict[str, Any] | None:
        """Return the last embedded summary object Burbot wrote to stdout."""
        decoder = json.JSONDecoder()
        summary: dict[str, Any] | None = None
        for index, char in enumerate(text):
            if char != "{":
                continue
            try:
                value, _ = decoder.raw_decode(text[index:])
            except json.JSONDecodeError:
                continue
            if isinstance(value, dict) and "status" in value and "run_id" in value:
                summary = value
        return summary

    @staticmethod
    def _selected_artifact_action(
        summary: dict[str, Any] | None,
    ) -> dict[str, Any] | None:
        """Extract the artifact action descriptor for Harbor metadata."""
        if not isinstance(summary, dict):
            return None
        artifact = summary.get("artifact")
        if not isinstance(artifact, dict):
            return None
        action_name = artifact.get("action_name")
        contract_id = artifact.get("contract_id")
        if action_name is None and contract_id is None:
            return None
        return {"action_name": action_name, "contract_id": contract_id}

    def populate_context_post_run(self, context: AgentContext) -> None:
        """Parse the captured Burbot stdout and emit result + trajectory artifacts."""
        agent_dir = self.logs_dir
        burbot_log = agent_dir / "burbot.txt"
        rc_path = agent_dir / "burbot.rc"

        stdout = burbot_log.read_text(errors="replace") if burbot_log.exists() else ""
        rc: int | None = None
        if rc_path.exists():
            try:
                rc = int(rc_path.read_text().strip())
            except ValueError:
                rc = None

        summary = self._parse_burbot_summary(stdout)
        artifact_action = self._selected_artifact_action(summary)
        burbot_status = summary.get("status") if isinstance(summary, dict) else None
        burbot_run_id = summary.get("run_id") if isinstance(summary, dict) else None
        burbot_open_actions = (
            summary.get("open_actions") if isinstance(summary, dict) else None
        )
        success = rc == 0 and burbot_status == "completed"

        debug_dir = agent_dir / "burbot-debug"
        debug_dir_str = str(debug_dir) if debug_dir.exists() else None

        provider = str(getattr(self, "provider", "openai"))
        auth_source = str(getattr(self, "auth_source", "local-codex"))
        effort = str(getattr(self, "effort", "xhigh"))
        fast_mode = bool(getattr(self, "fast", True))
        prompt = getattr(self, "_prompt", "")
        command = getattr(self, "_command", [])

        result = {
            "success": success,
            "process_return_code": rc,
            "provider": provider,
            "auth_source": auth_source,
            "model": self.model_name,
            "effort": effort,
            "fast_mode": fast_mode,
            "prompt": prompt,
            "assistant_text": stdout,
            "burbot_summary": summary,
            "burbot_status": burbot_status,
            "burbot_run_id": burbot_run_id,
            "burbot_open_actions": burbot_open_actions,
            "burbot_selected_artifact_action": artifact_action,
            "burbot_debug_dir": debug_dir_str,
            "error": None if success else stdout[-4000:],
        }
        (agent_dir / "result.json").write_text(json.dumps(result, indent=2) + "\n")

        trajectory = {
            "type": "burbot_run",
            "command": command,
            "return_code": rc,
            "process_return_code": rc,
            "burbot_summary": summary,
            "burbot_status": burbot_status,
            "burbot_run_id": burbot_run_id,
            "burbot_open_actions": burbot_open_actions,
            "burbot_selected_artifact_action": artifact_action,
            "stdout": stdout,
            "burbot_debug_dir": debug_dir_str,
        }
        (agent_dir / "trajectory.json").write_text(
            json.dumps(trajectory, indent=2) + "\n"
        )

        context.metadata = {
            "assistant_text": stdout,
            "burbot_open_actions": burbot_open_actions,
            "burbot_run_id": burbot_run_id,
            "burbot_debug_dir": debug_dir_str,
            "burbot_selected_artifact_action": artifact_action,
            "burbot_status": burbot_status,
            "effort": effort,
            "fast_mode": fast_mode,
            "model": self.model_name,
            "process_return_code": rc,
            "provider": provider,
            "success": success,
            "auth_source": auth_source,
        }

    @with_prompt_template
    async def run(
        self,
        instruction: str,
        environment: BaseEnvironment,
        context: AgentContext,
    ) -> None:
        """Execute one unattended Burbot benchmark turn inside the task workspace."""
        if not self.model_name:
            raise ValueError("Model name is required")

        auth_source = str(getattr(self, "auth_source", "local-codex"))
        force_local_codex = auth_source in {
            "codex",
            "local-codex",
            "codex-oauth",
            "local-codex-oauth",
        }
        env: dict[str, str] = {"BURBOT_OPENAI_AUTH_SOURCE": auth_source}
        forwarded_keys = [
            "BURBOT_OPENAI_TIMEOUT_SECS",
            "BURBOT_OPENAI_RETRY_ATTEMPTS",
            "PUFFER_OPENAI_STREAM_READ_TIMEOUT_MS",
        ]
        if not force_local_codex:
            forwarded_keys.extend(["OPENAI_API_KEY", "OPENAI_BASE_URL"])
        for key in forwarded_keys:
            value = os.environ.get(key, "")
            if value:
                env[key] = value

        yolo_enabled = bool(getattr(self, "yolo", False))
        self._prompt = instruction
        self._command = [
            self._burbot_bin_path,
            "run",
            "--goal",
            instruction,
            "--tools",
            self._tools_dir,
            "--llm-tool-call",
            "--model",
            self.model_name,
        ]
        if yolo_enabled:
            self._command.append("--yolo")

        prompt_path = "/tmp/burbot-benchmark-prompt.txt"
        quoted_instruction = shlex.quote(instruction)
        quoted_prompt_path = shlex.quote(prompt_path)
        quoted_bin = shlex.quote(self._burbot_bin_path)
        quoted_tools = shlex.quote(self._tools_dir)
        quoted_model = shlex.quote(self.model_name)
        yolo_flag = " --yolo" if yolo_enabled else ""

        await self.exec_as_agent(
            environment,
            command=(
                "set -uo pipefail; "
                "mkdir -p /logs/agent; "
                f"printf '%s' {quoted_instruction} > {quoted_prompt_path}; "
                f"{quoted_bin} run "
                "--goal \"$("
                f"cat {quoted_prompt_path}"
                ")\" "
                f"--tools {quoted_tools} "
                "--llm-tool-call "
                f"--model {quoted_model}"
                f"{yolo_flag} "
                ">/logs/agent/burbot.txt 2>&1; "
                "rc=$?; "
                "printf '%s' \"$rc\" > /logs/agent/burbot.rc; "
                "trace_src=\"$(pwd)/.puffer/burbot\"; "
                "if [ -d \"$trace_src\" ]; then "
                "  rm -rf /logs/agent/burbot-debug; "
                "  cp -r \"$trace_src\" /logs/agent/burbot-debug; "
                "fi; "
                "cat /logs/agent/burbot.txt; "
                "exit 0"
            ),
            env=env,
        )
