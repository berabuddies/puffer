/**
 * Planned tests for Composer input edges:
 *
 * - IME composition active → Enter does NOT submit (v1 ref :554)
 * - Enter while textarea disabled is a no-op
 * - Empty submit is a no-op (no create_session / run_agent_turn fires)
 * - Draft persists across session switch (v1 ref :1877)
 * - Very-long paste does not crash the composer
 * - Paste with newlines preserves them in the draft
 */
import { test } from "@playwright/test";

import { bootOnboarded } from "../support/bootHelpers";
import {
  composerType,
  composerSubmit,
  composerIME,
  composerExpectState,
  composerExpectDraft
} from "../support/composerHelpers";

void bootOnboarded;
void composerType;
void composerSubmit;
void composerIME;
void composerExpectState;
void composerExpectDraft;

test.fixme("composer: planned suite — see file header for cases", async () => {});
