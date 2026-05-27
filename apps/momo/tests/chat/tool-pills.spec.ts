/**
 * Planned tests for Task 4 (Tool call pills):
 *
 * - tool-calls-requested renders a pending pill keyed by callId
 * - tool-invocations with success: true flips the pill to "success"
 * - failed pill goes red (data-status="failed")
 * - callId reuse across turns: same callId, new turn → new pill (v1 :1112)
 * - tool invocation without prior request synthesizes a terminal-state pill
 * - unmapped toolId falls back to "I'm working on it now..." in prod build
 *   and "Calling: <toolId>" in dev build (debugFlags toggle)
 */
import { test } from "@playwright/test";

import { bootOnboarded } from "../support/bootHelpers";
import {
  emitTurnStart,
  emitToolRequest,
  emitToolInvocation,
  emitTurnComplete
} from "../support/chatEmit";
import { locateToolPill } from "../support/chatLocators";

void bootOnboarded;
void emitTurnStart;
void emitToolRequest;
void emitToolInvocation;
void emitTurnComplete;
void locateToolPill;

test.fixme("tool-pills: planned suite — see file header for cases", async () => {});
