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
  // Add entries as backend tools come online. Empty by design today —
  // every tool falls through to the dev/prod fallback in ToolCallPill.
};

export function lookupToolLabel(toolId: string): ToolLabel | null {
  return TOOL_LABELS[toolId] ?? null;
}
