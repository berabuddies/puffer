/**
 * Planned tests for cross-session isolation. For each event type, prove
 * emissions on session A don't render in session B's open view:
 *
 * - text-delta for session A doesn't surface in session B's thread (v1 :19, :99)
 * - composer draft per session — switch + return preserves the draft (v1 :410)
 * - background-running indicator: session B's open view doesn't show the
 *   running spinner for session A (v1 :479)
 * - tool / thinking / question events parameterize over the same harness
 */
import { test } from "@playwright/test";

import { bootOnboarded } from "../support/bootHelpers";
import { emitTextDelta, emitToolRequest, emitQuestion } from "../support/chatEmit";
import { locateUserBubble, locateAssistantBubble } from "../support/chatLocators";

void bootOnboarded;
void emitTextDelta;
void emitToolRequest;
void emitQuestion;
void locateUserBubble;
void locateAssistantBubble;

test.fixme("session-isolation: planned suite — see file header for cases", async () => {});
