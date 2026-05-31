import { invoke } from "@tauri-apps/api/core";
import type {
  AgentActivityStatus,
  AuthProviderStatus,
  AskUserQuestionItem,
  BrowserRenderer,
  DesktopPinState,
  DiffSnapshot,
  DraftProxyEndpoint,
  ExternalCredential,
  FolderGroup,
  ProviderSummary,
  ProxyTestResult,
  PullRequest,
  RemoteConnection,
  RemoteOperation,
  RepoActionResult,
  RepoStatus,
  SaveProxySettingsInput,
  SessionDetail,
  SessionGroupsPage,
  SessionListItem,
  SettingsSnapshot,
  MessageActor,
  OpenAIRealtimeClientSecret,
  OpenAIRealtimeClientSecretOptions,
  TimelineItem,
  WorkflowDefinition,
  WorkflowBindingCreateRequest,
  WorkflowMonitorHistoryMessage,
  WorkflowRun,
  WorkflowSnapshot
} from "../types";
import {
  mockCreatePrResult,
  mockFolders,
  mockMergePrResult,
  mockRepoStatus,
  mockSessionDetail,
  mockSessionDetailFor,
  mockSettingsSnapshot
} from "../mockData";

type BackendFolderGroup = {
  folderId: string;
  folderLabel: string;
  folderPath: string;
  sessionCount: number;
  sessions: BackendSessionListItem[];
  tags?: string[];
};

type BackendSessionGroupsPage = {
  groups: BackendFolderGroup[];
  offset: number;
  limit: number;
  returnedSessions: number;
  totalSessions: number;
  hasMore: boolean;
};

type BackendDesktopPinState = {
  pinnedAgentIds?: string[];
  pinnedWorkspacePaths?: string[];
};

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

type BackendRepoActionResult = {
  ok: boolean;
  action: string;
  message: string;
  repoStatus: BackendRepoStatus;
  pullRequest: BackendPullRequest | null;
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
  | {
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
    } & BackendActorFields
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

function remoteArgs(remote?: RemoteConnection): Record<string, unknown> {
  if (!remote || !remote.enabled || !remote.target.trim()) {
    return {};
  }
  return {
    remoteTarget: remote.target,
    remoteCwd: remote.cwd || null,
    remotePassword: remote.password || null
  };
}

function shouldInvokeRemote(remote?: RemoteConnection): boolean {
  return canInvokeTauri() && Boolean(remote?.enabled && remote.target.trim());
}

type BackendSettingsConfig = SettingsSnapshot["config"];
type BackendResourceCounts = SettingsSnapshot["resources"];
type BackendSettingsSessionSummary = SettingsSnapshot["sessions"];
type BackendAuthProviderStatus = AuthProviderStatus;
type BackendProviderSummary = ProviderSummary;
type BackendNetworkProxySettings = SettingsSnapshot["networkProxy"];

type BackendSettingsSnapshot = {
  workspaceRoot: string;
  workspaceConfigFile: string;
  userConfigFile: string;
  authStoreFile: string;
  builtinResourcesDir: string;
  config: BackendSettingsConfig;
  resources: BackendResourceCounts;
  sessions: BackendSettingsSessionSummary;
  auth: BackendAuthProviderStatus[];
  providers: BackendProviderSummary[];
  networkProxy: BackendNetworkProxySettings;
};

type BackendRemoteOperation = RemoteOperation;

/** Exposed to Svelte components so they can branch on whether the daemon
 *  is reachable. Tauri can spawn it automatically; browser mode needs a
 *  configured daemon WebSocket handshake. */
export function isDaemonReachable(): boolean {
  return canReachDaemon();
}

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
      type: item.type === "input" ? "input" as const : "choice" as const,
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
    .map(([question, answer]) =>
      `${question}: ${Array.isArray(answer) ? answer.join(", ") : answer}`
    )
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

function normalizeTimelineItem(value: BackendTimelineItem): TimelineItem {
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
    case "system_message":
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
    case "tool_call":
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

function normalizeSessionDetail(value: BackendSessionDetail): SessionDetail {
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

function normalizeFolderGroup(group: BackendFolderGroup): FolderGroup {
  return {
    id: group.folderId,
    label: group.folderLabel,
    path: group.folderPath,
    sessionCount: group.sessionCount,
    sessions: group.sessions.map(normalizeSessionListItem),
    tags: group.tags ?? []
  };
}

function normalizeSessionGroupsPage(page: BackendSessionGroupsPage): SessionGroupsPage {
  return {
    groups: page.groups.map(normalizeFolderGroup),
    offset: page.offset,
    limit: page.limit,
    returnedSessions: page.returnedSessions,
    totalSessions: page.totalSessions,
    hasMore: page.hasMore
  };
}

export async function listGroupedSessions(remote?: RemoteConnection): Promise<FolderGroup[]> {
  if (!canInvokeTauri()) {
    if (canReachDaemon()) return listGroupedSessionsFromDaemon();
    return mockFolders;
  }
  const response = await invoke<BackendFolderGroup[]>("list_grouped_sessions", remoteArgs(remote));
  return response.map(normalizeFolderGroup);
}

export async function loadSessionDetail(
  sessionId: string,
  remote?: RemoteConnection
): Promise<SessionDetail> {
  if (!canInvokeTauri()) {
    if (canReachDaemon()) return loadSessionDetailFromDaemon(sessionId);
    return mockSessionDetailFor(sessionId);
  }
  const response = await invoke<BackendSessionDetail>("load_session_detail", {
    sessionId,
    ...remoteArgs(remote)
  });
  return normalizeSessionDetail(response);
}

export async function refreshRepoStatus(
  sessionId: string,
  remote?: RemoteConnection
): Promise<RepoStatus> {
  if (!canInvokeTauri()) {
    if (canReachDaemon()) {
      const client = await ensureLocalDaemonClient();
      const response = await client.request<BackendRepoStatus>("refresh_repo_status", {
        sessionId
      });
      return normalizeRepoStatus(response);
    }
    return mockRepoStatus;
  }
  const response = await invoke<BackendRepoStatus>("refresh_repo_status", {
    sessionId,
    ...remoteArgs(remote)
  });
  return normalizeRepoStatus(response);
}

export async function createPullRequest(
  sessionId: string,
  title?: string,
  body?: string,
  remote?: RemoteConnection
): Promise<RepoActionResult> {
  if (!canInvokeTauri()) {
    return mockCreatePrResult();
  }
  const response = await invoke<BackendRepoActionResult>("create_pull_request", {
    sessionId,
    title: title ?? null,
    body: body ?? null,
    ...remoteArgs(remote)
  });
  return {
    ok: response.ok,
    action: response.action,
    message: response.message,
    repoStatus: normalizeRepoStatus(response.repoStatus),
    pullRequest: normalizePullRequest(response.pullRequest)
  };
}

export async function mergePullRequest(
  sessionId: string,
  pullRequestNumber?: number,
  mergeMethod?: string,
  remote?: RemoteConnection
): Promise<RepoActionResult> {
  if (!canInvokeTauri()) {
    return mockMergePrResult();
  }
  const response = await invoke<BackendRepoActionResult>("merge_pull_request", {
    sessionId,
    pullRequestNumber: pullRequestNumber ?? null,
    mergeMethod: mergeMethod ?? null,
    ...remoteArgs(remote)
  });
  return {
    ok: response.ok,
    action: response.action,
    message: response.message,
    repoStatus: normalizeRepoStatus(response.repoStatus),
    pullRequest: normalizePullRequest(response.pullRequest)
  };
}

export async function loadSettingsSnapshot(remote?: RemoteConnection): Promise<SettingsSnapshot> {
  if (shouldInvokeRemote(remote)) {
    return invoke<BackendSettingsSnapshot>("load_settings_snapshot", remoteArgs(remote));
  }
  if (canReachDaemon()) {
    const client = await ensureLocalDaemonClient();
    return client.request<BackendSettingsSnapshot>("load_settings_snapshot");
  }
  if (!canInvokeTauri()) {
    return mockSettingsSnapshot;
  }
  return invoke<BackendSettingsSnapshot>("load_settings_snapshot", remoteArgs(remote));
}

export async function loginWithOauth(
  providerId: string,
  remote?: RemoteConnection
): Promise<SettingsSnapshot> {
  if (shouldInvokeRemote(remote)) {
    return invoke<BackendSettingsSnapshot>("login_with_oauth", {
      providerId,
      ...remoteArgs(remote)
    });
  }
  if (canReachDaemon()) {
    const client = await ensureLocalDaemonClient();
    return client.request<BackendSettingsSnapshot>("login_with_oauth", {
      providerId
    });
  }
  if (!canInvokeTauri()) {
    return mockSettingsSnapshot;
  }
  return invoke<BackendSettingsSnapshot>("login_with_oauth", {
    providerId,
    ...remoteArgs(remote)
  });
}

export async function loginWithApiKey(
  providerId: string,
  apiKey: string,
  remote?: RemoteConnection
): Promise<SettingsSnapshot> {
  if (shouldInvokeRemote(remote)) {
    return invoke<BackendSettingsSnapshot>("login_with_api_key", {
      providerId,
      apiKey,
      ...remoteArgs(remote)
    });
  }
  if (canReachDaemon()) {
    const client = await ensureLocalDaemonClient();
    return client.request<BackendSettingsSnapshot>("login_with_api_key", {
      providerId,
      apiKey
    });
  }
  if (!canInvokeTauri()) {
    return mockSettingsSnapshot;
  }
  return invoke<BackendSettingsSnapshot>("login_with_api_key", {
    providerId,
    apiKey,
    ...remoteArgs(remote)
  });
}

/** Lists credentials Puffer can adopt without an interactive flow — typically
 *  whatever is already stored under `~/.claude` or `~/.codex`. The desktop
 *  shell surfaces these so the user does not have to paste an API key they
 *  already have on disk. */
export async function listExternalCredentials(): Promise<ExternalCredential[]> {
  if (canReachDaemon()) {
    const client = await ensureLocalDaemonClient();
    return client.request<ExternalCredential[]>("list_external_credentials");
  }
  if (!canInvokeTauri()) {
    return [];
  }
  return invoke<ExternalCredential[]>("list_external_credentials");
}

/** Adopts a credential discovered by `listExternalCredentials` for the given
 *  provider, then returns the refreshed settings snapshot. */
export async function importExternalCredential(
  providerId: string,
  source: "claude" | "codex"
): Promise<SettingsSnapshot> {
  if (canReachDaemon()) {
    const client = await ensureLocalDaemonClient();
    return client.request<BackendSettingsSnapshot>("import_external_credential", {
      providerId,
      source
    });
  }
  if (!canInvokeTauri()) {
    return mockSettingsSnapshot;
  }
  return invoke<BackendSettingsSnapshot>("import_external_credential", {
    providerId,
    source
  });
}

export async function logoutProvider(
  providerId: string,
  remote?: RemoteConnection
): Promise<SettingsSnapshot> {
  if (shouldInvokeRemote(remote)) {
    return invoke<BackendSettingsSnapshot>("logout_provider", {
      providerId,
      ...remoteArgs(remote)
    });
  }
  if (canReachDaemon()) {
    const client = await ensureLocalDaemonClient();
    return client.request<BackendSettingsSnapshot>("logout_provider", {
      providerId
    });
  }
  if (!canInvokeTauri()) {
    return mockSettingsSnapshot;
  }
  return invoke<BackendSettingsSnapshot>("logout_provider", {
    providerId,
    ...remoteArgs(remote)
  });
}

export async function saveProxySettings(
  input: SaveProxySettingsInput
): Promise<SettingsSnapshot> {
  const client = await ensureLocalDaemonClient();
  return client.request<BackendSettingsSnapshot>("save_proxy_settings", input);
}

export async function testProxy(input: {
  proxyId?: string;
  endpoint?: DraftProxyEndpoint;
}): Promise<ProxyTestResult> {
  const client = await ensureLocalDaemonClient();
  return client.request<ProxyTestResult>("test_proxy", input);
}

export async function runRemoteBash(
  remote: RemoteConnection,
  command: string
): Promise<RemoteOperation> {
  if (!canInvokeTauri()) {
    return { success: true, stdout: "mock remote bash\n", stderr: "" };
  }
  return invoke<BackendRemoteOperation>("run_remote_bash", {
    remoteTarget: remote.target,
    remoteCwd: remote.cwd || null,
    remotePassword: remote.password || null,
    command
  });
}

export async function readRemoteFile(
  remote: RemoteConnection,
  path: string
): Promise<RemoteOperation> {
  if (!canInvokeTauri()) {
    return { success: true, stdout: "mock remote file\n", stderr: "" };
  }
  return invoke<BackendRemoteOperation>("read_remote_file", {
    remoteTarget: remote.target,
    remoteCwd: remote.cwd || null,
    remotePassword: remote.password || null,
    path
  });
}

export async function writeRemoteFile(
  remote: RemoteConnection,
  path: string,
  contents: string
): Promise<RemoteOperation> {
  if (!canInvokeTauri()) {
    return { success: true, stdout: "", stderr: "" };
  }
  return invoke<BackendRemoteOperation>("write_remote_file", {
    remoteTarget: remote.target,
    remoteCwd: remote.cwd || null,
    remotePassword: remote.password || null,
    path,
    contentsBase64: btoa(unescape(encodeURIComponent(contents)))
  });
}

// ============================================================================
// Daemon-backed workspace + runtime. The daemon is authoritative: Puffer owns
// sessions, transcripts, and provider state. This module is a thin adapter.
// ============================================================================

import {
  canInvokeTauri,
  canReachDaemon,
  configuredBrowserRemoteDaemonHandshake,
  ensureLocalDaemonClient,
  switchDaemonClient
} from "./daemonClient";

/** A blank session created on the fly. The daemon places it in the given
 *  `cwd` (defaults to the daemon's boot workspace). Returns the session id
 *  so the UI can open an `AgentDetail` immediately. */
export async function createSession(
  cwd?: string,
  providerId?: string,
  modelId?: string,
  displayName = "New Session"
): Promise<{
  sessionId: string;
  cwd: string;
  createdAtMs: number;
  providerId?: string;
  modelId?: string;
}> {
  const client = await ensureLocalDaemonClient();
  return client.request(
    "create_session",
    Object.fromEntries(
      Object.entries({ cwd, providerId, modelId, displayName }).filter(([, value]) =>
        Boolean(value)
      )
    )
  );
}

/** The daemon's boot workspace — useful for showing "new agent in <path>" in
 *  the UI so the user isn't surprised where their session lands. */
export async function loadDefaultWorkspace(): Promise<{
  cwd: string;
  workspaceRoot: string;
}> {
  const client = await ensureLocalDaemonClient();
  return client.request("default_workspace");
}

/** Result of a completed git clone — fired via the `clone:<id>:done`
 *  channel and resolved through the `done` promise. */
export type GitCloneDone = {
  ok: boolean;
  dest: string;
  stdout: string;
  stderr: string;
  exitCode: number | null;
};

/** Handle returned from `cloneRepo`. The `cloneId` + `dest` land
 *  synchronously; `done` resolves once git exits. Subscribe to
 *  `onProgress(line)` to surface live stderr (git's `--progress` output)
 *  in the UI while the clone is still running. */
export type GitCloneHandle = {
  cloneId: string;
  dest: string;
  done: Promise<GitCloneDone>;
  onProgress(handler: (line: string) => void): () => void;
};

/** Clones a git repository into the daemon's filesystem and returns a
 *  handle that streams progress. Remote clones are a special case of
 *  "clone on the currently-connected daemon" — the caller connects to
 *  the SSH daemon first (via connectSshDaemon) and the clone lands on
 *  that remote machine.
 *
 *  The daemon RPC returns `{cloneId, dest}` IMMEDIATELY, before the
 *  clone finishes; progress arrives on `clone:<id>:progress` events and
 *  the final status on `clone:<id>:done`. */
export async function cloneRepo(
  url: string,
  dest: string,
  options: { depth?: number } = {}
): Promise<GitCloneHandle> {
  const client = await ensureLocalDaemonClient();
  const start = await client.request<{ cloneId: string; dest: string }>("git_clone", {
    url,
    dest,
    ...(options.depth != null ? { depth: options.depth } : {})
  });
  const progressChannel = `clone:${start.cloneId}:progress`;
  const doneChannel = `clone:${start.cloneId}:done`;
  const done = new Promise<GitCloneDone>((resolve) => {
    const offDone = client.on<{
      cloneId: string;
      ok: boolean;
      dest: string;
      stdout: string;
      stderr: string;
      exitCode: number | null;
    }>(doneChannel, (payload) => {
      offDone();
      resolve({
        ok: payload.ok,
        dest: payload.dest,
        stdout: payload.stdout,
        stderr: payload.stderr,
        exitCode: payload.exitCode
      });
    });
  });
  return {
    cloneId: start.cloneId,
    dest: start.dest,
    done,
    onProgress(handler: (line: string) => void) {
      return client.on<{ line: string; cloneId: string }>(progressChannel, (payload) => {
        if (payload && typeof payload.line === "string") handler(payload.line);
      });
    }
  };
}

/** Tears down the current local daemon and spawns a fresh one rooted at
 *  `cwd`, then swaps the shared DaemonClient to the new handshake.
 *  Returns the new handshake so callers can show "now in <workspaceRoot>"
 *  after the switch. Used by the WorkspacePicker. */
export async function restartLocalDaemon(cwd: string): Promise<{
  url: string;
  token: string;
  workspaceRoot: string;
  protocolVersion: string;
}> {
  if (!canInvokeTauri()) {
    throw new Error("Switching local workspace requires the Tauri desktop shell.");
  }
  const handshake = await invoke<{
    url: string;
    token: string;
    protocolVersion: string;
    workspaceRoot: string;
  }>("restart_local_daemon", { cwd });
  await switchDaemonClient(handshake);
  return handshake;
}

/** Starts a remote `puffer daemon` over SSH and makes the app's shared
 *  daemon client connect to it. Returns the handshake (now pointing at a
 *  local forwarded port) so the UI can show the remote workspace path. */
export async function connectSshDaemon(
  sshTarget: string,
  options: { remoteBinary?: string; remoteWorkspace?: string } = {}
): Promise<{ url: string; token: string; workspaceRoot: string; protocolVersion: string }> {
  if (!canInvokeTauri()) {
    const browserHandshake = configuredBrowserRemoteDaemonHandshake();
    if (!browserHandshake) {
      throw new Error("SSH remote daemon requires the Tauri desktop shell.");
    }
    await switchDaemonClient(browserHandshake);
    return browserHandshake;
  }
  const handshake = await invoke<{
    url: string;
    token: string;
    protocolVersion: string;
    workspaceRoot: string;
  }>("start_ssh_daemon", {
    sshTarget,
    remoteBinary: options.remoteBinary ?? null,
    remoteWorkspace: options.remoteWorkspace ?? null
  });
  await switchDaemonClient(handshake);
  return handshake;
}

/** Refresh the grouped-sessions view from the daemon. Used whenever the
 *  workspace board needs to re-read state after create/mutate. Falls back
 *  to mock folders in the pure-web preview (no daemon reachable) so the
 *  design screenshots + visual review keep working without a backend. */
export async function listGroupedSessionsFromDaemon(): Promise<FolderGroup[]> {
  try {
    const client = await ensureLocalDaemonClient();
    const raw = await client.request<BackendFolderGroup[]>("list_grouped_sessions");
    return raw.map(normalizeFolderGroup);
  } catch (_error) {
    if (!canReachDaemon()) return mockFolders;
    throw _error;
  }
}

export async function listGroupedSessionsPageFromDaemon(
  offset = 0,
  limit = 30
): Promise<SessionGroupsPage> {
  try {
    const client = await ensureLocalDaemonClient();
    const raw = await client.request<BackendSessionGroupsPage>("list_grouped_sessions_page", {
      offset,
      limit
    });
    return normalizeSessionGroupsPage(raw);
  } catch (_error) {
    if (!canReachDaemon()) {
      const entries = mockFolders.flatMap((folder) =>
        folder.sessions.map((session) => ({ folder, session }))
      );
      const pageEntries = entries.slice(offset, offset + limit);
      const groupsById = new Map<string, FolderGroup>();
      for (const { folder, session } of pageEntries) {
        const group = groupsById.get(folder.id) ?? {
          ...folder,
          sessions: [],
          sessionCount: 0
        };
        group.sessions.push(session);
        group.sessionCount = group.sessions.length;
        groupsById.set(folder.id, group);
      }
      const pageGroups = Array.from(groupsById.values());
      return {
        groups: pageGroups,
        offset,
        limit,
        returnedSessions: pageGroups.reduce((count, group) => count + group.sessions.length, 0),
        totalSessions: entries.length,
        hasMore: offset + limit < entries.length
      };
    }
    throw _error;
  }
}

function normalizeDesktopPinState(value: BackendDesktopPinState | null | undefined): DesktopPinState {
  return {
    pinnedAgentIds: Array.isArray(value?.pinnedAgentIds) ? value.pinnedAgentIds : [],
    pinnedWorkspacePaths: Array.isArray(value?.pinnedWorkspacePaths) ? value.pinnedWorkspacePaths : []
  };
}

export async function loadDesktopPins(): Promise<DesktopPinState> {
  try {
    const client = await ensureLocalDaemonClient();
    const raw = await client.request<BackendDesktopPinState>("load_desktop_pins");
    return normalizeDesktopPinState(raw);
  } catch (_error) {
    if (!canReachDaemon()) return { pinnedAgentIds: [], pinnedWorkspacePaths: [] };
    throw _error;
  }
}

export async function setDesktopPin(
  kind: "agent" | "workspace",
  id: string,
  pinned: boolean
): Promise<DesktopPinState> {
  const client = await ensureLocalDaemonClient();
  const raw = await client.request<BackendDesktopPinState>("set_desktop_pin", {
    kind,
    id,
    pinned
  });
  return normalizeDesktopPinState(raw);
}

/** Load one session's detail (transcript + latest diff + repo state) via
 *  the daemon. Falls back to mock detail in web preview mode. */
export async function loadSessionDetailFromDaemon(
  sessionId: string
): Promise<SessionDetail> {
  try {
    const client = await ensureLocalDaemonClient();
    const raw = await client.request<BackendSessionDetail>("load_session_detail", {
      sessionId
    });
    return normalizeSessionDetail(raw);
  } catch (_error) {
    if (!canReachDaemon()) return mockSessionDetailFor(sessionId) ?? mockSessionDetail;
    throw _error;
  }
}

/** Sets or clears the user-edited title for one session. */
export async function renameSession(sessionId: string, title: string): Promise<SessionDetail> {
  const client = await ensureLocalDaemonClient();
  const raw = await client.request<BackendSessionDetail>("rename_session", {
    sessionId,
    title
  });
  return normalizeSessionDetail(raw);
}

/** Permanently deletes one session and all of its sidecar files. */
export async function deleteSession(sessionId: string): Promise<void> {
  const client = await ensureLocalDaemonClient();
  await client.request<{ ok: boolean }>("delete_session", { sessionId });
}

/** Replaces the tag list on one session. Tags are trimmed, deduped, sorted. */
export async function setSessionTags(sessionId: string, tags: string[]): Promise<SessionDetail> {
  const client = await ensureLocalDaemonClient();
  const raw = await client.request<BackendSessionDetail>("set_session_tags", {
    sessionId,
    tags
  });
  return normalizeSessionDetail(raw);
}

/** Permanently deletes every session under `folderPath` and removes the
 *  project's metadata entry. Use with strong UI confirmation. */
export async function deleteProject(folderPath: string): Promise<{ removedSessions: number }> {
  const client = await ensureLocalDaemonClient();
  const result = await client.request<{ ok: boolean; removedSessions: number }>("delete_project", {
    folderPath
  });
  return { removedSessions: result.removedSessions };
}

/** Replaces the tag list on one project (folder). */
export async function setProjectTags(folderPath: string, tags: string[]): Promise<string[]> {
  const client = await ensureLocalDaemonClient();
  const result = await client.request<{ ok: boolean; tags: string[] }>("set_project_tags", {
    folderPath,
    tags
  });
  return result.tags;
}

/** Load registered workflows and recent runs from the daemon. */
export async function loadWorkflowSnapshot(): Promise<WorkflowSnapshot> {
  const client = await ensureLocalDaemonClient();
  return client.request<WorkflowSnapshot>("workflow_list");
}

/** Persist one workflow definition through the daemon and return the refreshed snapshot. */
export async function saveWorkflow(workflow: WorkflowDefinition): Promise<WorkflowSnapshot> {
  const client = await ensureLocalDaemonClient();
  return client.request<WorkflowSnapshot>("workflow_save", { workflow });
}

/** Create or update one connection-triggered workflow binding. */
export async function createWorkflowBinding(binding: WorkflowBindingCreateRequest): Promise<WorkflowSnapshot> {
  const client = await ensureLocalDaemonClient();
  return client.request<WorkflowSnapshot>("workflow_binding_create", binding);
}

/** Create or resume a connector monitor and return the refreshed workflow snapshot. */
export async function createMonitor(
  connectionSlug: string,
  model?: string | null
): Promise<WorkflowSnapshot> {
  const client = await ensureLocalDaemonClient();
  const params: { connection_slug: string; model?: string | null } = {
    connection_slug: connectionSlug
  };
  if (model !== undefined) {
    params.model = model?.trim() || null;
  }
  return client.request<WorkflowSnapshot>("task_monitor_create", params);
}

/** Delete one connector monitor and return the refreshed workflow snapshot. */
export async function deleteMonitor(slug: string): Promise<WorkflowSnapshot> {
  return deleteWorkflowBinding(slug);
}

/** Ignore one monitor-created task and return the refreshed workflow snapshot. */
export async function ignoreMonitorTask(taskId: string, reason?: string): Promise<WorkflowSnapshot> {
  const client = await ensureLocalDaemonClient();
  return client.request<WorkflowSnapshot>("task_monitor_ignore", {
    task_id: taskId,
    reason: reason?.trim() || undefined
  });
}

/** Save one monitor memory file and return the refreshed workflow snapshot. */
export async function saveMonitorMemory(connectionSlug: string, content: string): Promise<WorkflowSnapshot> {
  const client = await ensureLocalDaemonClient();
  return client.request<WorkflowSnapshot>("task_monitor_memory_save", {
    connection_slug: connectionSlug,
    content
  });
}

/** Load recent received monitor messages and their agent outcomes. */
export async function loadMonitorHistory(limit = 200): Promise<WorkflowMonitorHistoryMessage[]> {
  const client = await ensureLocalDaemonClient();
  const result = await client.request<{ messages?: WorkflowMonitorHistoryMessage[] }>(
    "task_monitor_history_list",
    { limit }
  );
  return result.messages ?? [];
}

/** Delete one connection-triggered workflow binding. */
export async function deleteWorkflowBinding(slug: string): Promise<WorkflowSnapshot> {
  const client = await ensureLocalDaemonClient();
  return client.request<WorkflowSnapshot>("workflow_binding_delete", { slug });
}

/** Delete one connector connection. */
export async function deleteWorkflowConnection(slug: string): Promise<WorkflowSnapshot> {
  const client = await ensureLocalDaemonClient();
  return client.request<WorkflowSnapshot>("workflow_connection_delete", { slug });
}

/** Toggle a native workflow or subscription workflow binding. */
export async function toggleWorkflow(slug: string, enabled: boolean): Promise<WorkflowSnapshot> {
  const client = await ensureLocalDaemonClient();
  return client.request<WorkflowSnapshot>("workflow_toggle", { slug, enabled });
}

/** Load runs for one workflow slug from the daemon. */
export async function listWorkflowRuns(workflowSlug: string): Promise<WorkflowRun[]> {
  const client = await ensureLocalDaemonClient();
  return client.request<WorkflowRun[]>("workflow_runs_list", { workflowSlug });
}

/** Load one workflow run by global run index from the daemon. */
export async function showWorkflowRun(idx: number): Promise<WorkflowRun | null> {
  const client = await ensureLocalDaemonClient();
  return client.request<WorkflowRun | null>("workflow_run_show", { idx });
}

export type PermissionAction = "allow_once" | "allow_session" | "allow_all_session" | "deny";
export type UserQuestionAnswers = Record<string, string | string[]>;
export type UserQuestionAnnotations = Record<string, Record<string, string>>;
export type AgentPermissionMode = "read-only" | "workspace-write" | "full-access";
export type AgentTurnMode = "default" | "plan";
export type AgentTurnOptions = {
  providerId?: string | null;
  modelId?: string | null;
  thinkingOptionId?: string | null;
  fastMode?: boolean;
  permissionMode?: AgentPermissionMode;
  mode?: AgentTurnMode;
};

/** Starts a new agent turn on `sessionId` with `message`. Returns the turn id
 *  so the caller can correlate streamed events and reply to permission
 *  prompts. Subscribe to `subscribeSessionEvents(sessionId, handler)` to see
 *  events as the turn runs. */
export async function runAgentTurn(
  sessionId: string,
  message: string,
  options: AgentTurnOptions = {}
): Promise<string> {
  try {
    const client = await ensureLocalDaemonClient();
    const result = await client.request<{ turnId: string }>("run_agent_turn", {
      sessionId,
      message,
      ...options
    });
    return result.turnId;
  } catch (daemonError) {
    if (!canInvokeTauri()) throw daemonError;
    // Fallback: the in-process Tauri command (same behavior, just no daemon).
    return invoke<string>("run_agent_turn", { sessionId, message, ...options });
  }
}

/** Runs a slash command (e.g. `/connect <slug> <conn>`) through the
 *  deterministic command dispatcher. No provider/LLM is contacted. Streams
 *  `user-question-request`, `turn-complete`, and `turn-error` events on the
 *  same session channel as a regular turn. */
export async function dispatchSlashCommand(
  sessionId: string,
  message: string
): Promise<string> {
  const client = await ensureLocalDaemonClient();
  const result = await client.request<{ turnId: string }>("dispatch_slash_command", {
    sessionId,
    message
  });
  return result.turnId;
}

/** Runs deterministic connector setup without creating a persisted session.
 *  Events stream on `connector-setup:<setupId>:event` and use the same
 *  question/complete/error payloads as a normal turn. */
export async function startConnectorSetupCommand(
  setupId: string,
  message: string
): Promise<string> {
  const client = await ensureLocalDaemonClient();
  const result = await client.request<{ turnId: string }>("start_connector_setup", {
    setupId,
    message
  });
  return result.turnId;
}

/** Resolves a pending permission prompt for an in-flight turn. */
export async function resolvePermission(
  turnId: string,
  requestId: string,
  action: PermissionAction
): Promise<void> {
  try {
    const client = await ensureLocalDaemonClient();
    await client.request("resolve_permission", { turnId, requestId, action });
    return;
  } catch (daemonError) {
    if (!canInvokeTauri()) throw daemonError;
    await invoke("resolve_permission", { turnId, requestId, action });
  }
}

/** Resolves a pending AskUserQuestion prompt for an in-flight turn. */
export async function resolveUserQuestion(
  turnId: string,
  requestId: string,
  answers: UserQuestionAnswers,
  annotations: UserQuestionAnnotations = {}
): Promise<void> {
  try {
    const client = await ensureLocalDaemonClient();
    await client.request("resolve_user_question", { turnId, requestId, answers, annotations });
    return;
  } catch (daemonError) {
    if (!canInvokeTauri()) throw daemonError;
    await invoke("resolve_user_question", { turnId, requestId, answers, annotations });
  }
}

/** Best-effort cancel: the current model/tool step completes then the turn
 *  exits. Any pending permission is treated as Deny. */
export async function cancelTurn(turnId: string): Promise<{ ok: boolean }> {
  try {
    const client = await ensureLocalDaemonClient();
    const result = await client.request<{ ok?: boolean }>("cancel_turn", { turnId });
    return { ok: result?.ok !== false };
  } catch (daemonError) {
    if (!canInvokeTauri()) throw daemonError;
    await invoke("cancel_turn", { turnId });
    return { ok: true };
  }
}

/** Stores an API key credential in the workspace auth store via the daemon
 *  and returns the refreshed settings snapshot. Falls back to the legacy
 *  Tauri-invoke path if the daemon is unreachable. */
export async function loginWithApiKeyViaDaemon(
  providerId: string,
  apiKey: string
): Promise<SettingsSnapshot> {
  try {
    const client = await ensureLocalDaemonClient();
    return await client.request<SettingsSnapshot>("login_with_api_key", {
      providerId,
      apiKey
    });
  } catch (daemonError) {
    if (!canInvokeTauri()) throw daemonError;
    return loginWithApiKey(providerId, apiKey);
  }
}

/** Removes stored credentials for a provider via the daemon and returns the
 *  refreshed settings snapshot. Falls back to Tauri-invoke when the daemon
 *  is unreachable. */
export async function logoutProviderViaDaemon(
  providerId: string
): Promise<SettingsSnapshot> {
  try {
    const client = await ensureLocalDaemonClient();
    return await client.request<SettingsSnapshot>("logout_provider", {
      providerId
    });
  } catch (daemonError) {
    if (!canInvokeTauri()) throw daemonError;
    return logoutProvider(providerId);
  }
}

// ---------------------------------------------------------------------------
// Filesystem RPCs for the Files tab.
// ---------------------------------------------------------------------------

export type DirEntryKind = "file" | "directory" | "symlink";

export type DirEntry = {
  name: string;
  kind: DirEntryKind;
  size: number;
  modifiedMs: number;
};

export type ReadFileResult = {
  path: string;
  encoding: "utf8" | "base64";
  content: string;
  size: number;
  truncated: boolean;
  textPreview?: string[];
  htmlPreview?: string;
};

export type FileTabStateItem = {
  path: string;
  pinned: boolean;
};

export type FileTabsState = {
  tabs: FileTabStateItem[];
  activePath?: string | null;
};

export type LspOperationResult = {
  operation: string;
  filePath: string;
  result: string;
  resultCount?: number;
  fileCount?: number;
};

export type LspInspectResult = {
  path: string;
  cwd: string;
  line: number;
  character: number;
  operations: Record<string, LspOperationResult>;
};

/** List one directory. Absolute path required. The daemon enforces an
 *  allowlist (session cwd, workspace root, $HOME), so paths outside those
 *  roots error out. Entries come back sorted dirs-first, then files. */
export async function listDir(path: string): Promise<DirEntry[]> {
  const client = await ensureLocalDaemonClient();
  const result = await client.request<{ entries: DirEntry[] }>("list_dir", { path });
  return result.entries;
}

/** Read a file. `maxBytes` caps the returned content (default 256 KiB);
 *  larger files are truncated and returned with `truncated: true`. Files
 *  larger than the daemon hard limit are refused outright with an error. Binary files
 *  come back base64-encoded with `encoding: "base64"`. */
export async function readFile(path: string, maxBytes?: number): Promise<ReadFileResult> {
  const client = await ensureLocalDaemonClient();
  return client.request<ReadFileResult>(
    "read_file",
    maxBytes != null ? { path, maxBytes } : { path }
  );
}

/** Overwrite an existing UTF-8 file and return the updated file content. */
export async function writeFile(path: string, content: string): Promise<ReadFileResult> {
  const client = await ensureLocalDaemonClient();
  return client.request<ReadFileResult>("write_file", { path, content });
}

/** Load daemon-persisted Files tab state for one agent session. */
export async function loadFileTabs(sessionId: string): Promise<FileTabsState> {
  const client = await ensureLocalDaemonClient();
  return client.request<FileTabsState>("load_file_tabs", { sessionId });
}

/** Persist Files tab state for one agent session in the daemon. */
export async function saveFileTabs(
  sessionId: string,
  state: FileTabsState
): Promise<FileTabsState> {
  const client = await ensureLocalDaemonClient();
  return client.request<FileTabsState>("save_file_tabs", { sessionId, ...state });
}

/** Ask the configured language server for symbol context at a file position.
 *  `line` and `character` are zero-based LSP coordinates. */
export async function lspInspect(
  path: string,
  cwd: string,
  line: number,
  character: number
): Promise<LspInspectResult> {
  const client = await ensureLocalDaemonClient();
  return client.request<LspInspectResult>("lsp_inspect", { path, cwd, line, character });
}

/** Start a filesystem watch. `paths` must live under the daemon's allowlist
 *  (session cwd / workspace root / $HOME). The daemon fires
 *  `workspace:fs:changed` events with `{watchId, paths}` debounced on a 100 ms
 *  window. Recursive watches follow into every subdirectory. Dispose via
 *  `fsUnwatch(watchId)` on unmount to free the native watcher. */
export async function fsWatch(
  paths: string[],
  recursive: boolean = true,
  watchId: string | null = null
): Promise<{ watchId: string }> {
  const client = await ensureLocalDaemonClient();
  return client.request<{ watchId: string }>("fs_watch", {
    paths,
    recursive,
    ...(watchId ? { watchId } : {})
  });
}

/** Stop a filesystem watch. Idempotent — a stale id is a no-op. */
export async function fsUnwatch(watchId: string): Promise<void> {
  const client = await ensureLocalDaemonClient();
  await client.request("fs_unwatch", { watchId });
}

/** Payload shape for `workspace:fs:changed` events. `replay: true` means the
 *  daemon is re-delivering the event after a reconnect — a newly-mounted
 *  FilesPane has no cache yet and should ignore replays. */
export type FsChangedEvent = {
  watchId: string;
  paths: string[];
  changedAtMs: number;
  replay?: boolean;
};

// ---------------------------------------------------------------------------
// PTY — user-facing Terminal tab. The daemon spawns a shell in the session's
// cwd; data is base64-encoded on the wire to stay ASCII-safe regardless of
// shell output. One pty_id per open tab.
// ---------------------------------------------------------------------------

export type PtyTabInfo = {
  ptyId: string;
  sessionId: string;
  title: string;
  cwd: string;
  cols: number;
  rows: number;
  createdAtMs: number;
  active: boolean;
};

export type PtySessionInfo = {
  tabs: PtyTabInfo[];
  initialized: boolean;
};

export type PtyReplayChunk = {
  seq: number;
  data: string;
};

/** List live PTYs owned by one agent session. */
export async function listPtys(sessionId: string): Promise<PtySessionInfo> {
  const client = await ensureLocalDaemonClient();
  return client.request<PtySessionInfo>("pty_list", { sessionId });
}

/** Open a new PTY in `cwd` (defaults to the daemon's cwd). Returns the
 *  opaque pty_id to use on subsequent write/resize/close calls and to
 *  subscribe to `pty:<id>:data` + `pty:<id>:exit` events. */
export async function openPty(params: {
  sessionId?: string;
  cwd?: string;
  cols?: number;
  rows?: number;
  title?: string;
}): Promise<{ ptyId: string }> {
  const client = await ensureLocalDaemonClient();
  return client.request<{ ptyId: string }>("pty_open", params);
}

/** Mark a PTY as the active terminal for its agent session. */
export async function focusPty(ptyId: string): Promise<void> {
  const client = await ensureLocalDaemonClient();
  await client.request("pty_focus", { ptyId });
}

/** Replay buffered PTY output for reconnecting terminal panes. */
export async function replayPty(ptyId: string): Promise<PtyReplayChunk[]> {
  const client = await ensureLocalDaemonClient();
  const result = await client.request<{ chunks: PtyReplayChunk[] }>("pty_replay", { ptyId });
  return result.chunks;
}

/** Rename a live PTY tab. */
export async function renamePty(ptyId: string, title: string): Promise<PtyTabInfo> {
  const client = await ensureLocalDaemonClient();
  return client.request<PtyTabInfo>("pty_rename", { ptyId, title });
}

/** Send keystrokes to the PTY. `dataB64` is the base64-encoding of the
 *  raw bytes (typically the UTF-8 of xterm's onData string). */
export async function writePty(ptyId: string, dataB64: string): Promise<void> {
  const client = await ensureLocalDaemonClient();
  await client.request("pty_write", { ptyId, data: dataB64 });
}

/** Resize the PTY; xterm's FitAddon supplies cols/rows after measuring. */
export async function resizePty(
  ptyId: string,
  cols: number,
  rows: number
): Promise<void> {
  const client = await ensureLocalDaemonClient();
  await client.request("pty_resize", { ptyId, cols, rows });
}

/** Tear down the PTY. Idempotent — calling on an already-exited pty_id is
 *  a no-op on the daemon side. */
export async function closePty(ptyId: string): Promise<void> {
  const client = await ensureLocalDaemonClient();
  await client.request("pty_close", { ptyId });
}

// ---------------------------------------------------------------------------
// Browser — Chrome-backed Browser tab. The daemon owns a managed Chrome
// process and streams screencast frames back to the renderer.
// ---------------------------------------------------------------------------

export type BrowserState = {
  url: string;
  title: string;
  loading: boolean;
  updatedAtMs?: number;
  width?: number;
  height?: number;
  popOut?: boolean;
  error?: string;
};

export type BrowserFrameEvent = {
  frameId: string;
  mimeType: string;
  encoding: "base64";
  data: string;
  width: number;
  height: number;
};

export type BrowserMouseInput = {
  kind: "mouse";
  eventType: "mousePressed" | "mouseReleased" | "mouseMoved";
  x: number;
  y: number;
  button?: "left" | "middle" | "right" | "none";
  buttons?: number;
  clickCount?: number;
};

export type BrowserWheelInput = {
  kind: "wheel";
  x: number;
  y: number;
  deltaX: number;
  deltaY: number;
};

export type BrowserKeyInput = {
  kind: "key";
  eventType: "keyDown" | "keyUp" | "rawKeyDown" | "char";
  key: string;
  code: string;
  text?: string;
  modifiers?: number;
};

export type BrowserTextInput = {
  kind: "text";
  text: string;
};

export type BrowserInputEvent =
  | BrowserMouseInput
  | BrowserWheelInput
  | BrowserKeyInput
  | BrowserTextInput;

export type BrowserCopySelectionResult = {
  text: string;
  copiedFrom: string;
};

export type BrowserCursorResult = {
  cursor: string;
};

export type BrowserDevtoolsEvent =
  | {
      kind: "console";
      level: string;
      text: string;
      url?: string;
      timestamp?: number;
    }
  | {
      kind: "network";
      phase: "request" | "response" | "failed";
      requestId: string;
      method?: string;
      status?: number;
      url?: string;
      mimeType?: string;
      errorText?: string;
    };

export type BrowserTabInfo = {
  tabId: string;
  label: string;
  url: string;
  title: string;
  loading: boolean;
  connected: boolean;
  active: boolean;
  backendSessionId: string;
  createdAtMs: number;
  updatedAtMs: number;
};

export type BrowserTabsState = {
  activeTabId?: string | null;
  tabs: BrowserTabInfo[];
};

export type BrowserRecordedFrame = {
  frameId: string;
  backendSessionId: string;
  rootSessionId: string;
  tabId: string;
  url: string;
  title: string;
  mimeType: string;
  encoding: string;
  data: string;
  width: number;
  height: number;
  recordedAtMs: number;
};

export type BrowserRecordingSnapshot = {
  frames: BrowserRecordedFrame[];
};

export type BrowserBackendStatus = {
  preferredRenderer: BrowserRenderer;
  activeRenderer: BrowserRenderer;
  fallbackReason: string | null;
  cef: {
    available: boolean;
    root: string | null;
    frameworkPath: string | null;
    missing: string[];
    tintinChromium: {
      executable: string | null;
      appBundle: string | null;
      isCefRuntime: boolean;
    };
    buildHint: string;
  };
  screencast: {
    chromiumExecutable: string | null;
  };
};

export type BrowserCefNativeStatus = {
  available: boolean;
  active: boolean;
  root: string | null;
  helper: string | null;
  remoteDebuggingPort: number;
  buildEnabled: boolean;
  error: string | null;
};

export type BrowserCefNativeRect = {
  x: number;
  y: number;
  width: number;
  height: number;
};

export type BrowserCefNativeState = BrowserState & {
  connected: boolean;
  remoteDebuggingPort?: number;
};

/** Report the preferred Browser renderer and the runtime fallback state. */
export async function browserBackendStatus(
  preferredRenderer: BrowserRenderer
): Promise<BrowserBackendStatus> {
  const client = await ensureLocalDaemonClient();
  return client.request<BrowserBackendStatus>("browser_backend_status", { preferredRenderer });
}

/** Report whether the Tauri process can host a native CEF browser view. */
export async function browserCefNativeStatus(): Promise<BrowserCefNativeStatus | null> {
  try {
    return await invoke<BrowserCefNativeStatus>("browser_cef_native_status");
  } catch {
    return null;
  }
}

/** Open or focus a native CEF browser inside the Tauri window. */
export async function browserCefNativeOpen(params: {
  sessionId: string;
  url?: string;
  rect: BrowserCefNativeRect;
}): Promise<BrowserCefNativeState> {
  return invoke<BrowserCefNativeState>("browser_cef_native_open", params);
}

/** Resize a native CEF browser inside the Tauri window. */
export async function browserCefNativeResize(
  sessionId: string,
  rect: BrowserCefNativeRect
): Promise<BrowserCefNativeState> {
  return invoke<BrowserCefNativeState>("browser_cef_native_resize", { sessionId, rect });
}

/** Navigate a native CEF browser. */
export async function browserCefNativeNavigate(
  sessionId: string,
  url: string
): Promise<BrowserCefNativeState> {
  return invoke<BrowserCefNativeState>("browser_cef_native_navigate", { sessionId, url });
}

/** Reload a native CEF browser. */
export async function browserCefNativeReload(sessionId: string): Promise<BrowserCefNativeState> {
  return invoke<BrowserCefNativeState>("browser_cef_native_reload", { sessionId });
}

/** Move a native CEF browser backward or forward in history. */
export async function browserCefNativeHistory(
  sessionId: string,
  direction: "back" | "forward"
): Promise<BrowserCefNativeState> {
  return invoke<BrowserCefNativeState>("browser_cef_native_history", { sessionId, direction });
}

/** Close a native CEF browser. */
export async function browserCefNativeClose(sessionId: string): Promise<void> {
  await invoke("browser_cef_native_close", { sessionId });
}

/** Open or reuse the Chrome-backed browser session for a Puffer session. */
export async function browserOpen(params: {
  sessionId: string;
  url?: string;
  width: number;
  height: number;
}): Promise<BrowserState> {
  const client = await ensureLocalDaemonClient();
  return client.request<BrowserState>("browser_open", params);
}

/** Navigate the Chrome-backed browser session. */
export async function browserNavigate(sessionId: string, url: string): Promise<void> {
  const client = await ensureLocalDaemonClient();
  await client.request("browser_navigate", { sessionId, url });
}

/** Reload the Chrome-backed browser session. */
export async function browserReload(sessionId: string): Promise<void> {
  const client = await ensureLocalDaemonClient();
  await client.request("browser_reload", { sessionId });
}

/** Move the Chrome-backed browser session backward or forward in history. */
export async function browserHistory(
  sessionId: string,
  direction: "back" | "forward"
): Promise<void> {
  const client = await ensureLocalDaemonClient();
  await client.request("browser_history", { sessionId, direction });
}

/** Resize the Chrome viewport backing the Browser tab. */
export async function browserResize(
  sessionId: string,
  width: number,
  height: number
): Promise<void> {
  const client = await ensureLocalDaemonClient();
  await client.request("browser_resize", { sessionId, width, height });
}

/** Forward one user input event from the Browser tab to Chrome. */
export async function browserInput(
  sessionId: string,
  event: BrowserInputEvent
): Promise<void> {
  const client = await ensureLocalDaemonClient();
  await client.request("browser_input", { sessionId, event });
}

/** Copy the current Chrome-owned webpage selection as plain text. */
export async function browserCopySelection(
  sessionId: string
): Promise<BrowserCopySelectionResult> {
  const client = await ensureLocalDaemonClient();
  return client.request<BrowserCopySelectionResult>("browser_copy_selection", { sessionId });
}

/** Read the CSS cursor Chrome reports at a browser viewport coordinate. */
export async function browserCursor(
  sessionId: string,
  x: number,
  y: number
): Promise<BrowserCursorResult> {
  const client = await ensureLocalDaemonClient();
  return client.request<BrowserCursorResult>("browser_cursor", { sessionId, x, y });
}

/** Close the Chrome-backed browser session. */
export async function browserClose(sessionId: string): Promise<void> {
  const client = await ensureLocalDaemonClient();
  await client.request("browser_close", { sessionId });
}

/** List the daemon-owned Browser tabs for an agent session. */
export async function browserTabsList(sessionId: string): Promise<BrowserTabsState> {
  const client = await ensureLocalDaemonClient();
  return client.request<BrowserTabsState>("browser_agent", { action: "list", sessionId });
}

/** Open or reuse a daemon-owned Browser tab for an agent session. */
export async function browserTabOpen(params: {
  sessionId: string;
  tabId?: string;
  label?: string;
  url?: string;
  width?: number;
  height?: number;
  activate?: boolean;
}): Promise<BrowserTabInfo> {
  const client = await ensureLocalDaemonClient();
  return client.request<BrowserTabInfo>("browser_agent", { action: "open", ...params });
}

/** Focus a daemon-owned Browser tab for an agent session. */
export async function browserTabFocus(sessionId: string, tabId: string): Promise<BrowserTabInfo> {
  const client = await ensureLocalDaemonClient();
  return client.request<BrowserTabInfo>("browser_agent", { action: "focus", sessionId, tabId });
}

/** Close a daemon-owned Browser tab for an agent session. */
export async function browserTabClose(sessionId: string, tabId: string): Promise<BrowserTabsState> {
  const client = await ensureLocalDaemonClient();
  return client.request<BrowserTabsState>("browser_agent", { action: "close", sessionId, tabId });
}

/** Load the deduplicated browser screen recording for one agent session. */
export async function browserRecording(sessionId: string): Promise<BrowserRecordingSnapshot> {
  const client = await ensureLocalDaemonClient();
  return client.request<BrowserRecordingSnapshot>("browser_recording", { sessionId });
}

// ---------------------------------------------------------------------------
// Settings persistence — MCP servers, provider models, permissions, config.
// All round-trip through the daemon so remote workspaces get the same UX.
// ---------------------------------------------------------------------------

export type McpServerInfo = {
  id: string;
  displayName: string;
  description: string;
  transport: string;
  endpoint: string;
  target: string;
  sourceKind: string;
  sourcePath: string | null;
};

export type AddMcpServerInput = {
  id: string;
  displayName?: string;
  description?: string;
  transport: "stdio" | "sse" | "http";
  endpoint?: string;
  target?: string;
  scope?: "local" | "user";
};

export type ModelDescriptorInfo = {
  id: string;
  displayName: string;
  description?: string;
  provider: string;
  api: string;
  contextWindow: number;
  maxOutputTokens: number;
  supportsReasoning: boolean;
  supportsTools?: boolean;
  isDefault?: boolean;
  thinkingOptions?: {
    id: string;
    label: string;
    description?: string;
    isDefault?: boolean;
  }[];
  defaultThinkingOptionId?: string;
};

export type LocalModelStatus = {
  id: string;
  modelId: string;
  displayName: string;
  checkedAtMs: number;
  supported: boolean;
  recommended: boolean;
  installed: boolean;
  configured: boolean;
  running: boolean;
  installing: boolean;
  reason: string;
  endpoint: string;
  size: string;
  installPath: string;
  providerPath: string;
  logPath: string;
  installLogPath: string;
  serveLogPath: string;
  checks: LocalModelCheck[];
};

export type LocalModelCheck = {
  label: string;
  state: "ok" | "missing" | "warning" | "error" | string;
  detail: string;
};

export type LocalModelInstallJob = {
  jobId: string;
  status: LocalModelStatus;
};

export type LocalModelEvent = {
  modelId: string;
  jobId: string;
  phase: string;
  message: string;
  status?: LocalModelStatus | null;
};

export type PermissionsSnapshot = {
  path: string;
  tools: Record<string, string>;
};

export type LambdaSkillLibraryInfo = {
  id: string;
  root: string;
  generatedSubpath?: string | null;
  hostCatalogueSubpath?: string | null;
  compilerPath?: string | null;
  allowedTools: string[];
  hostToolBindings: Record<string, string[]>;
  skillHostToolBindings: Record<string, Record<string, string[]>>;
  userInvocable: boolean;
  disableModelInvocation: boolean;
  requireApproval: boolean;
  disabledSkills: string[];
  sourceKind: string;
  sourcePath: string;
};

export type LambdaVerifiedSkillInfo = {
  name: string;
  description: string;
  libraryId?: string | null;
  libraryRoot?: string | null;
  sourceKind?: string | null;
  sourcePath?: string | null;
  generatedPath?: string | null;
  ready: boolean;
  enabled: boolean;
  modelInvocable: boolean;
  gateSource?: string | null;
  failureReason?: string | null;
  allowedTools: string[];
  requireApproval: boolean;
  tools?: number | null;
  actions?: number | null;
};

export type LambdaSkillLibrariesSnapshot = {
  directories: {
    workspace: string;
    user: string;
  };
  libraries: LambdaSkillLibraryInfo[];
  skills: LambdaVerifiedSkillInfo[];
  doctor: string;
  warnings: string[];
};

export type SaveLambdaSkillLibraryInput = {
  id: string;
  root: string;
  generatedSubpath?: string | null;
  hostCatalogueSubpath?: string | null;
  compilerPath?: string | null;
  allowedTools?: string[];
  hostToolBindings?: Record<string, string[]>;
  skillHostToolBindings?: Record<string, Record<string, string[]>>;
  userInvocable?: boolean;
  disableModelInvocation?: boolean;
  requireApproval?: boolean;
  scope?: "workspace" | "user";
};

export type SetLambdaSkillEnabledInput = {
  libraryId: string;
  sourceKind: "workspace" | "user";
  skillName: string;
  enabled: boolean;
};

export type SetLambdaSkillApprovalInput = {
  libraryId: string;
  sourceKind: "workspace" | "user";
  requireApproval: boolean;
};

export type RemoveLambdaSkillLibraryInput = {
  libraryId: string;
  sourceKind: "workspace" | "user";
};

export type ConfigPatch = {
  defaultProvider?: string | null;
  defaultModel?: string | null;
  theme?: string;
  openaiBaseUrl?: string | null;
};

export async function localModelStatus(modelId = "minicpm5"): Promise<LocalModelStatus> {
  const client = await ensureLocalDaemonClient();
  return client.request<LocalModelStatus>("local_model_status", { modelId });
}

export async function installLocalModel(modelId = "minicpm5"): Promise<LocalModelInstallJob> {
  const client = await ensureLocalDaemonClient();
  return client.request<LocalModelInstallJob>("install_local_model", { modelId });
}

export async function listMcpServers(): Promise<McpServerInfo[]> {
  const client = await ensureLocalDaemonClient();
  const result = await client.request<{ servers: McpServerInfo[] }>("list_mcp_servers");
  return result.servers;
}

export async function addMcpServer(input: AddMcpServerInput): Promise<McpServerInfo[]> {
  const client = await ensureLocalDaemonClient();
  const result = await client.request<{ servers: McpServerInfo[] }>("add_mcp_server", input);
  return result.servers;
}

export async function listProviderModels(providerId: string): Promise<ModelDescriptorInfo[]> {
  const client = await ensureLocalDaemonClient();
  const result = await client.request<{ providerId: string; models: ModelDescriptorInfo[] }>(
    "list_provider_models",
    { providerId }
  );
  return result.models;
}

export async function createOpenAIRealtimeClientSecret(
  options: OpenAIRealtimeClientSecretOptions = {}
): Promise<OpenAIRealtimeClientSecret> {
  const client = await ensureLocalDaemonClient();
  return client.request<OpenAIRealtimeClientSecret>(
    "create_openai_realtime_client_secret",
    options
  );
}

export async function listPermissions(): Promise<PermissionsSnapshot> {
  const client = await ensureLocalDaemonClient();
  return client.request<PermissionsSnapshot>("list_permissions");
}

export async function savePermissions(
  tools: Record<string, string>
): Promise<PermissionsSnapshot> {
  const client = await ensureLocalDaemonClient();
  return client.request<PermissionsSnapshot>("save_permissions", { tools });
}

export async function listLambdaSkillLibraries(): Promise<LambdaSkillLibrariesSnapshot> {
  const client = await ensureLocalDaemonClient();
  return client.request<LambdaSkillLibrariesSnapshot>("list_lambda_skill_libraries");
}

export async function saveLambdaSkillLibrary(
  input: SaveLambdaSkillLibraryInput
): Promise<LambdaSkillLibrariesSnapshot> {
  const client = await ensureLocalDaemonClient();
  return client.request<LambdaSkillLibrariesSnapshot>("save_lambda_skill_library", input);
}

export async function removeLambdaSkillLibrary(
  input: RemoveLambdaSkillLibraryInput
): Promise<LambdaSkillLibrariesSnapshot> {
  const client = await ensureLocalDaemonClient();
  return client.request<LambdaSkillLibrariesSnapshot>("remove_lambda_skill_library", input);
}

export async function setLambdaSkillEnabled(
  input: SetLambdaSkillEnabledInput
): Promise<LambdaSkillLibrariesSnapshot> {
  const client = await ensureLocalDaemonClient();
  return client.request<LambdaSkillLibrariesSnapshot>("set_lambda_skill_enabled", input);
}

export async function setLambdaSkillApproval(
  input: SetLambdaSkillApprovalInput
): Promise<LambdaSkillLibrariesSnapshot> {
  const client = await ensureLocalDaemonClient();
  return client.request<LambdaSkillLibrariesSnapshot>("set_lambda_skill_approval", input);
}

/** Patch the user config file and return the fresh settings snapshot. The
 *  daemon reloads its own in-memory config under the lock, so subsequent
 *  turns pick up the new default_model without a daemon restart. */
export async function updateConfig(patch: ConfigPatch): Promise<SettingsSnapshot> {
  const client = await ensureLocalDaemonClient();
  return client.request<SettingsSnapshot>("update_config", patch);
}

export type Minicpm5Recommendation = {
  recommend: boolean;
  reason?: string;
  display_name?: string;
  why?: string;
  size?: string;
  install_cmd?: string;
};

/** Ask the desktop backend whether to recommend the local MiniCPM5 model on
 *  this machine (macOS + Apple Silicon + not yet installed). */
export async function minicpm5Recommend(): Promise<Minicpm5Recommendation> {
  return await invoke<Minicpm5Recommendation>("minicpm5_recommend");
}

/** Kick off the local-model install. Progress streams as `minicpm5://install-log`
 *  events; completion arrives as `minicpm5://install-done` ({ success }). */
export async function minicpm5Install(): Promise<void> {
  await invoke("minicpm5_install");
}
