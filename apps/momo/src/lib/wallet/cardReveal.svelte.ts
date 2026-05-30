/**
 * Production wiring for cardReveal — adapts the real `walletClient` (REST or
 * mock, a Svelte-runes module) into the runes-free `CardSource` interface.
 *
 * Kept separate from cardReveal.ts so that file stays node-testable: importing
 * walletClient.svelte pulls in the `$state` runtime, which a plain-node test
 * runner can't evaluate. Call sites that actually reveal a card import
 * `walletCardSource()` from here; the pure logic + its tests import only
 * cardReveal.ts.
 */

import { walletClient } from "../walletClient.svelte";
import type { CardSource } from "./cardReveal";

/** The live wallet client as a CardSource. Returns the shared singleton — no
 *  per-call allocation needed since walletClient is itself a singleton. */
export function walletCardSource(): CardSource {
  return walletClient;
}
