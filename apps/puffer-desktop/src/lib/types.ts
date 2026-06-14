export type InspectorTab = "latest-diff" | "history" | "tool-details";
export type BrowserRenderer = "cef" | "screencast";
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

export type AgentTurnAttachmentKind = "image" | "file" | "video";
export type AttachmentState = "available" | "missing";
export type AttachmentPreviewResult =
  | { state: "available"; mimeType: string; bytes: number[] }
  | { state: "missing" }
  | { state: "unsupported" };
export type GeneratedVideoAccessResult =
  | { state: "available"; url: string; mimeType: string; size: number; expiresAtMs: number }
  | { state: "missing" }
  | { state: "unsupported" };

export type AgentTurnAttachment = {
  id: string;
  name: string;
  mimeType: string;
  size: number;
  extension: string;
  kind: AgentTurnAttachmentKind;
};

export type AttachmentPreviewSource =
  | { kind: "local_file"; path: string }
  | { kind: "remote_url"; url: string }
  | {
      kind: "generated_media";
      jobId: string;
      artifactId: string;
      index: number;
      localPath?: string | null;
      remoteSourceUrl?: string | null;
    };

export type MessageAttachment = AgentTurnAttachment & {
  state?: AttachmentState;
  source: AttachmentPreviewSource;
  previewUrl?: string | null;
};

export type ChatAttachmentUpload = AgentTurnAttachment & {
  file: File;
  previewUrl?: string | null;
  state?: AttachmentState;
};

export type FolderGroup = {
  id: string;
  label: string;
  path: string;
  sessionCount: number;
  sessions: SessionListItem[];
  tags: string[];
};

export type SessionGroupsPage = {
  groups: FolderGroup[];
  offset: number;
  limit: number;
  returnedSessions: number;
  totalSessions: number;
  hasMore: boolean;
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
  turnId?: string | null;
  title: string;
  summary: string;
  body: string;
  meta: string[];
  attachments?: MessageAttachment[];
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
  secret?: boolean;
};

export type UserQuestionTimelineItem = TimelineBase & {
  kind: "question";
  status: string;
  questions: AskUserQuestionItem[];
  answers?: Record<string, string | string[]>;
  /** Free-form marker from the tool's `metadata` (e.g. { kind: "canvas-offer" }). */
  metadata?: Record<string, unknown>;
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
  browserRenderer: BrowserRenderer;
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
  media: MediaSettings;
  mascotId: string;
  mascotDisplayName: string;
  mascotEnabled: boolean;
  uiNoAltScreen: boolean;
  uiTmuxGoldenMode: boolean;
};

export type MediaSettings = {
  image: MediaGenerationSettings | null;
  video: MediaGenerationSettings | null;
};

export type MediaGenerationSettings = {
  providerId: string;
  logicalModelId: string;
  selections: Record<string, string>;
};

export type MediaKind = "image" | "video";

export type GenerateMediaInput = {
  kind: MediaKind;
  prompt: string;
  count?: number;
};

export type GeneratedMediaArtifactResult = {
  artifactId: string;
  index: number;
  path: string;
  mimeType: string;
  size: number;
  remoteSourceUrl?: string | null;
};

export type GenerateMediaResult = {
  jobId: string;
  requestedCount: number;
  artifacts: GeneratedMediaArtifactResult[];
  kind: MediaKind;
  providerId: string;
  modelId: string;
  status: string;
  prompt: string;
};

export type MediaCapabilityControl =
  | { enum: { values: string[]; default: string } }
  | { range: { min: number; max: number; step: number; default: number } }
  | { bool: { default: boolean } };

export type MediaCapabilityAxisInfo = {
  id: string;
  label: string;
  role: "param" | "selector" | string;
  control: MediaCapabilityControl;
};

export type MediaCapabilityInfo = {
  providerId: string;
  providerDisplayName: string;
  modelId: string;
  modelDisplayName: string;
  kind: MediaKind;
  operation: string;
  axes: MediaCapabilityAxisInfo[];
  status: "available" | "unavailable" | "unknown" | string;
  source: string;
  reason: string | null;
  checkedAtMs: number;
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

export type ProxyScheme = "http" | "https" | "socks5" | "socks5h";

export type SanitizedProxyEndpoint = {
  id: string;
  scheme: ProxyScheme;
  host: string;
  port: number;
  username: string | null;
  hasPassword: boolean;
  uri: string;
};

export type ProxyTestResult = {
  proxyId: string | null;
  ok: boolean;
  message: string;
  latencyMs: number | null;
  statusCode: number | null;
};

export type NetworkProxySettings = {
  enabled: boolean;
  selected: string | null;
  bypass: string[];
  proxies: SanitizedProxyEndpoint[];
  lastTest: ProxyTestResult | null;
};

export type DraftProxyEndpoint = {
  id: string;
  scheme: ProxyScheme;
  host: string;
  port: number;
  username: string | null;
  password: string | null;
  keepPassword?: boolean;
};

export type SaveProxySettingsInput = {
  enabled: boolean;
  selected: string | null;
  bypass: string[];
  proxies: DraftProxyEndpoint[];
};

export type SecretSummary = {
  id: string;
  label: string;
  description: string | null;
  username: string | null;
  origin: string | null;
  source: string;
  createdAtMs: number;
  updatedAtMs: number;
};

export type SecretsSettings = {
  storeFile: string;
  keySource: string;
  chromeImportSupported: boolean;
  items: SecretSummary[];
};

export type BrowserCaptchaSolver = {
  id: "nopecha" | "2captcha" | string;
  displayName: string;
  description: string;
  enabled: boolean;
  baseUrl: string;
  apiKeySecretId: string | null;
  hasApiKey: boolean;
  version: string;
  bundled: boolean;
  extensionPath: string;
  releaseUrl: string;
  downloadUrl: string;
  sha256: string;
  license: string;
};

export type BrowserCaptchaSettings = {
  enabled: boolean;
  selectedSolver: string;
  solvers: BrowserCaptchaSolver[];
};

export type BrowserExtension = {
  id: string;
  displayName: string;
  path: string;
  enabled: boolean;
  manifestPresent: boolean;
  source: string;
};

export type BrowserSettings = {
  extensionsEnabled: boolean;
  extensions: BrowserExtension[];
  captcha: BrowserCaptchaSettings;
};

export type SaveBrowserExtensionInput = {
  id: string;
  displayName: string;
  path: string;
  enabled: boolean;
};

export type SaveBrowserCaptchaSolverInput = {
  id: string;
  enabled: boolean;
  baseUrl: string | null;
  apiKeySecretId: string | null;
};

export type SaveBrowserSettingsInput = {
  extensionsEnabled: boolean;
  extensions: SaveBrowserExtensionInput[];
  captcha: {
    enabled: boolean;
    selectedSolver: string;
    solvers: SaveBrowserCaptchaSolverInput[];
  };
};

export type SaveSecretInput = {
  id?: string | null;
  label: string;
  value: string;
  description?: string | null;
  username?: string | null;
  origin?: string | null;
};

export type ChromeImportReport = {
  imported: number;
  skipped: number;
  errors: string[];
};

export type ChromeSecretsImportResult = {
  settings: SettingsSnapshot;
  report: ChromeImportReport;
};

export type OpenAIRealtimeClientSecretOptions = {
  providerId?: string;
  model?: string;
  voice?: string;
  reasoningEffort?: string;
  session?: Record<string, unknown>;
};

export type OpenAIRealtimeClientSecret = {
  providerId: string;
  model: string;
  voice: string;
  clientSecret: string;
  expiresAt: number | null;
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
  browser: BrowserSettings;
  networkProxy: NetworkProxySettings;
  secrets: SecretsSettings;
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
  monitor_rule_schema?: MonitorRuleSchema | null;
};

export type WorkflowMonitorTaskAction = {
  name: string;
  prompt: string;
};

export type WorkflowActionUsage = {
  input_tokens?: number;
  output_tokens?: number;
  cache_read_tokens?: number;
  cache_creation_tokens?: number;
  spent_tokens?: number;
};

export type WorkflowMonitorTask = {
  task_id: string;
  subject: string;
  description: string;
  status: string;
  monitor_connection?: string | null;
  monitor_connector?: string | null;
  monitor_memory_path?: string | null;
  monitor_envelope_id?: string | null;
  ignored?: boolean;
  ignore_reason?: string | null;
  ignore_analysis_started?: boolean;
  ignore_analysis_status?: string | null;
  ignore_analysis_result?: string | null;
  ignore_analysis_error?: string | null;
  ignore_analysis_usage?: WorkflowActionUsage | null;
  ignore_analysis_completed_at_ms?: number | null;
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
  monitor_envelope_id?: string | null;
  ignore_reason?: string | null;
  ignore_analysis_started?: boolean;
  ignore_analysis_status?: string | null;
  ignore_analysis_result?: string | null;
  ignore_analysis_error?: string | null;
  ignore_analysis_usage?: WorkflowActionUsage | null;
  ignore_analysis_completed_at_ms?: number | null;
  actions?: WorkflowMonitorTaskAction[];
  possible_ignore_reasons?: string[];
};

export type WorkflowMonitorHistoryAction = {
  action: string;
  status: string;
  summary: string;
  started_at_ms: number;
  ended_at_ms: number;
  usage?: WorkflowActionUsage | null;
};

export type WorkflowMonitorHistoryMessage = {
  idx: number;
  run_id: string;
  workflow_slug: string;
  connection_slug?: string | null;
  connector_slug?: string | null;
  envelope_id?: string | null;
  received_at_ms?: number | null;
  topic?: string | null;
  kind?: string | null;
  dedup_key?: string | null;
  summary: string;
  text: string;
  payload?: Record<string, unknown> | null;
  action_log: WorkflowMonitorHistoryAction[];
  status: string;
  started_at_ms: number;
  ended_at_ms: number;
};

export type WorkflowFilterRule = Record<string, unknown>;

export type MonitorRuleMode = "exclude" | "include";

export type MonitorRuleOperator = "contains" | "equals" | "matches" | "exists";

export type MonitorRuleFieldType = "string" | "number" | "boolean" | "enum" | "exists";

export type MonitorRuleSchemaValue = {
  value: string | number | boolean;
  label: string;
};

export type MonitorRuleSchemaTextField = {
  path: string;
  label: string;
};

export type MonitorRuleSchemaField = {
  path: string;
  label: string;
  type: MonitorRuleFieldType;
  operators: MonitorRuleOperator[];
  values?: MonitorRuleSchemaValue[];
};

export type MonitorRuleSchema = {
  version: number;
  event_source?: string | null;
  text_fields?: MonitorRuleSchemaTextField[];
  fields?: MonitorRuleSchemaField[];
  source_path?: string | null;
};

export type MonitorRuleAddRequest =
  | {
      connection_slug: string;
      mode: MonitorRuleMode;
      kind: "keyword";
      keywords: string[];
      operator?: MonitorRuleOperator;
      case_insensitive?: boolean;
    }
  | {
      connection_slug: string;
      mode: MonitorRuleMode;
      kind: "field";
      field: string;
      operator: MonitorRuleOperator;
      value?: string | number | boolean | null;
    };

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
  model?: string | null;
  filter_pattern?: string | null;
  include_filter?: WorkflowFilterRule | null;
  include_filters?: WorkflowFilterRule[];
  ignore_filters?: WorkflowFilterRule[];
  contact_ids?: string[];
  monitor?: boolean;
  monitor_memory_path?: string | null;
  monitor_rule_schema?: MonitorRuleSchema | null;
  created_at_ms?: number | null;
};

export type ContactContextItem = {
  kind: string;
  text: string;
  timestamp_ms?: number | null;
  payload?: Record<string, unknown> | null;
};

export type ConnectorContact = {
  id: string;
  avatar?: string | null;
  name?: string | null;
  context?: ContactContextItem[];
  score?: number;
};

export type SavedContact = {
  id: string;
  name: string;
  description: string;
  avatar?: string | null;
  contact_ids: string[];
};

export type ContactProposal = {
  name: string;
  description: string;
  avatar?: string | null;
  contact_ids: string[];
};

export type ContactsSnapshot = {
  contacts: SavedContact[];
  candidates: ConnectorContact[];
  proposals: ContactProposal[];
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
