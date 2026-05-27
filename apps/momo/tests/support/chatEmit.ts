import type { FakeDaemon } from "./fakeDaemon";

/**
 * Chat-event primitives. Thin wrappers around `daemon.emit(channel, payload)`
 * that fix the channel naming (`session:<id>:event`) and the event type
 * field, so test bodies stay close to the wire without re-deriving the
 * channel string every time.
 *
 * Use primitives in race / lifecycle / cross-session tests where the *gaps*
 * between events matter. Use `emitTurnLifecycle` only for happy-path sugar.
 *
 * Channel naming is verified against `apps/momo/src/lib/agentClient.ts`
 * (line 176: `session:${sessionId}:event`).
 */

function channel(sessionId: string): string {
  return `session:${sessionId}:event`;
}

export interface TurnStartArgs {
  sessionId: string;
  turnId: string;
}
export function emitTurnStart(daemon: FakeDaemon, args: TurnStartArgs): void {
  daemon.emit(channel(args.sessionId), { type: "turn-start", turnId: args.turnId });
}

export interface TextDeltaArgs {
  sessionId: string;
  turnId: string;
  delta: string;
}
export function emitTextDelta(daemon: FakeDaemon, args: TextDeltaArgs): void {
  daemon.emit(channel(args.sessionId), {
    type: "text-delta",
    turnId: args.turnId,
    delta: args.delta
  });
}

export interface ThinkingDeltaArgs {
  sessionId: string;
  turnId: string;
  delta: string;
}
export function emitThinkingDelta(daemon: FakeDaemon, args: ThinkingDeltaArgs): void {
  daemon.emit(channel(args.sessionId), {
    type: "thinking-delta",
    turnId: args.turnId,
    delta: args.delta
  });
}

export interface TurnCompleteArgs {
  sessionId: string;
  turnId: string;
  assistantText?: string;
}
export function emitTurnComplete(daemon: FakeDaemon, args: TurnCompleteArgs): void {
  const payload: Record<string, unknown> = { type: "turn-complete", turnId: args.turnId };
  if (args.assistantText !== undefined) payload.assistantText = args.assistantText;
  daemon.emit(channel(args.sessionId), payload);
}

export interface TurnErrorArgs {
  sessionId: string;
  turnId: string;
  error: string;
}
export function emitTurnError(daemon: FakeDaemon, args: TurnErrorArgs): void {
  daemon.emit(channel(args.sessionId), {
    type: "turn-error",
    turnId: args.turnId,
    error: args.error
  });
}

export interface ToolRequestArgs {
  sessionId: string;
  turnId: string;
  callId: string;
  toolId: string;
  input?: unknown;
}
export function emitToolRequest(daemon: FakeDaemon, args: ToolRequestArgs): void {
  daemon.emit(channel(args.sessionId), {
    type: "tool-calls-requested",
    turnId: args.turnId,
    requests: [{ callId: args.callId, toolId: args.toolId, input: args.input ?? {} }]
  });
}

export interface ToolInvocationArgs {
  sessionId: string;
  turnId: string;
  callId: string;
  toolId: string;
  input?: unknown;
  output?: unknown;
  success: boolean;
}
export function emitToolInvocation(daemon: FakeDaemon, args: ToolInvocationArgs): void {
  daemon.emit(channel(args.sessionId), {
    type: "tool-invocations",
    turnId: args.turnId,
    invocations: [
      {
        callId: args.callId,
        toolId: args.toolId,
        input: args.input ?? {},
        output: args.output,
        success: args.success
      }
    ]
  });
}

export interface QuestionArgs {
  sessionId: string;
  turnId: string;
  requestId: string;
  questions: Array<{
    question: string;
    header?: string;
    options: Array<{ label: string; description?: string; preview?: string | null }>;
    multiSelect?: boolean;
  }>;
}
export function emitQuestion(daemon: FakeDaemon, args: QuestionArgs): void {
  daemon.emit(channel(args.sessionId), {
    type: "user-question-request",
    turnId: args.turnId,
    requestId: args.requestId,
    questions: args.questions
  });
}

/**
 * Convenience for happy-path tests only — fires turn-start, all deltas in
 * order, then turn-complete. Do NOT use in race / lifecycle / cancel /
 * reconnection tests; those need to control the gaps between events.
 */
export interface TurnLifecycleArgs {
  sessionId: string;
  turnId: string;
  deltas: string[];
  assistantText?: string;
}
export function emitTurnLifecycle(daemon: FakeDaemon, args: TurnLifecycleArgs): void {
  emitTurnStart(daemon, { sessionId: args.sessionId, turnId: args.turnId });
  for (const delta of args.deltas) {
    emitTextDelta(daemon, { sessionId: args.sessionId, turnId: args.turnId, delta });
  }
  emitTurnComplete(daemon, {
    sessionId: args.sessionId,
    turnId: args.turnId,
    assistantText: args.assistantText
  });
}
