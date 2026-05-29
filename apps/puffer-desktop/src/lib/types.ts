export type InspectorTab = "latest-diff" | "history" | "tool-details";
export type AppView = "workspace" | "settings" | "login";

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

export type FolderGroup = {
  id: string;
  label: string;
  path: string;
  sessionCount: number;
  sessions: SessionListItem[];
  tags: string[];
};

export type DesktopPinState = {
  pinnedAgentIds: string[];
  pinnedWorkspacePaths: string[];
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

export type RepoActionResult = {
  ok: boolean;
  action: string;
  message: string;
  repoStatus: RepoStatus;
  pullRequest: PullRequest | null;
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

export type DesktopPreferences = {
  rememberSession: boolean;
  rememberInspectorLayout: boolean;
  launchInspectorOpen: boolean;
  defaultInspectorTab: InspectorTab;
  defaultInspectorWidth: number;
  remoteEnabled: boolean;
  remoteTarget: string;
  remoteCwd: string;
};

export type RemoteConnection = {
  enabled: boolean;
  target: string;
  cwd: string;
  password: string;
};

export type RemoteOperation = {
  success: boolean;
  stdout: string;
  stderr: string;
};

export type SettingsConfig = {
  appName: string;
  defaultProvider: string | null;
  defaultModel: string | null;
  openaiBaseUrl: string | null;
  theme: string;
  mascotId: string;
  mascotDisplayName: string;
  mascotEnabled: boolean;
  uiNoAltScreen: boolean;
  uiTmuxGoldenMode: boolean;
  browserChromeProfile: string | null;
};

export type ResourceCounts = {
  providers: number;
  tools: number;
  agents: number;
  prompts: number;
  hooks: number;
  skills: number;
  mascots: number;
  plugins: number;
  mcpServers: number;
  ides: number;
};

export type SettingsSessionSummary = {
  totalSessions: number;
  folderGroups: number;
};

export type AuthProviderStatus = {
  providerId: string;
  kind: string;
  email: string | null;
  expiresAtMs: number | null;
  scopes: string[];
  planType: string | null;
  organizationName: string | null;
};

export type ProviderSummary = {
  id: string;
  displayName: string;
  baseUrl: string;
  defaultApi: string;
  modelCount: number;
  authModes: string[];
  sourceKind: string;
  sourcePath: string | null;
};

export type BrowserProfile = {
  id: string;
  name: string;
  email: string | null;
  googleAccounts: BrowserGoogleAccount[];
  path: string;
  isLastUsed: boolean;
  isSelected: boolean;
};

export type BrowserGoogleAccount = {
  email: string;
  name: string | null;
  gaiaId: string | null;
};

export type SettingsSnapshot = {
  workspaceRoot: string;
  workspaceConfigFile: string;
  userConfigFile: string;
  authStoreFile: string;
  builtinResourcesDir: string;
  config: SettingsConfig;
  resources: ResourceCounts;
  sessions: SettingsSessionSummary;
  auth: AuthProviderStatus[];
  providers: ProviderSummary[];
  browserProfiles: BrowserProfile[];
};

export type WorkflowTrigger =
  | { type: "cron"; cron: string }
  | {
      type: "subscription";
      source_topic: string;
      pattern?: string | null;
      classify_prompt?: string | null;
    }
  | {
      type: "connection";
      connection_slug: string;
      filter?: Record<string, unknown> | null;
      pattern?: string | null;
      classify_prompt?: string | null;
    };

export type WorkflowPipelineNode = {
  id: string;
  type?: string | null;
  agent?: string | null;
  prompt: string;
  model?: string | null;
  tools?: string[];
  env?: Record<string, string>;
  depends_on?: string[];
};

export type WorkflowDefinition = {
  schema: string;
  slug: string;
  enabled: boolean;
  trigger: WorkflowTrigger;
  pipeline: {
    name: string;
    working_dir?: string | null;
    concurrency?: number | null;
    nodes: WorkflowPipelineNode[];
  };
};

export type WorkflowRunStatus = "pending" | "running" | "completed" | "failed" | "skipped";

export type WorkflowRunNode = {
  id: string;
  status: WorkflowRunStatus;
  started_at_ms?: number | null;
  ended_at_ms?: number | null;
  output?: string | null;
  error?: string | null;
};

export type WorkflowRun = {
  idx: number;
  workflow_slug: string;
  run_id: string;
  trigger: Record<string, unknown>;
  status: WorkflowRunStatus;
  started_at_ms: number;
  ended_at_ms?: number | null;
  nodes: WorkflowRunNode[];
  error?: string | null;
  trigger_key?: string | null;
};

export type WorkflowConnector = {
  connector_slug: string;
  description: string;
  skill: string;
  runtime_hints?: string[];
  requires_auth: boolean;
  can_subscribe: boolean;
  can_proxy_agent: boolean;
  can_trigger_workflow?: boolean;
  suggested_connection_slug?: string;
  connect_command?: string;
  action_slugs: string[];
};

export type WorkflowConnection = {
  slug: string;
  connector_slug: string;
  description: string;
  state: string;
  has_consumer: boolean;
  auth_failure_notified?: boolean;
  can_trigger_workflow?: boolean;
  connect_command?: string | null;
  monitor_command?: string | null;
};

export type WorkflowMonitorTaskAction = {
  name: string;
  prompt: string;
};

export type WorkflowMonitorTask = {
  task_id: string;
  subject: string;
  description: string;
  status: string;
  monitor_connection?: string | null;
  monitor_connector?: string | null;
  monitor_memory_path?: string | null;
  ignored?: boolean;
  actions?: WorkflowMonitorTaskAction[];
  possible_ignore_reasons?: string[];
  started_at_ms?: number | null;
  updated_at_ms?: number | null;
};

export type WorkflowTaskSource = "agent" | "monitor";

export type WorkflowTask = {
  task_id: string;
  subject: string;
  description: string;
  active_form?: string;
  status: string;
  source: WorkflowTaskSource;
  task_scope?: string | null;
  task_scope_label?: string | null;
  task_type?: string | null;
  owner?: string | null;
  blocks?: string[];
  blocked_by?: string[];
  command?: string | null;
  process_id?: number | null;
  output_file?: string | null;
  received_at?: string | null;
  expires_at?: string | null;
  started_at_ms?: number | null;
  updated_at_ms?: number | null;
  exit_code?: number | null;
  ignored?: boolean;
  monitor_connection?: string | null;
  monitor_connector?: string | null;
  monitor_memory_path?: string | null;
  actions?: WorkflowMonitorTaskAction[];
  possible_ignore_reasons?: string[];
};

export type WorkflowFilterRule = Record<string, unknown>;

export type WorkflowBinding = {
  slug: string;
  description: string;
  connection_slug: string;
  connector_slug?: string | null;
  status: string;
  enabled: boolean;
  action_type: string;
  action_path?: string | null;
  action_format?: string | null;
  filter_pattern?: string | null;
  ignore_filters?: WorkflowFilterRule[];
  monitor?: boolean;
  monitor_memory_path?: string | null;
  created_at_ms?: number | null;
};

export type WorkflowBindingCreateRequest = {
  slug?: string;
  description?: string;
  connection_slug: string;
  connector_slug?: string | null;
  pattern?: string | null;
  file_append_path: string;
  enabled?: boolean;
};

export type WorkflowSnapshot = {
  workflows: WorkflowDefinition[];
  runs: WorkflowRun[];
  connectors?: WorkflowConnector[];
  connections?: WorkflowConnection[];
  connector_error?: string | null;
  workflow_bindings?: WorkflowBinding[];
  workflow_binding_error?: string | null;
  tasks?: WorkflowTask[];
  task_error?: string | null;
  monitor_tasks?: WorkflowMonitorTask[];
  monitor_task_error?: string | null;
  monitor_memories?: WorkflowMonitorMemory[];
  monitor_memory_error?: string | null;
  monitor_ignore_filter_error?: string | null;
};

export type WorkflowMonitorMemory = {
  connection_slug: string;
  path: string;
  content: string;
  truncated?: boolean;
};

export type ExternalCredential = {
  providerId: string;
  source: "claude" | "codex";
  kind: "api_key" | "oauth";
  description: string;
  sourcePath: string;
};
