import type { FakeDaemon, DeferralHandle } from "./fakeDaemon";

interface DaemonRequest {
  method: string;
  params: Record<string, unknown>;
}

export type { DeferralHandle };

/**
 * Pin an RPC open until the test code says go. Returns a handle whose
 * `resolve()` ships the default success response (or a custom value),
 * and `reject(err)` ships a failure frame. Until either is called, the
 * webview sees the RPC as outstanding — perfect for race tests like:
 *
 *   const pending = deferRpc(daemon, "run_agent_turn");
 *   // assert composer Stop button is disabled while pending...
 *   pending.resolve();
 *   // assert composer flips back to Send...
 *
 * Backed by `FakeDaemon.deferRpc` which intercepts the matching request
 * before the normal dispatcher runs.
 */
export function deferRpc(
  daemon: FakeDaemon,
  method: string,
  predicate: (req: DaemonRequest) => boolean = () => true
): DeferralHandle {
  return daemon.deferRpc(method, predicate);
}

/**
 * Add a fixed-duration delay to the next RPC matching `method` +
 * `predicate`. Thin convenience wrapper — drops a layer of boilerplate
 * around `daemon.delayResponse(method, pred, ms)` while keeping the call
 * site obvious. Use this when the test just needs to observe a loading
 * state for a known duration; reach for {@link deferRpc} when the test
 * needs to interact with the UI mid-flight.
 */
export function delayRpc(
  daemon: FakeDaemon,
  method: string,
  ms: number,
  predicate: (req: DaemonRequest) => boolean = () => true
): void {
  daemon.delayResponse(method, predicate, ms);
}
