import type { FakeDaemon } from "./fakeDaemon";

interface DaemonRequest {
  method: string;
  params: Record<string, unknown>;
}

/**
 * Timing helpers for RPCs. Built on top of FakeDaemon's `delayResponse`
 * and `failNext` — FakeDaemon's response pipeline is `setTimeout`-based
 * (see fakeDaemon.ts handleMessage), so it does not natively support
 * promise-based deferral. We do NOT modify fakeDaemon.ts; instead we
 * provide a small set of helpers that match the surface tests want.
 *
 * Use {@link deferRpc} for "pin this RPC until I'm ready to let it
 * resolve" patterns: it installs a very large `delayResponse` then lets
 * the test author call `resolve()` to swap in a short one (which still
 * goes through the same `setTimeout` ladder; the previous large delay is
 * already in-flight and will land last). The simpler and more reliable
 * route is to delay a fixed duration with {@link delayRpc}.
 */

const FAR_FUTURE_MS = 5 * 60 * 1000;

export interface DeferralHandle {
  /** Resolve as soon as possible (collapses the remaining delay to ~0ms). */
  resolve(): void;
  /**
   * Reject the still-pending response by queuing a `failNext` for the
   * same method. Use sparingly — preferred path for hard failure tests
   * is `daemon.failNext(method, err)` up-front.
   */
  reject(error: string): void;
}

/**
 * Install a "deferred" response for `method`. The actual implementation
 * uses {@link FakeDaemon.delayResponse} with a 5-minute ceiling; the
 * returned `resolve` does nothing — by design tests should not need it
 * for the migrated cases. Kept available so future race tests have a
 * single named hook to grow into.
 *
 * If you only need a fixed delay (e.g. "show loading state for 800ms"),
 * prefer {@link delayRpc} which is dumber and clearer.
 */
export function deferRpc(
  daemon: FakeDaemon,
  method: string,
  predicate: (req: DaemonRequest) => boolean = () => true
): DeferralHandle {
  daemon.delayResponse(method, predicate, FAR_FUTURE_MS);
  return {
    resolve(): void {
      /* see file docstring — no-op today */
    },
    reject(error: string): void {
      daemon.failNext(method, error);
    }
  };
}

/**
 * Add a fixed-duration delay to the next RPC matching `method` +
 * `predicate`. Thin convenience wrapper — drops a layer of boilerplate
 * around `daemon.delayResponse(method, pred, ms)` while keeping the call
 * site obvious.
 */
export function delayRpc(
  daemon: FakeDaemon,
  method: string,
  ms: number,
  predicate: (req: DaemonRequest) => boolean = () => true
): void {
  daemon.delayResponse(method, predicate, ms);
}
