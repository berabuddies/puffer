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

    def populate_context_post_run(self, context: AgentContext) -> None:
        """Populate Harbor metadata from Burbot's result artifact when available."""
        result_path = self.logs_dir / "result.json"
        if not result_path.exists():
            return

        try:
            payload = json.loads(result_path.read_text())
        except (OSError, json.JSONDecodeError):
            return

        context.metadata = {
            "assistant_text": payload.get("assistant_text"),
            "burbot_open_actions": payload.get("burbot_open_actions"),
            "burbot_run_id": payload.get("burbot_run_id"),
            "burbot_debug_dir": payload.get("burbot_debug_dir"),
            "burbot_selected_artifact_action": payload.get(
                "burbot_selected_artifact_action"
            ),
            "burbot_status": payload.get("burbot_status"),
            "effort": payload.get("effort"),
            "fast_mode": payload.get("fast_mode"),
            "model": payload.get("model"),
            "process_return_code": payload.get("process_return_code"),
            "provider": payload.get("provider"),
            "success": payload.get("success"),
            "auth_source": payload.get("auth_source"),
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

        prompt_path = "/tmp/burbot-benchmark-prompt.txt"
        quoted_instruction = shlex.quote(instruction)
        quoted_bin = shlex.quote(self._burbot_bin_path)
        quoted_tools = shlex.quote(self._tools_dir)
        quoted_model = shlex.quote(self.model_name)
        quoted_provider = shlex.quote(str(getattr(self, "provider", "openai")))
        quoted_effort = shlex.quote(str(getattr(self, "effort", "xhigh")))
        quoted_fast = shlex.quote(str(getattr(self, "fast", True)).lower())

        await self.exec_as_agent(
            environment,
            command=(
                "set -euo pipefail; "
                f"printf '%s' {quoted_instruction} > {prompt_path}; "
                f"BURBOT_BIN={quoted_bin} "
                f"BURBOT_TOOLS={quoted_tools} "
                f"BURBOT_PROMPT_PATH={prompt_path} "
                f"BURBOT_MODEL={quoted_model} "
                f"BURBOT_PROVIDER={quoted_provider} "
                f"BURBOT_EFFORT={quoted_effort} "
                f"BURBOT_FAST={quoted_fast} "
                "python3 - <<'PY'\n"
                "import json\n"
                "import os\n"
                "import shutil\n"
                "import subprocess\n"
                "from pathlib import Path\n"
                "prompt = Path(os.environ['BURBOT_PROMPT_PATH']).read_text()\n"
                "cmd = [\n"
                "    os.environ['BURBOT_BIN'],\n"
                "    'run',\n"
                "    '--goal',\n"
                "    prompt,\n"
                "    '--tools',\n"
                "    os.environ['BURBOT_TOOLS'],\n"
                "    '--llm-tool-call',\n"
                "    '--model',\n"
                "    os.environ['BURBOT_MODEL'],\n"
                "]\n"
                "proc = subprocess.run(\n"
                "    cmd,\n"
                "    stdout=subprocess.PIPE,\n"
                "    stderr=subprocess.STDOUT,\n"
                "    text=True,\n"
                "    check=False,\n"
                ")\n"
                "debug_dir = Path('/logs/agent/burbot-debug')\n"
                "trace_source = Path.cwd() / '.puffer' / 'burbot'\n"
                "if trace_source.exists():\n"
                "    if debug_dir.exists():\n"
                "        shutil.rmtree(debug_dir)\n"
                "    shutil.copytree(trace_source, debug_dir)\n"
                "def parse_burbot_summary(text):\n"
                "    decoder = json.JSONDecoder()\n"
                "    summary = None\n"
                "    for index, char in enumerate(text):\n"
                "        if char != '{':\n"
                "            continue\n"
                "        try:\n"
                "            value, _ = decoder.raw_decode(text[index:])\n"
                "        except json.JSONDecodeError:\n"
                "            continue\n"
                "        if isinstance(value, dict) and 'status' in value and 'run_id' in value:\n"
                "            summary = value\n"
                "    return summary\n"
                "def selected_artifact_action(summary):\n"
                "    if not isinstance(summary, dict):\n"
                "        return None\n"
                "    artifact = summary.get('artifact')\n"
                "    if not isinstance(artifact, dict):\n"
                "        return None\n"
                "    action_name = artifact.get('action_name')\n"
                "    contract_id = artifact.get('contract_id')\n"
                "    if action_name is None and contract_id is None:\n"
                "        return None\n"
                "    return {\n"
                "        'action_name': action_name,\n"
                "        'contract_id': contract_id,\n"
                "    }\n"
                "burbot_summary = parse_burbot_summary(proc.stdout)\n"
                "artifact_action = selected_artifact_action(burbot_summary)\n"
                "burbot_status = (\n"
                "    burbot_summary.get('status') if isinstance(burbot_summary, dict) else None\n"
                ")\n"
                "agent_success = proc.returncode == 0 and burbot_status == 'completed'\n"
                "Path('/logs/agent/burbot.txt').write_text(proc.stdout)\n"
                "result = {\n"
                "    'success': agent_success,\n"
                "    'process_return_code': proc.returncode,\n"
                "    'provider': os.environ['BURBOT_PROVIDER'],\n"
                "    'auth_source': os.environ.get('BURBOT_OPENAI_AUTH_SOURCE'),\n"
                "    'model': os.environ['BURBOT_MODEL'],\n"
                "    'effort': os.environ['BURBOT_EFFORT'],\n"
                "    'fast_mode': os.environ['BURBOT_FAST'] == 'true',\n"
                "    'prompt': prompt,\n"
                "    'assistant_text': proc.stdout,\n"
                "    'burbot_summary': burbot_summary,\n"
                "    'burbot_status': burbot_status,\n"
                "    'burbot_run_id': (\n"
                "        burbot_summary.get('run_id') if isinstance(burbot_summary, dict) else None\n"
                "    ),\n"
                "    'burbot_open_actions': (\n"
                "        burbot_summary.get('open_actions')\n"
                "        if isinstance(burbot_summary, dict)\n"
                "        else None\n"
                "    ),\n"
                "    'burbot_selected_artifact_action': artifact_action,\n"
                "    'burbot_debug_dir': str(debug_dir) if debug_dir.exists() else None,\n"
                "    'error': None if agent_success else proc.stdout[-4000:],\n"
                "}\n"
                "Path('/logs/agent/result.json').write_text(json.dumps(result, indent=2) + '\\n')\n"
                "trajectory = {\n"
                "    'type': 'burbot_run',\n"
                "    'command': cmd,\n"
                "    'return_code': proc.returncode,\n"
                "    'process_return_code': proc.returncode,\n"
                "    'burbot_summary': burbot_summary,\n"
                "    'burbot_status': result['burbot_status'],\n"
                "    'burbot_run_id': result['burbot_run_id'],\n"
                "    'burbot_open_actions': result['burbot_open_actions'],\n"
                "    'burbot_selected_artifact_action': artifact_action,\n"
                "    'stdout': proc.stdout,\n"
                "    'burbot_debug_dir': result['burbot_debug_dir'],\n"
                "}\n"
                "Path('/logs/agent/trajectory.json').write_text(json.dumps(trajectory, indent=2) + '\\n')\n"
                "raise SystemExit(0 if agent_success else (proc.returncode or 1))\n"
                "PY\n"
                "cat /logs/agent/burbot.txt"
            ),
            env=env,
        )
