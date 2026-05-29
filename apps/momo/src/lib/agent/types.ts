/**
 * Chat-relevant type subset, ported from
 * `apps/puffer-desktop/src/lib/types.ts` (+ a few from `api/desktop.ts`).
 *
 * Only the types needed to render a conversation and drive an agent turn are
 * kept here. The repo/diff types (`RepoStatus`, `DiffSnapshot`, `AgentDiff`,
 * `DivergenceReport`, `PullRequest`) are retained *because* `SessionDetail`
 * and `normalizeSessionDetail` reference them structurally — without them the
 * `SessionDetail` shape can't round-trip. Browser/workflow/settings/remote/
 * diff-history-UI types from desktop are intentionally dropped.
 */

export type TimelineKind =
  | "user"
  | "assistant"
  | "system"
  | "tool"
  | "permission"
  | "question"
  | "diff"
  | "command";

export type MessageActorKind =
  | "user"
  | "assistant"
  | "agent"
  | "subagent"
  | "team_lead"
  | "system"
  | "runtime";

export type MessageActor = {
  kind: MessageActorKind;
  id: string;
  agentId?: string | null;
  agentType?: string | null;
  name?: string | null;
  teamName?: string | null;
  sessionId?: string | null;
  parentSessionId?: string | null;
};

export type AgentActivityStatus = "idle" | "running" | "awaiting" | "review";

export type SessionListItem = {
  id: string;
  displayName: string | null;
  generatedTitle: string | null;
  title: string;
  cwd: string;
  folderPath: string;
  updatedAtMs: number;
  createdAtMs: number;
  eventCount: number;
  activityStatus: AgentActivityStatus;
  slug: string | null;
  tags: string[];
  note: string | null;
  parentSessionId: string | null;
  providerId: string | null;
  modelId: string | null;
};

export type PullRequest = {
  number: number;
  title: string;
  url: string;
  state: string;
  isDraft: boolean;
  mergeStateStatus: string | null;
  headRefName: string | null;
  baseRefName: string | null;
};

export type RepoStatus = {
  sessionId: string;
  cwd: string;
  isGitRepo: boolean;
  repoRoot: string | null;
  branch: string | null;
  headSha: string | null;
  isClean: boolean;
  hasUncommittedChanges: boolean;
  statusLines: string[];
  ghAvailable: boolean;
  ghAuthenticated: boolean;
  canCreatePr: boolean;
  canMergePr: boolean;
  createPrReason: string | null;
  mergePrReason: string | null;
  pullRequest: PullRequest | null;
  warnings: string[];
};

export type PermissionDialog = {
  state: string;
  reason: string;
  summary: string | null;
  inputText: string | null;
  toolName: string | null;
  choices: string[];
};

export type DiffSnapshot = {
  id: string;
  source: string;
  title: string;
  command: string;
  status: string;
  unstagedDiffstat: string;
  stagedDiffstat: string;
  patch: string;
};

type TimelineBase = {
  id: string;
  kind: TimelineKind;
  createdAtMs?: number | null;
  title: string;
  summary: string;
  body: string;
  meta: string[];
  status?: string | null;
  actor?: MessageActor | null;
};

export type MessageTimelineItem = TimelineBase & {
  kind: "user" | "assistant" | "system" | "command";
};

export type ToolTimelineItem = TimelineBase & {
  kind: "tool";
  toolName: string;
  status: string;
  input: string;
  output: string;
  inputJson: Record<string, unknown> | null;
  metadata?: unknown;
  subject?: MessageActor | null;
};

export type PermissionTimelineItem = TimelineBase & {
  kind: "permission";
  toolName: string | null;
  status: string;
  permissionDialog: PermissionDialog;
  scopeLabel: string | null;
  choices: string[];
};

export type AskUserQuestionOption = {
  label: string;
  description: string;
  preview?: string | null;
};

export type AskUserQuestionItem = {
  question: string;
  header: string;
  type?: "choice" | "input";
  options: AskUserQuestionOption[];
  multiSelect?: boolean;
  searchable?: boolean;
};

export type UserQuestionTimelineItem = TimelineBase & {
  kind: "question";
  status: string;
  questions: AskUserQuestionItem[];
  answers?: Record<string, string | string[]>;
};

export type DiffTimelineItem = TimelineBase & {
  kind: "diff";
  diff: DiffSnapshot;
};

export type TimelineItem =
  | MessageTimelineItem
  | ToolTimelineItem
  | PermissionTimelineItem
  | UserQuestionTimelineItem
  | DiffTimelineItem;

/** A single agent edit reconstructed from a tool-call transcript event.
 *  `kind` is the high-level operation (write/replace/move/remove);
 *  `summary` is a unified-diff-ish snippet rendered server-side. */
export type AgentDiffEntry = {
  callId: string;
  toolId: string;
  kind: string;
  path: string;
  success: boolean;
  summary: string;
};

/** Per-file rollup of the agent's edits — useful for "the agent
 *  touched these N files this session" lists. */
export type AgentDiffFile = {
  path: string;
  latestKind: string;
  editCount: number;
  latestSummary: string;
};

export type AgentDiff = {
  files: AgentDiffFile[];
  entries: AgentDiffEntry[];
};

/** Set difference between agent-touched and git-touched paths. Empty
 *  arrays mean the two views agree; non-empty means there's drift to
 *  surface (hand-edits, hook rewrites, rolled-back applies, …). */
export type DivergenceReport = {
  agentOnly: string[];
  gitOnly: string[];
  agentTotal: number;
  gitTotal: number;
};

export type SessionDetail = {
  session: SessionListItem;
  timeline: TimelineItem[];
  latestDiff: DiffSnapshot | null;
  diffHistory: DiffSnapshot[];
  repoStatus: RepoStatus;
  agentDiff: AgentDiff;
  divergence: DivergenceReport;
};

// --- agent turn options (ported from api/desktop.ts) ---

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
