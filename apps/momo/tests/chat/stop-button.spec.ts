/**
 * Planned tests for Task 2 (Stop button):
 *
 * - Stop disabled until turn-start lands a turnId (v1 ref :1253)
 * - Stop swaps in for Send when running becomes true
 * - Click fires cancel_turn with the active turnId
 * - Stop hides on turn-complete, send button returns
 * - Cancel-RPC failure rolls back UI state (Stop stays visible / toast surfaces)
 * - Cross-session: stop in session A does not cancel a running turn in session B
 */
import { test } from "@playwright/test";

// Imports kept so the harness wiring lights up at type-check time.
import { bootOnboarded } from "../support/bootHelpers";
import { emitTurnStart } from "../support/chatEmit";
import { composerExpectState } from "../support/composerHelpers";

void bootOnboarded;
void emitTurnStart;
void composerExpectState;

test.fixme("stop-button: planned suite — see file header for cases", async () => {});
