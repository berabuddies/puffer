/**
 * Planned tests for WebSocket reconnection. Each parameterizes over the
 * event types: text-delta, thinking-delta, tool-request, tool-invocation,
 * user-question-request.
 *
 * - WS disconnect mid-turn + reconnect: in-flight turn survives, deltas
 *   emitted during the gap don't duplicate when replayed
 * - No event duplication after reconnect: emitting the same event twice
 *   does not double-up the bubble
 * - Replay-on-reconnect: server-side replay of missed events lands cleanly
 *   into the existing bubble (matched by turnId / callId / requestId)
 */
import { test } from "@playwright/test";

import { bootOnboarded } from "../support/bootHelpers";
import { emitTextDelta, emitTurnStart, emitTurnComplete } from "../support/chatEmit";

void bootOnboarded;
void emitTextDelta;
void emitTurnStart;
void emitTurnComplete;

test.fixme("reconnection: planned suite — see file header for cases", async () => {});
