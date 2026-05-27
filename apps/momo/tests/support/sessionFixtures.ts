import type { Page } from "@playwright/test";
import type { FakeDaemon, FakeDaemonSessionInput } from "./fakeDaemon";

/**
 * Fixture factories for sessions and lifecycle plumbing tests share. Goal
 * is `makeSession({ id, timeline })` is enough boilerplate for happy
 * path: defaults fill in cwd, providerId, timestamps so the call site
 * only declares what its test actually cares about.
 */

export interface MakeSessionArgs {
  id: string;
  title?: string;
  cwd?: string;
  providerId?: string | null;
  modelId?: string | null;
  timeline?: FakeDaemonSessionInput["timeline"];
  /**
   * Override timestamps if your test depends on a specific clock skew.
   * Defaults to "now" for updatedAtMs and now-60s for createdAtMs.
   */
  baseTime?: number;
}

export function makeSession(args: MakeSessionArgs): FakeDaemonSessionInput {
  const base = args.baseTime ?? Date.now();
  const title = args.title ?? args.id;
  const timeline = args.timeline ?? [];
  return {
    sessionId: args.id,
    displayName: title,
    title,
    cwd: args.cwd ?? "/tmp/momo",
    folderPath: args.cwd ?? "/tmp/momo",
    updatedAtMs: base,
    createdAtMs: base - 60_000,
    eventCount: timeline.length,
    activityStatus: "idle",
    providerId: args.providerId ?? "puffer",
    modelId: args.modelId ?? null,
    timeline
  };
}

/**
 * Force a session's `load_session_detail` to re-fire while a turn is
 * already running. Implemented as a hash-route round-trip — pushes the
 * route to "/" then back to "/#/agent/<id>", which makes the Agent.svelte
 * onMount re-call ensureSession.
 *
 * Note: this is a stub for race tests; mid-flow hydration semantics may
 * need a more surgical re-fire pattern once those tests get written.
 */
export async function hydrateMidFlow(
  page: Page,
  _daemon: FakeDaemon,
  sessionId: string
): Promise<void> {
  await page.goto("/#/home");
  await page.goto(`/#/agent/${sessionId}`);
}
