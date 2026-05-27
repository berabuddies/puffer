import type { IconName } from "../data/types";

/**
 * Friendly label + icon for known backend tool ids.
 *
 * Keys are the `toolId` strings emitted in `tool-calls-requested` /
 * `tool-invocations` events (see `apps/momo/src-tauri/src/turn.rs`
 * analog). UNMAPPED tools fall back to debug-flag-controlled text:
 *   - dev build: `Calling: <toolId>`
 *   - prod build: `"I'm working on it now..."`
 *
 * Populate lazily as the puffer agent adds tools we want to surface
 * with proper consumer-friendly copy.
 */
export interface ToolLabel {
  icon: IconName;
  label: string;
}

export const TOOL_LABELS: Record<string, ToolLabel> = {
  // Puffer agent tools surfaced by `puffer non-interactive --json-events`.
  // ToolIds here match `tool_id` on the ndjson events from
  // crates/puffer-core/runtime/claude_tools (see also resources/tools/*.yaml
  // and resources/internal_tools/*.yaml). Keep copy short — these labels
  // render inside a 36px pill on the assistant bubble.
  Bash: { icon: "bot", label: "Working in the shell..." },
  Skill: { icon: "bot", label: "Reading a skill..." },
  Read: { icon: "bot", label: "Reading a file..." },
  Write: { icon: "bot", label: "Writing a file..." },
  Edit: { icon: "bot", label: "Editing a file..." },
  Grep: { icon: "bot", label: "Searching files..." },
  Glob: { icon: "bot", label: "Finding files..." },
  Telegram: { icon: "message-circle", label: "Talking to Telegram..." },
  Email: { icon: "mail", label: "Talking to Email..." },
  Slack: { icon: "message-circle", label: "Talking to Slack..." },
  Lark: { icon: "message-circle", label: "Talking to Lark..." },
  ConnectorAct: { icon: "bot", label: "Running connector action..." },
  SendMessage: { icon: "message-circle", label: "Sending a message..." },
  TaskCreate: { icon: "check", label: "Tracking a task..." },
  TaskUpdate: { icon: "check", label: "Updating a task..." },
};

/**
 * Lightly inspects the tool input so a generic tool like `Bash` can
 * masquerade as something more meaningful (e.g. a Telegram CLI call
 * looks like `Bash`, but the user-meaningful action is Telegram). Falls
 * back to the static `TOOL_LABELS` map when nothing more specific
 * applies.
 */
export function lookupToolLabel(toolId: string, input?: unknown): ToolLabel | null {
  if (toolId === "Bash" && isBashTelegramCall(input)) {
    return { icon: "message-circle", label: "Talking to Telegram..." };
  }
  if (toolId === "Bash" && isBashEmailCall(input)) {
    return { icon: "mail", label: "Talking to Email..." };
  }
  return TOOL_LABELS[toolId] ?? null;
}

function bashCommandText(input: unknown): string | null {
  if (input === null || typeof input !== "object") return null;
  const command = (input as { command?: unknown }).command;
  return typeof command === "string" ? command.trim() : null;
}

function isBashTelegramCall(input: unknown): boolean {
  const command = bashCommandText(input);
  if (!command) return false;
  return /^telegram(\s|$)/.test(command);
}

function isBashEmailCall(input: unknown): boolean {
  const command = bashCommandText(input);
  if (!command) return false;
  return /^email(\s|$)/.test(command);
}
