/**
 * Planned tests for Task 3 (Thinking display):
 *
 * - thinking-delta accumulates into a single block per turnId
 * - dev vs prod toggle: SHOW_RAW_AGENT_ACTIVITY gates the raw text branch
 * - multiple deltas append without re-rendering as separate blocks
 * - turn-complete flips block.pending from true to false
 * - turn-error also clears pending
 * - thinking block survives transcript reload mid-flow (or documents the
 *   intentional skip per plan's "Out of Scope" table)
 */
import { test } from "@playwright/test";

import { bootOnboarded } from "../support/bootHelpers";
import { emitTurnStart, emitThinkingDelta, emitTurnComplete } from "../support/chatEmit";
import { locateThinkingBlock } from "../support/chatLocators";

void bootOnboarded;
void emitTurnStart;
void emitThinkingDelta;
void emitTurnComplete;
void locateThinkingBlock;

test.fixme("thinking: planned suite — see file header for cases", async () => {});
