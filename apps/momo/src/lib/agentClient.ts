/**
 * Sidebar session DTO types.
 *
 * The agent turn loop and session RPCs now go through the puffer **daemon**
 * (`lib/agent/daemonChat.ts` + `lib/agent/agentChat.svelte.ts`); the legacy
 * 1431 chat wrappers that used to live here were removed when momo's chat
 * surface migrated to the daemon. What remains is the `SessionListItem`
 * shape the sidebar (`sessionStore` / `Sidebar.svelte`) renders. It mirrors
 * the daemon's `SessionListItemDto` (camelCase via serde `rename_all`).
 */

export interface SessionListItem {
  sessionId: string;
  displayName: string | null;
  generatedTitle: string | null;
  title: string;
  cwd: string;
  folderPath: string;
  updatedAtMs: number;
  createdAtMs: number;
  eventCount: number;
  activityStatus: string;
  slug: string | null;
  tags: string[];
  note: string | null;
  parentSessionId: string | null;
  providerId: string | null;
  modelId: string | null;
}
