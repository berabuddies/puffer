/**
 * Pure normalizers, ported from `apps/puffer-desktop/src/lib/api/desktop.ts`.
 *
 * These turn the daemon's wire shape (`Backend*`) into the UI types in
 * `./types`. Deliberately *pure* — no Tauri `invoke`, no `mockData`, no
 * daemon-reachability fallback. Those branches in desktop.ts mask daemon
 * errors with mock data; momo's chat must surface daemon failures, so the
 * thin RPC layer (`daemonChat.ts`) calls these on the raw daemon result and
 * lets a failed request throw.
 */
import type {
  AgentActivityStatus,
  AskUserQuestionItem,
  DiffSnapshot,
  MessageActor,
  PullRequest,
  RepoStatus,
  SessionDetail,
  SessionListItem,
  TimelineItem
} from "./types";

type BackendSessionListItem = {
  sessionId: string;
  displayName: string | null;
  generatedTitle?: string | null;
  title: string;
  cwd: string;
  folderPath: string;
  updatedAtMs: number;
  createdAtMs: number;
  eventCount: number;
  slug: string | null;
  tags: string[];
  note: string | null;
  parentSessionId: string | null;
  providerId?: string | null;
  modelId?: string | null;
  activityStatus?: string | null;
};

type BackendDiff = {
  id: string;
  source: string;
  commandLabel: string;
  statusText: string;
  unstagedDiffstat: string;
  stagedDiffstat: string;
  patch: string;
  patchExcerpt: string;
};

type BackendPullRequest = {
  number: number;
  title: string;
  url: string;
  state: string;
  isDraft: boolean;
  mergeStateStatus: string | null;
  headRefName: string | null;
  baseRefName: string | null;
};

type BackendRepoStatus = {
  sessionId: string;
  cwd: string;
  repoRoot: string | null;
  branch: string | null;
  headSha: string | null;
  isClean: boolean;
  statusLines: string[];
  hasGh: boolean;
  ghAuthenticated: boolean;
  canCreatePullRequest: boolean;
  canMergePullRequest: boolean;
  createPullRequestReason: string | null;
  mergePullRequestReason: string | null;
  openPullRequest: BackendPullRequest | null;
  warnings: string[];
};

type BackendActorFields = {
  actor?: MessageActor | null;
  subject?: MessageActor | null;
};

type BackendTimelineItem =
  | ({
      kind: "user_message";
      id: string;
      text: string;
      createdAtMs?: number | null;
    } & BackendActorFields)
  | ({
      kind: "assistant_message";
      id: string;
      text: string;
      createdAtMs?: number | null;
    } & BackendActorFields)
  | ({
      kind: "system_message";
      id: string;
      text: string;
      createdAtMs?: number | null;
    } & BackendActorFields)
  | ({
      kind: "command";
      id: string;
      commandName: string;
      commandArgs: string;
      createdAtMs?: number | null;
    } & BackendActorFields)
  | ({
      kind: "tool_call";
      id: string;
      createdAtMs?: number | null;
      toolId?: string;
      tool_id?: string;
      status: string;
      summary: string | null;
      inputText?: string;
      input_text?: string;
      inputJson?: Record<string, unknown> | null;
      input_json?: Record<string, unknown> | null;
      outputText?: string;
      output_text?: string;
      metadata?: unknown;
    } & BackendActorFields)
  | ({
      kind: "permission_dialog";
      id: string;
      createdAtMs?: number | null;
      toolId: string;
      state: string;
      summary: string | null;
      reason: string;
      inputText: string | null;
    } & BackendActorFields)
  | { kind: "diff_snapshot"; id: string; snapshot: BackendDiff; createdAtMs?: number | null };

type BackendAgentDiffFile = {
  path: string;
  latestKind: string;
  editCount: number;
  latestSummary: string;
};

type BackendAgentDiffEntry = {
  callId: string;
  toolId: string;
  kind: string;
  path: string;
  success: boolean;
  summary: string;
};

type BackendAgentDiff = {
  files: BackendAgentDiffFile[];
  entries: BackendAgentDiffEntry[];
};

type BackendDivergenceReport = {
  agentOnly: string[];
  gitOnly: string[];
  agentTotal: number;
  gitTotal: number;
};

type BackendSessionDetail = BackendSessionListItem & {
  timeline: BackendTimelineItem[];
  latestDiff: BackendDiff | null;
  diffHistory: BackendDiff[];
  repoStatus: BackendRepoStatus;
  agentDiff: BackendAgentDiff;
  divergence: BackendDivergenceReport;
};

function preview(text: string, maxLength = 160): string {
  return text.length > maxLength ? `${text.slice(0, maxLength).trimEnd()}...` : text;
}

function normalizePullRequest(value: BackendPullRequest | null): PullRequest | null {
  if (!value) {
    return null;
  }
  return {
    number: value.number,
    title: value.title,
    url: value.url,
    state: value.state,
    isDraft: value.isDraft,
    mergeStateStatus: value.mergeStateStatus,
    headRefName: value.headRefName,
    baseRefName: value.baseRefName
  };
}

function normalizeRepoStatus(value: BackendRepoStatus): RepoStatus {
  return {
    sessionId: value.sessionId,
    cwd: value.cwd,
    isGitRepo: value.repoRoot !== null,
    repoRoot: value.repoRoot,
    branch: value.branch,
    headSha: value.headSha,
    isClean: value.isClean,
    hasUncommittedChanges: !value.isClean,
    statusLines: value.statusLines,
    ghAvailable: value.hasGh,
    ghAuthenticated: value.ghAuthenticated,
    canCreatePr: value.canCreatePullRequest,
    canMergePr: value.canMergePullRequest,
    createPrReason: value.createPullRequestReason,
    mergePrReason: value.mergePullRequestReason,
    pullRequest: normalizePullRequest(value.openPullRequest),
    warnings: value.warnings
  };
}

function normalizeDiff(value: BackendDiff): DiffSnapshot {
  return {
    id: value.id,
    source: value.source,
    title: value.commandLabel,
    command: value.commandLabel,
    status: value.statusText,
    unstagedDiffstat: value.unstagedDiffstat,
    stagedDiffstat: value.stagedDiffstat,
    patch: value.patch || value.patchExcerpt
  };
}

function asRecord(value: unknown): Record<string, unknown> | null {
  return typeof value === "object" && value !== null ? (value as Record<string, unknown>) : null;
}

function parseJsonObject(text: string): Record<string, unknown> | null {
  try {
    return asRecord(JSON.parse(text));
  } catch {
    return null;
  }
}

function normalizeAskUserQuestions(raw: unknown): AskUserQuestionItem[] {
  if (!Array.isArray(raw)) return [];
  return raw
    .map(asRecord)
    .filter((item): item is Record<string, unknown> => item !== null)
    .map((item) => ({
      question: typeof item.question === "string" ? item.question : "Question",
      header: typeof item.header === "string" ? item.header : "Question",
      type: item.type === "input" ? ("input" as const) : ("choice" as const),
      multiSelect: item.multiSelect === true,
      searchable: item.searchable === true,
      options: Array.isArray(item.options)
        ? item.options
            .map(asRecord)
            .filter((option): option is Record<string, unknown> => option !== null)
            .map((option) => ({
              label: typeof option.label === "string" ? option.label : "Option",
              description: typeof option.description === "string" ? option.description : "",
              preview: typeof option.preview === "string" ? option.preview : null
            }))
        : []
    }));
}

function normalizeQuestionAnswers(raw: unknown): Record<string, string | string[]> {
  const record = asRecord(raw);
  if (!record) return {};
  const answers: Record<string, string | string[]> = {};
  for (const [key, value] of Object.entries(record)) {
    if (typeof value === "string") {
      answers[key] = value;
    } else if (Array.isArray(value)) {
      const values = value.filter((item): item is string => typeof item === "string");
      if (values.length > 0) answers[key] = values;
    }
  }
  return answers;
}

function normalizeAskUserQuestionTool(
  value: Extract<BackendTimelineItem, { kind: "tool_call" }>,
  input: Record<string, unknown>,
  output: Record<string, unknown>
): TimelineItem {
  const questions = normalizeAskUserQuestions(output.questions ?? input.questions);
  const answers = normalizeQuestionAnswers(output.answers ?? input.answers);
  const pending = output.pending === true || Object.keys(answers).length === 0;
  const answerSummary = Object.entries(answers)
    .map(([question, answer]) => `${question}: ${Array.isArray(answer) ? answer.join(", ") : answer}`)
    .join("\n");
  return {
    id: value.id,
    kind: "question",
    createdAtMs: value.createdAtMs ?? null,
    title: pending ? "Question" : "Answered question",
    summary: answerSummary || questions.map((question) => question.question).join("\n"),
    body: "",
    meta: [],
    status: pending ? "pending" : "answered",
    actor: value.actor ?? null,
    questions,
    answers
  };
}

function normalizeActivityStatus(value: string | null | undefined): AgentActivityStatus {
  switch (value) {
    case "running":
    case "awaiting":
    case "review":
      return value;
    default:
      return "idle";
  }
}

function normalizeSessionListItem(value: BackendSessionListItem): SessionListItem {
  return {
    id: value.sessionId,
    displayName: value.displayName,
    generatedTitle: value.generatedTitle ?? null,
    title: value.title,
    cwd: value.cwd,
    folderPath: value.folderPath,
    updatedAtMs: value.updatedAtMs,
    createdAtMs: value.createdAtMs,
    eventCount: value.eventCount,
    activityStatus: normalizeActivityStatus(value.activityStatus),
    slug: value.slug,
    tags: value.tags,
    note: value.note,
    parentSessionId: value.parentSessionId,
    providerId: value.providerId ?? null,
    modelId: value.modelId ?? null
  };
}

export function normalizeTimelineItem(value: BackendTimelineItem): TimelineItem {
  switch (value.kind) {
    case "user_message":
      return {
        id: value.id,
        kind: "user",
        createdAtMs: value.createdAtMs ?? null,
        title: "User message",
        summary: preview(value.text),
        body: value.text,
        meta: [],
        actor: value.actor ?? null
      };
    case "assistant_message":
      return {
        id: value.id,
        kind: "assistant",
        createdAtMs: value.createdAtMs ?? null,
        title: "Assistant response",
        summary: preview(value.text),
        body: value.text,
        meta: [],
        actor: value.actor ?? null
      };
    case "system_message": {
      const systemText = value.text;
      const isVerifiedSkillGate = systemText.trim().startsWith("Verified Skill Gate");
      const verifiedGateFailed = /\bevent:\s*[^\n]*reject/i.test(systemText);
      return {
        id: value.id,
        kind: "system",
        createdAtMs: value.createdAtMs ?? null,
        title: isVerifiedSkillGate ? "Verified Skill Gate" : "System message",
        summary: preview(systemText),
        body: systemText,
        meta: isVerifiedSkillGate ? ["verified skill"] : [],
        status: isVerifiedSkillGate ? (verifiedGateFailed ? "error" : "success") : null,
        actor: value.actor ?? null
      };
    }
    case "command":
      return {
        id: value.id,
        kind: "command",
        createdAtMs: value.createdAtMs ?? null,
        title: `/${value.commandName}`,
        summary: preview(value.commandArgs || `/${value.commandName}`),
        body: [value.commandName, value.commandArgs].filter(Boolean).join(" "),
        meta: ["slash command"],
        actor: value.actor ?? null
      };
    case "tool_call": {
      const toolId = value.toolId ?? value.tool_id ?? "";
      const inputText = value.inputText ?? value.input_text ?? "";
      const inputJson = value.inputJson ?? value.input_json ?? null;
      const outputText = value.outputText ?? value.output_text ?? "";
      const input = inputJson ?? parseJsonObject(inputText) ?? {};
      const output = parseJsonObject(outputText) ?? {};
      if (
        toolId === "AskUserQuestion" ||
        Array.isArray(input.questions) ||
        Array.isArray(output.questions)
      ) {
        return normalizeAskUserQuestionTool(value, input, output);
      }
      const toolName = toolId || "Tool";
      return {
        id: value.id,
        kind: "tool",
        createdAtMs: value.createdAtMs ?? null,
        title: `Tool call: ${toolName}`,
        summary: value.summary ?? preview(outputText || inputText),
        body: outputText || "Tool call completed without textual output.",
        meta: [toolName, value.status],
        toolName,
        status: value.status,
        input: inputText,
        output: outputText,
        inputJson,
        metadata: value.metadata,
        actor: value.actor ?? null,
        subject: value.subject ?? null
      };
    }
    case "permission_dialog":
      return {
        id: value.id,
        kind: "permission",
        createdAtMs: value.createdAtMs ?? null,
        title: "Permission request",
        summary: value.summary ?? `${value.toolId} requires approval`,
        body: `Tool: ${value.toolId}\nReason: ${value.reason}`,
        meta: [value.state],
        toolName: value.toolId,
        status: value.state,
        actor: value.actor ?? null,
        permissionDialog: {
          state: value.state,
          reason: value.reason,
          summary: value.summary,
          inputText: value.inputText,
          toolName: value.toolId,
          choices: ["Approve once", "Always allow", "Deny"]
        },
        scopeLabel: "workspace",
        choices: ["Approve once", "Always allow", "Deny"]
      };
    case "diff_snapshot": {
      const diff = normalizeDiff(value.snapshot);
      return {
        id: value.id,
        kind: "diff",
        createdAtMs: value.createdAtMs ?? null,
        title: diff.title,
        summary: diff.status,
        body: diff.patch,
        meta: [diff.command],
        diff
      };
    }
  }
}

export function normalizeSessionDetail(value: BackendSessionDetail): SessionDetail {
  const session = normalizeSessionListItem(value);
  // Older daemons may not emit agentDiff/divergence yet — fall back to
  // empty defaults so the UI still renders a sensible (empty) diff tab
  // instead of throwing on undefined.
  const agentDiff = value.agentDiff ?? { files: [], entries: [] };
  const divergence = value.divergence ?? {
    agentOnly: [],
    gitOnly: [],
    agentTotal: 0,
    gitTotal: 0
  };
  return {
    session,
    timeline: value.timeline.map(normalizeTimelineItem),
    latestDiff: value.latestDiff ? normalizeDiff(value.latestDiff) : null,
    diffHistory: value.diffHistory.map(normalizeDiff),
    repoStatus: normalizeRepoStatus(value.repoStatus),
    agentDiff,
    divergence
  };
}
