/**
 * Planned tests for Task 5 (Interactive answer form):
 *
 * - user-question-request renders a form with the question heading
 * - single-select submit fires resolve_user_question with the chosen label
 * - double-submit guard: clicking submit twice fires the RPC once
 * - answered state lock: pre-clicked answers can't be changed after submit
 * - RPC failure rollback: answered=false, toast surfaces, can retry
 * - duplicate requestId is ignored (defensive de-dup)
 */
import { test } from "@playwright/test";

import { bootOnboarded } from "../support/bootHelpers";
import { emitTurnStart, emitQuestion, emitTurnComplete } from "../support/chatEmit";
import { locateQuestionForm } from "../support/chatLocators";

void bootOnboarded;
void emitTurnStart;
void emitQuestion;
void emitTurnComplete;
void locateQuestionForm;

test.fixme("answer-form: planned suite — see file header for cases", async () => {});
