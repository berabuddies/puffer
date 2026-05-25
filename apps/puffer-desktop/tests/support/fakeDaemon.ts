import type { Page, WebSocketRoute } from "@playwright/test";

export const FAKE_DAEMON_URL = "ws://127.0.0.1:17777/ws";

type JsonRecord = Record<string, unknown>;

type DaemonRequest = {
  id: number | string;
  method: string;
  params: JsonRecord;
};

type Waiter = {
  method: string;
  predicate: (request: DaemonRequest) => boolean;
  resolve: (request: DaemonRequest) => void;
};

type ResponseDelay = {
  method: string;
  predicate: (request: DaemonRequest) => boolean;
  ms: number;
};

type ResponseFailureDelay = ResponseDelay & {
  error: string;
};

type FakeFileValue =
  | string
  | {
      encoding: "base64";
      content: string;
      size: number;
      textPreview?: string[];
      htmlPreview?: string;
    };

type TabSet = {
  activeTabId: string | null;
  tabs: JsonRecord[];
};

type PtySet = {
  initialized: boolean;
  activePtyId: string | null;
  tabs: JsonRecord[];
};

type WorkflowSnapshotFixture = {
  workflows: JsonRecord[];
  runs: JsonRecord[];
  connectors?: JsonRecord[];
  connections?: JsonRecord[];
  connector_error?: string | null;
  workflow_bindings?: JsonRecord[];
  workflow_binding_error?: string | null;
  monitor_tasks?: JsonRecord[];
  monitor_task_error?: string | null;
};

type SessionDetailOverrides = {
  latestDiff: JsonRecord | null;
  diffHistory: JsonRecord[];
  repoStatus: JsonRecord | null;
  agentDiff: JsonRecord;
  divergence: JsonRecord;
};

export type FakeDaemonSessionInput = {
  sessionId: string;
  displayName?: string | null;
  generatedTitle?: string | null;
  title?: string;
  cwd?: string;
  folderPath?: string;
  updatedAtMs?: number;
  createdAtMs?: number;
  eventCount?: number;
  activityStatus?: "idle" | "running" | "awaiting" | "review" | null;
  slug?: string | null;
  tags?: string[];
  note?: string | null;
  parentSessionId?: string | null;
  providerId?: string | null;
  modelId?: string | null;
  timeline?: JsonRecord[];
  latestDiff?: JsonRecord | null;
  diffHistory?: JsonRecord[];
  repoStatus?: JsonRecord | null;
  agentDiff?: JsonRecord;
  divergence?: JsonRecord;
};

const ONE_PIXEL_PNG =
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAFgwJ/lzTnGQAAAABJRU5ErkJggg==";

const now = Date.now();

const session = {
  sessionId: "session-browser",
  displayName: "Browser regression",
  generatedTitle: null,
  title: "Browser regression",
  cwd: "/tmp/puffer",
  folderPath: "/tmp/puffer",
  updatedAtMs: now,
  createdAtMs: now - 60_000,
  eventCount: 3,
  activityStatus: "idle",
  slug: "browser-regression",
  tags: ["desktop", "browser"],
  note: "Automated desktop browser regression fixture",
  parentSessionId: null,
  providerId: "codex",
  modelId: "test-model"
};

function defaultTimeline(): JsonRecord[] {
  return [
    {
      kind: "user_message",
      id: "msg-user",
      text: "Open the browser tab.",
      createdAtMs: now - 30_000
    },
    {
      kind: "assistant_message",
      id: "msg-assistant",
      text: "Ready to exercise the managed browser.",
      createdAtMs: now - 20_000
    }
  ];
}

function sessionMeta(input: FakeDaemonSessionInput): JsonRecord {
  const {
    timeline: _timeline,
    latestDiff: _latestDiff,
    diffHistory: _diffHistory,
    repoStatus: _repoStatus,
    agentDiff: _agentDiff,
    divergence: _divergence,
    ...metadataInput
  } = input;
  const title = metadataInput.title ?? metadataInput.displayName ?? session.title;
  const cwd = metadataInput.cwd ?? session.cwd;
  const folderPath = metadataInput.folderPath ?? cwd;
  const hasProviderId = Object.prototype.hasOwnProperty.call(metadataInput, "providerId");
  const hasModelId = Object.prototype.hasOwnProperty.call(metadataInput, "modelId");
  return {
    ...session,
    ...metadataInput,
    sessionId: metadataInput.sessionId,
    displayName: metadataInput.displayName ?? title,
    title,
    cwd,
    folderPath,
    updatedAtMs: metadataInput.updatedAtMs ?? session.updatedAtMs,
    createdAtMs: metadataInput.createdAtMs ?? session.createdAtMs,
    eventCount: metadataInput.eventCount ?? input.timeline?.length ?? session.eventCount,
    activityStatus: metadataInput.activityStatus ?? session.activityStatus,
    tags: metadataInput.tags ?? session.tags,
    note: metadataInput.note ?? null,
    parentSessionId: metadataInput.parentSessionId ?? null,
    providerId: hasProviderId ? metadataInput.providerId ?? null : session.providerId,
    modelId: hasModelId ? metadataInput.modelId ?? null : session.modelId
  };
}

const fileTabs = [
  { path: "/tmp/puffer/src/main.rs", pinned: true },
  { path: "/tmp/puffer/src/lib.rs", pinned: true }
];

type FakeFileTab = {
  path: string;
  pinned: boolean;
};

function defaultFileContent(path: string): string {
  return path.endsWith("lib.rs") ? "pub fn fixture() {}\n" : "fn main() {}\n";
}

function parentPath(path: string): string {
  const index = path.lastIndexOf("/");
  return index <= 0 ? "/" : path.slice(0, index);
}

function fakeFileSize(value: FakeFileValue): number {
  return typeof value === "string" ? value.length : value.size;
}

function response(id: number | string, result: unknown): string {
  return JSON.stringify({ type: "response", id, ok: true, result });
}

function failure(id: number | string, error: string): string {
  return JSON.stringify({ type: "response", id, ok: false, error });
}

function browserTabInfo(tabId: string, url = "about:blank", active = true): JsonRecord {
  return {
    tabId,
    label: tabId === "tab-1" ? "New tab" : "Fixture tab",
    url,
    title: url === "about:blank" ? "" : "Fixture page",
    loading: false,
    connected: true,
    active,
    backendSessionId: `${session.sessionId}:browser:${tabId}`,
    createdAtMs: now,
    updatedAtMs: Date.now()
  };
}

function browserState(url = "about:blank"): JsonRecord {
  return {
    url,
    title: url === "about:blank" ? "" : "Fixture page",
    loading: false,
    width: 960,
    height: 720
  };
}

function defaultAuthStatuses(): JsonRecord[] {
  return [
    {
      providerId: "codex",
      kind: "oauth",
      email: "tester@example.com",
      expiresAtMs: null,
      scopes: [],
      planType: "test",
      organizationName: null
    },
    {
      providerId: "anthropic",
      kind: "api_key",
      email: null,
      expiresAtMs: null,
      scopes: [],
      planType: null,
      organizationName: null
    }
  ];
}

export class FakeDaemon {
  readonly requests: DaemonRequest[] = [];
  readonly socketUrls: string[] = [];
  readonly url: string;
  private readonly sockets = new Set<WebSocketRoute>();
  private readonly waiters: Waiter[] = [];
  private readonly responseDelays: ResponseDelay[] = [];
  private readonly responseFailureDelays: ResponseFailureDelay[] = [];
  private readonly methodFailures = new Map<string, string[]>();
  private readonly browserTabs = new Map<string, TabSet>();
  private readonly browserRecordings = new Map<string, JsonRecord[]>();
  private readonly ptys = new Map<string, PtySet>();
  private readonly sessions = new Map<string, JsonRecord>();
  private readonly timelines = new Map<string, JsonRecord[]>();
  private readonly details = new Map<string, SessionDetailOverrides>();
  private groupedSessionFilter: ((metadata: JsonRecord) => boolean) | null = null;
  private readonly files = new Map<string, FakeFileValue>();
  private readonly canonicalFilePaths = new Map<string, string>();
  private fileTabsState: JsonRecord | null = null;
  private readonly lspLocations = new Map<string, string>();
  private readonly providerModels: Record<string, JsonRecord[]>;
  private readonly providerSummaries: JsonRecord[] | null;
  private readonly emitBrowserOpenFrame: boolean;
  private readonly emitBrowserResizeFrame: boolean;
  private workspaceRoot = "/tmp/puffer";
  private authStatuses: JsonRecord[];
  private externalCredentials: JsonRecord[];
  private settingsConfig: { defaultProvider: string | null; defaultModel: string | null } = {
    defaultProvider: "codex",
    defaultModel: "test-model"
  };
  private permissions: JsonRecord = {
    path: "/tmp/puffer/.puffer/permissions.json",
    tools: { bash: "ask" }
  };
  private desktopPins: JsonRecord = {
    pinnedAgentIds: [],
    pinnedWorkspacePaths: []
  };
  private mcpServers: JsonRecord[] = [
    {
      id: "playwright",
      displayName: "Playwright",
      description: "Browser automation",
      transport: "stdio",
      endpoint: "",
      target: "npx @playwright/mcp",
      sourceKind: "builtin",
      sourcePath: null
    }
  ];
  private readonly protocol: "legacy" | "real";
  private readonly activeTurnIds = new Set<string>();
  private workflowSnapshot: WorkflowSnapshotFixture = {
    workflows: [
      {
        schema: "puffer.workflow.v1",
        slug: "agent-review-pipeline",
        enabled: true,
        trigger: { type: "subscription", source_topic: "workspace.task.created", pattern: "review" },
        pipeline: {
          name: "Agent review pipeline",
          working_dir: "/tmp/puffer",
          concurrency: 1,
          nodes: [
            {
              id: "codex-implement",
              type: "codex",
              agent: "Codex implementer",
              model: "gpt-5.4-codex",
              tools: ["read", "edit"],
              prompt: "Implement the requested change."
            },
            {
              id: "claude-review",
              type: "claude",
              agent: "Claude reviewer",
              model: "claude-sonnet-4-5",
              tools: ["read", "diff"],
              depends_on: ["codex-implement"],
              prompt: "Review the implementation."
            },
            {
              id: "puffer-ship",
              type: "puffer",
              agent: "Puffer shipper",
              model: "puffer-default",
              tools: ["git", "test"],
              depends_on: ["claude-review"],
              prompt: "Prepare the handoff."
            }
          ]
        }
      }
    ],
    runs: [],
    connectors: [
      {
        connector_slug: "telegram-login",
        description: "Telegram personal account over MTProto",
        skill: "telegram",
        runtime_hints: ["subscriber", "internal-tool"],
        requires_auth: true,
        can_subscribe: true,
        can_proxy_agent: false,
        can_trigger_workflow: true,
        suggested_connection_slug: "telegram-user",
        connect_command: "/connect telegram-login telegram-user",
        action_slugs: ["send_message", "edit_message", "delete_messages", "vote_poll"]
      },
      {
        connector_slug: "telegram-bot",
        description: "Telegram bot connector for agent proxy and bot chats",
        skill: "telegram-bot",
        runtime_hints: ["serve"],
        requires_auth: true,
        can_subscribe: true,
        can_proxy_agent: true,
        can_trigger_workflow: false,
        suggested_connection_slug: "telegram-bot",
        connect_command: "/connect telegram-bot telegram-bot",
        action_slugs: ["send_message"]
      },
      {
        connector_slug: "discord-bot",
        description: "Discord bot connector configured through puffer serve",
        skill: "discord",
        runtime_hints: ["serve"],
        requires_auth: true,
        can_subscribe: false,
        can_proxy_agent: false,
        can_trigger_workflow: false,
        suggested_connection_slug: "discord-bot",
        connect_command: "/connect discord-bot discord-bot",
        action_slugs: []
      },
      {
        connector_slug: "lark-app",
        description: "Lark custom app connector over OpenAPI",
        skill: "lark",
        runtime_hints: ["internal-tool"],
        requires_auth: true,
        can_subscribe: false,
        can_proxy_agent: false,
        can_trigger_workflow: false,
        suggested_connection_slug: "lark-app",
        connect_command: "/connect lark-app lark-app",
        action_slugs: ["send_message", "react", "send_reaction", "remove_reaction"]
      },
      {
        connector_slug: "lark-login",
        description: "Lark user-token account connector over OpenAPI",
        skill: "lark",
        runtime_hints: ["internal-tool"],
        requires_auth: true,
        can_subscribe: false,
        can_proxy_agent: false,
        can_trigger_workflow: false,
        suggested_connection_slug: "lark-login",
        connect_command: "/connect lark-login lark-login",
        action_slugs: ["send_message", "react", "send_reaction", "remove_reaction"]
      },
      {
        connector_slug: "matrix-bot",
        description: "Matrix room connector configured through puffer serve",
        skill: "matrix",
        runtime_hints: ["serve"],
        requires_auth: true,
        can_subscribe: false,
        can_proxy_agent: false,
        can_trigger_workflow: false,
        suggested_connection_slug: "matrix-bot",
        connect_command: "/connect matrix-bot matrix-bot",
        action_slugs: []
      },
      {
        connector_slug: "slack-app",
        description: "Slack app connector for bot-token Web API actions",
        skill: "slack",
        runtime_hints: ["internal-tool"],
        requires_auth: true,
        can_subscribe: false,
        can_proxy_agent: false,
        can_trigger_workflow: false,
        suggested_connection_slug: "slack-app",
        connect_command: "/connect slack-app slack-app",
        action_slugs: ["send_message", "react", "send_reaction", "remove_reaction"]
      },
      {
        connector_slug: "slack-login",
        description: "Slack workspace account over Web API or local app session",
        skill: "slack",
        runtime_hints: ["internal-tool"],
        requires_auth: true,
        can_subscribe: false,
        can_proxy_agent: false,
        can_trigger_workflow: false,
        suggested_connection_slug: "slack-login",
        connect_command: "/connect slack-login slack-login",
        action_slugs: ["send_message", "react", "send_reaction", "remove_reaction"]
      },
      {
        connector_slug: "slack-bot",
        description: "Legacy Slack bot connector placeholder; use slack-app or slack-login actions",
        skill: "slack",
        runtime_hints: ["connector"],
        requires_auth: true,
        can_subscribe: false,
        can_proxy_agent: false,
        can_trigger_workflow: false,
        suggested_connection_slug: "slack-bot",
        connect_command: "/connect slack-bot slack-bot",
        action_slugs: []
      },
      {
        connector_slug: "email",
        description: "Email connector over SMTP and IMAP-compatible polling",
        skill: "email",
        runtime_hints: ["subscriber", "internal-tool"],
        requires_auth: true,
        can_subscribe: true,
        can_proxy_agent: false,
        can_trigger_workflow: true,
        suggested_connection_slug: "email",
        connect_command: "/connect email email",
        action_slugs: ["send_message"]
      },
      {
        connector_slug: "asana-webhook",
        description: "Asana task, project, and story webhook preset backed by puffer serve",
        skill: "asana-webhook",
        runtime_hints: ["serve"],
        requires_auth: false,
        can_subscribe: false,
        can_proxy_agent: false,
        can_trigger_workflow: false,
        suggested_connection_slug: "asana-webhook",
        connect_command: "/connect asana-webhook asana-webhook",
        action_slugs: []
      },
      {
        connector_slug: "alertmanager-webhook",
        description: "Prometheus Alertmanager webhook preset backed by puffer serve",
        skill: "alertmanager-webhook",
        runtime_hints: ["serve"],
        requires_auth: false,
        can_subscribe: false,
        can_proxy_agent: false,
        can_trigger_workflow: false,
        suggested_connection_slug: "alertmanager-webhook",
        connect_command: "/connect alertmanager-webhook alertmanager-webhook",
        action_slugs: []
      },
      {
        connector_slug: "datadog-webhook",
        description: "Datadog monitor and event webhook preset backed by puffer serve",
        skill: "datadog-webhook",
        runtime_hints: ["serve"],
        requires_auth: false,
        can_subscribe: false,
        can_proxy_agent: false,
        can_trigger_workflow: false,
        suggested_connection_slug: "datadog-webhook",
        connect_command: "/connect datadog-webhook datadog-webhook",
        action_slugs: []
      },
      {
        connector_slug: "newrelic-webhook",
        description: "New Relic issue and alert webhook preset backed by puffer serve",
        skill: "newrelic-webhook",
        runtime_hints: ["serve"],
        requires_auth: false,
        can_subscribe: false,
        can_proxy_agent: false,
        can_trigger_workflow: false,
        suggested_connection_slug: "newrelic-webhook",
        connect_command: "/connect newrelic-webhook newrelic-webhook",
        action_slugs: []
      },
      {
        connector_slug: "opsgenie-webhook",
        description: "Opsgenie alert action webhook preset backed by puffer serve",
        skill: "opsgenie-webhook",
        runtime_hints: ["serve"],
        requires_auth: false,
        can_subscribe: false,
        can_proxy_agent: false,
        can_trigger_workflow: false,
        suggested_connection_slug: "opsgenie-webhook",
        connect_command: "/connect opsgenie-webhook opsgenie-webhook",
        action_slugs: []
      },
      {
        connector_slug: "azure-devops-webhook",
        description: "Azure DevOps code and work item webhook preset backed by puffer serve",
        skill: "azure-devops-webhook",
        runtime_hints: ["serve"],
        requires_auth: false,
        can_subscribe: false,
        can_proxy_agent: false,
        can_trigger_workflow: false,
        suggested_connection_slug: "azure-devops-webhook",
        connect_command: "/connect azure-devops-webhook azure-devops-webhook",
        action_slugs: []
      },
      {
        connector_slug: "bitbucket-webhook",
        description: "Bitbucket push and pull request webhook preset backed by puffer serve",
        skill: "bitbucket-webhook",
        runtime_hints: ["serve"],
        requires_auth: false,
        can_subscribe: false,
        can_proxy_agent: false,
        can_trigger_workflow: false,
        suggested_connection_slug: "bitbucket-webhook",
        connect_command: "/connect bitbucket-webhook bitbucket-webhook",
        action_slugs: []
      },
      {
        connector_slug: "github-webhook",
        description: "GitHub event webhook preset backed by puffer serve",
        skill: "github-webhook",
        runtime_hints: ["serve"],
        requires_auth: false,
        can_subscribe: false,
        can_proxy_agent: false,
        can_trigger_workflow: false,
        suggested_connection_slug: "github-webhook",
        connect_command: "/connect github-webhook github-webhook",
        action_slugs: []
      },
      {
        connector_slug: "grafana-webhook",
        description: "Grafana Alerting webhook preset backed by puffer serve",
        skill: "grafana-webhook",
        runtime_hints: ["serve"],
        requires_auth: false,
        can_subscribe: false,
        can_proxy_agent: false,
        can_trigger_workflow: false,
        suggested_connection_slug: "grafana-webhook",
        connect_command: "/connect grafana-webhook grafana-webhook",
        action_slugs: []
      },
      {
        connector_slug: "gitlab-webhook",
        description: "GitLab issue, merge request, comment, and push webhook preset backed by puffer serve",
        skill: "gitlab-webhook",
        runtime_hints: ["serve"],
        requires_auth: false,
        can_subscribe: false,
        can_proxy_agent: false,
        can_trigger_workflow: false,
        suggested_connection_slug: "gitlab-webhook",
        connect_command: "/connect gitlab-webhook gitlab-webhook",
        action_slugs: []
      },
      {
        connector_slug: "jira-webhook",
        description: "Jira issue and comment webhook preset backed by puffer serve",
        skill: "jira-webhook",
        runtime_hints: ["serve"],
        requires_auth: false,
        can_subscribe: false,
        can_proxy_agent: false,
        can_trigger_workflow: false,
        suggested_connection_slug: "jira-webhook",
        connect_command: "/connect jira-webhook jira-webhook",
        action_slugs: []
      },
      {
        connector_slug: "linear-webhook",
        description: "Linear issue and project webhook preset backed by puffer serve",
        skill: "linear-webhook",
        runtime_hints: ["serve"],
        requires_auth: false,
        can_subscribe: false,
        can_proxy_agent: false,
        can_trigger_workflow: false,
        suggested_connection_slug: "linear-webhook",
        connect_command: "/connect linear-webhook linear-webhook",
        action_slugs: []
      },
      {
        connector_slug: "pagerduty-webhook",
        description: "PagerDuty incident and service webhook preset backed by puffer serve",
        skill: "pagerduty-webhook",
        runtime_hints: ["serve"],
        requires_auth: false,
        can_subscribe: false,
        can_proxy_agent: false,
        can_trigger_workflow: false,
        suggested_connection_slug: "pagerduty-webhook",
        connect_command: "/connect pagerduty-webhook pagerduty-webhook",
        action_slugs: []
      },
      {
        connector_slug: "sentry-webhook",
        description: "Sentry issue, event, and alert webhook preset backed by puffer serve",
        skill: "sentry-webhook",
        runtime_hints: ["serve"],
        requires_auth: false,
        can_subscribe: false,
        can_proxy_agent: false,
        can_trigger_workflow: false,
        suggested_connection_slug: "sentry-webhook",
        connect_command: "/connect sentry-webhook sentry-webhook",
        action_slugs: []
      },
      {
        connector_slug: "stripe-webhook",
        description: "Stripe invoice, payment, and billing webhook preset backed by puffer serve",
        skill: "stripe-webhook",
        runtime_hints: ["serve"],
        requires_auth: false,
        can_subscribe: false,
        can_proxy_agent: false,
        can_trigger_workflow: false,
        suggested_connection_slug: "stripe-webhook",
        connect_command: "/connect stripe-webhook stripe-webhook",
        action_slugs: []
      },
      {
        connector_slug: "shopify-webhook",
        description: "Shopify order, product, customer, and inventory webhook preset backed by puffer serve",
        skill: "shopify-webhook",
        runtime_hints: ["serve"],
        requires_auth: false,
        can_subscribe: false,
        can_proxy_agent: false,
        can_trigger_workflow: false,
        suggested_connection_slug: "shopify-webhook",
        connect_command: "/connect shopify-webhook shopify-webhook",
        action_slugs: []
      },
      {
        connector_slug: "trello-webhook",
        description: "Trello board, card, list, and comment webhook preset backed by puffer serve",
        skill: "trello-webhook",
        runtime_hints: ["serve"],
        requires_auth: false,
        can_subscribe: false,
        can_proxy_agent: false,
        can_trigger_workflow: false,
        suggested_connection_slug: "trello-webhook",
        connect_command: "/connect trello-webhook trello-webhook",
        action_slugs: []
      },
      {
        connector_slug: "webhook",
        description: "HTTP webhook connector configured through puffer serve",
        skill: "webhook",
        runtime_hints: ["serve"],
        requires_auth: true,
        can_subscribe: false,
        can_proxy_agent: false,
        can_trigger_workflow: false,
        suggested_connection_slug: "webhook",
        connect_command: "/connect webhook webhook",
        action_slugs: []
      }
    ],
    connections: [
      {
        slug: "slack-app",
        connector_slug: "slack-app",
        description: "Workspace Slack",
        state: "authenticated",
        has_consumer: false,
        auth_failure_notified: false,
        can_trigger_workflow: false,
        connect_command: "/connect slack-app slack-app",
        monitor_command: null
      },
      {
        slug: "telegram-user",
        connector_slug: "telegram-login",
        description: "Personal Telegram",
        state: "active",
        has_consumer: true,
        auth_failure_notified: false,
        can_trigger_workflow: true,
        connect_command: "/connect telegram-login telegram-user",
        monitor_command: "/monitor telegram-user"
      }
    ],
    connector_error: null,
    workflow_bindings: [
      {
        slug: "monitor-telegram-user",
        description: "Monitor telegram-user for actionable tasks",
        connection_slug: "telegram-user",
        connector_slug: "telegram-login",
        status: "enabled",
        enabled: true,
        action_type: "triage_agent",
        monitor: true,
        monitor_memory_path: "/tmp/telegram-user.md",
        created_at_ms: now - 45_000
      }
    ],
    workflow_binding_error: null,
    monitor_tasks: [
      {
        task_id: "monitor-1",
        subject: "Reply to Telegram support ping",
        description: "Alice asked whether the deployment is finished.",
        status: "pending",
        monitor_connection: "telegram-user",
        monitor_connector: "telegram-login",
        monitor_memory_path: "/tmp/telegram-user.md",
        ignored: false,
        actions: [
          {
            name: "Draft reply",
            prompt: "Draft a concise reply to Alice with the deployment status."
          },
          {
            name: "Open context",
            prompt: "Open the Telegram thread and summarize the latest deployment question."
          }
        ],
        possible_ignore_reasons: ["duplicate support ping", "already answered in thread"],
        started_at_ms: now - 15_000,
        updated_at_ms: now - 5_000
      }
    ],
    monitor_task_error: null
  };
  private nextTab = 2;
  private nextPty = 1;
  private rejectConnections = false;

  constructor(options: {
    sessions?: FakeDaemonSessionInput[];
    providerModels?: Record<string, JsonRecord[]>;
    providers?: JsonRecord[];
    mcpServers?: JsonRecord[];
    protocol?: "legacy" | "real";
    workspaceRoot?: string;
    auth?: JsonRecord[];
    externalCredentials?: JsonRecord[];
    url?: string;
    emitBrowserOpenFrame?: boolean;
    emitBrowserResizeFrame?: boolean;
  } = {}) {
    this.url = options.url ?? FAKE_DAEMON_URL;
    this.protocol = options.protocol ?? "legacy";
    this.workspaceRoot = options.workspaceRoot ?? this.workspaceRoot;
    this.authStatuses = options.auth ?? defaultAuthStatuses();
    this.externalCredentials = options.externalCredentials ?? [];
    this.permissions = {
      ...this.permissions,
      path: `${this.workspaceRoot}/.puffer/permissions.json`
    };
    const sessions = options.sessions ?? [{ ...session, timeline: defaultTimeline() }];
    for (const input of sessions) {
      const metadata = sessionMeta(input);
      const sessionId = String(metadata.sessionId);
      this.sessions.set(sessionId, metadata);
      this.timelines.set(sessionId, input.timeline ?? defaultTimeline());
      this.details.set(sessionId, {
        latestDiff: input.latestDiff ?? null,
        diffHistory: input.diffHistory ?? [],
        repoStatus: input.repoStatus ?? null,
        agentDiff: input.agentDiff ?? { files: [], entries: [] },
        divergence: input.divergence ?? { agentOnly: [], gitOnly: [], agentTotal: 0, gitTotal: 0 }
      });
    }
    this.providerModels = options.providerModels ?? {};
    this.providerSummaries = options.providers ?? null;
    this.mcpServers = options.mcpServers ?? this.mcpServers;
    this.emitBrowserOpenFrame = options.emitBrowserOpenFrame ?? true;
    this.emitBrowserResizeFrame = options.emitBrowserResizeFrame ?? false;
  }

  setWorkspaceRoot(workspaceRoot: string): void {
    this.workspaceRoot = workspaceRoot;
    this.permissions = {
      ...this.permissions,
      path: `${workspaceRoot}/.puffer/permissions.json`
    };
  }

  setSettingsConfig(config: Partial<{ defaultProvider: string | null; defaultModel: string | null }>): void {
    this.settingsConfig = {
      ...this.settingsConfig,
      ...config
    };
  }

  setProviderModels(providerId: string, models: JsonRecord[]): void {
    this.providerModels[providerId] = models;
  }

  setBrowserTabs(sessionId: string, state: TabSet): void {
    this.browserTabs.set(sessionId, {
      activeTabId: state.activeTabId,
      tabs: state.tabs.map((tab) => ({ ...tab }))
    });
  }

  setBrowserRecording(sessionId: string, frames: JsonRecord[]): void {
    this.browserRecordings.set(sessionId, frames.map((frame) => ({ ...frame })));
  }

  setPermissions(tools: Record<string, string>): void {
    this.permissions = {
      path: `${this.workspaceRoot}/.puffer/permissions.json`,
      tools: { ...tools }
    };
  }

  setMcpServers(servers: JsonRecord[]): void {
    this.mcpServers = servers.map((server) => ({ ...server }));
  }

  seedFile(path: string, content: string): void {
    this.files.set(path, content);
  }

  seedCanonicalFile(requestPath: string, resultPath: string, content: string): void {
    this.files.set(requestPath, content);
    this.canonicalFilePaths.set(requestPath, resultPath);
  }

  setFileTabs(tabs: FakeFileTab[], activePath: string | null = tabs[0]?.path ?? null): void {
    this.fileTabsState = {
      tabs: tabs.map((tab) => ({ ...tab })),
      activePath
    };
  }

  seedBinaryFile(
    path: string,
    contentBase64: string,
    size?: number,
    textPreview?: string[],
    htmlPreview?: string
  ): void {
    this.files.set(path, {
      encoding: "base64",
      content: contentBase64,
      size: size ?? Math.ceil((contentBase64.length * 3) / 4),
      ...(textPreview ? { textPreview } : {}),
      ...(htmlPreview ? { htmlPreview } : {})
    });
  }

  setLspLocation(path: string, location: string): void {
    this.lspLocations.set(path, location);
  }

  setAuthStatuses(auth: JsonRecord[]): void {
    this.authStatuses = auth;
  }

  setWorkflowSnapshot(snapshot: WorkflowSnapshotFixture): void {
    this.workflowSnapshot = {
      workflows: snapshot.workflows.map((workflow) => ({ ...workflow })),
      runs: snapshot.runs.map((run) => ({ ...run })),
      connectors: snapshot.connectors?.map((connector) => ({ ...connector })),
      connections: snapshot.connections?.map((connection) => ({ ...connection })),
      connector_error: snapshot.connector_error ?? null,
      workflow_bindings: snapshot.workflow_bindings?.map((binding) => ({ ...binding })),
      workflow_binding_error: snapshot.workflow_binding_error ?? null,
      monitor_tasks: snapshot.monitor_tasks?.map((task) => ({ ...task })),
      monitor_task_error: snapshot.monitor_task_error ?? null
    };
  }

  socketCount(): number {
    return this.sockets.size;
  }

  async disconnectAllSockets(): Promise<void> {
    const sockets = [...this.sockets];
    await Promise.all(
      sockets.map((socket) =>
        socket.close({ code: 1011, reason: "fake daemon forced reconnect" }).catch(() => undefined)
      )
    );
    for (const socket of sockets) this.sockets.delete(socket);
  }

  async install(page: Page): Promise<void> {
    const expectedUrl = new URL(this.url);
    await page.routeWebSocket((url) => {
      const matches =
        !this.rejectConnections && url.origin === expectedUrl.origin && url.pathname === expectedUrl.pathname;
      if (matches) this.socketUrls.push(url.toString());
      return matches;
    }, (socket) => {
      this.sockets.add(socket);
      if (this.protocol === "real") {
        socket.send(JSON.stringify({
          event: "hello",
          payload: { protocolVersion: "1", workspaceRoot: this.workspaceRoot }
        }));
      }
      socket.onMessage((message) => this.handleMessage(socket, String(message)));
      socket.onClose(() => {
        this.sockets.delete(socket);
      });
    });
  }

  async dropConnections(): Promise<void> {
    this.rejectConnections = true;
    await Promise.all(
      Array.from(this.sockets, (socket) =>
        socket.close({ code: 1011, reason: "test disconnect" }).catch(() => {})
      )
    );
    this.sockets.clear();
  }

  allowConnections(): void {
    this.rejectConnections = false;
  }

  async open(
    page: Page,
    options: {
      allowUnauthenticatedWorkspace?: boolean;
      forceOnboarding?: boolean;
      skipOnboarding?: boolean;
      extraParams?: Record<string, string>;
    } = {}
  ): Promise<void> {
    if (options.allowUnauthenticatedWorkspace) {
      await page.addInitScript(() => {
        (window as unknown as { __PUFFER_DESKTOP_ALLOW_UNAUTHENTICATED_WORKSPACE?: boolean })
          .__PUFFER_DESKTOP_ALLOW_UNAUTHENTICATED_WORKSPACE = true;
      });
    }
    const params = new URLSearchParams({
      corbinaBackend: this.url,
      corbinaToken: "test"
    });
    if (options.forceOnboarding) {
      params.set("forceOnboarding", "1");
    } else if (options.skipOnboarding ?? true) {
      params.set("skipOnboarding", "1");
    }
    for (const [key, value] of Object.entries(options.extraParams ?? {})) {
      params.set(key, value);
    }
    await page.goto(`/?${params.toString()}`);
  }

  emit(event: string, payload: unknown): void {
    const p = payload as Record<string, unknown> | null;
    if (p && (p.type === "turn-complete" || p.type === "turn-error") && typeof p.turnId === "string") {
      this.activeTurnIds.delete(p.turnId);
    }
    const message = this.protocol === "real"
      ? JSON.stringify({ event, payload })
      : JSON.stringify({ type: "event", event, payload });
    for (const socket of this.sockets) socket.send(message);
  }

  waitForRequest(
    method: string,
    predicate: (request: DaemonRequest) => boolean = () => true
  ): Promise<DaemonRequest> {
    const existing = this.requests.find((request) => request.method === method && predicate(request));
    if (existing) return Promise.resolve(existing);
    return new Promise((resolve) => {
      this.waiters.push({ method, predicate, resolve });
    });
  }

  delayResponse(
    method: string,
    predicate: (request: DaemonRequest) => boolean,
    ms: number
  ): void {
    this.responseDelays.push({ method, predicate, ms });
  }

  delayFailure(
    method: string,
    predicate: (request: DaemonRequest) => boolean,
    error: string,
    ms: number
  ): void {
    this.responseFailureDelays.push({ method, predicate, error, ms });
  }

  failNext(method: string, error: string): void {
    const failures = this.methodFailures.get(method) ?? [];
    failures.push(error);
    this.methodFailures.set(method, failures);
  }

  setGroupedSessionFilter(filter: ((metadata: JsonRecord) => boolean) | null): void {
    this.groupedSessionFilter = filter;
  }

  setSessionTimeline(sessionId: string, timeline: JsonRecord[]): void {
    this.timelines.set(sessionId, timeline);
    const metadata = this.sessions.get(sessionId);
    if (metadata) {
      metadata.eventCount = timeline.length;
      metadata.updatedAtMs = Date.now();
    }
  }

  updateSessionMetadata(sessionId: string, updates: JsonRecord): void {
    const metadata = this.sessions.get(sessionId);
    if (!metadata) return;
    this.sessions.set(sessionId, {
      ...metadata,
      ...updates,
      sessionId,
      updatedAtMs: Date.now()
    });
  }

  private handleMessage(socket: WebSocketRoute, raw: string): void {
    let message: JsonRecord;
    try {
      message = JSON.parse(raw) as JsonRecord;
    } catch {
      return;
    }
    if (
      (message.type !== "request" && typeof message.method !== "string") ||
      (typeof message.id !== "number" && typeof message.id !== "string")
    ) return;

    const request: DaemonRequest = {
      id: message.id,
      method: String(message.method ?? ""),
      params: typeof message.params === "object" && message.params !== null
        ? (message.params as JsonRecord)
        : {}
    };
    this.record(request);

    try {
      const delayedFailure = this.takeResponseFailureDelay(request);
      if (delayedFailure) {
        setTimeout(() => socket.send(this.failure(request.id, delayedFailure.error)), delayedFailure.ms);
        return;
      }
      const outbound = this.response(request.id, this.dispatch(request));
      const delay = this.takeResponseDelay(request);
      if (delay === null) {
        socket.send(outbound);
      } else {
        setTimeout(() => socket.send(outbound), delay);
      }
    } catch (error) {
      socket.send(this.failure(request.id, String(error)));
    }
  }

  private response(id: number | string, result: unknown): string {
    return this.protocol === "real"
      ? JSON.stringify({ id: String(id), result })
      : response(id, result);
  }

  private failure(id: number | string, error: string): string {
    return this.protocol === "real"
      ? JSON.stringify({ id: String(id), error: { code: "fixture-error", message: error } })
      : failure(id, error);
  }

  private takeResponseDelay(request: DaemonRequest): number | null {
    const index = this.responseDelays.findIndex(
      (delay) => delay.method === request.method && delay.predicate(request)
    );
    if (index === -1) return null;
    const [delay] = this.responseDelays.splice(index, 1);
    return delay.ms;
  }

  private takeResponseFailureDelay(request: DaemonRequest): ResponseFailureDelay | null {
    const index = this.responseFailureDelays.findIndex(
      (delay) => delay.method === request.method && delay.predicate(request)
    );
    if (index === -1) return null;
    const [delay] = this.responseFailureDelays.splice(index, 1);
    return delay;
  }

  private record(request: DaemonRequest): void {
    this.requests.push(request);
    for (let index = 0; index < this.waiters.length; index += 1) {
      const waiter = this.waiters[index];
      if (waiter.method === request.method && waiter.predicate(request)) {
        this.waiters.splice(index, 1);
        waiter.resolve(request);
        return;
      }
    }
  }

  private dispatch(request: DaemonRequest): unknown {
    this.throwQueuedFailure(request.method);
    switch (request.method) {
      case "default_workspace":
        return { cwd: this.workspaceRoot, workspaceRoot: this.workspaceRoot };
      case "load_settings_snapshot":
        return this.settingsSnapshot();
      case "login_with_oauth":
        return this.loginProvider(request.params, "oauth");
      case "login_with_api_key":
        return this.loginProvider(request.params, "api_key");
      case "logout_provider":
        return this.logoutProvider(request.params);
      case "list_external_credentials":
        return this.externalCredentials;
      case "import_external_credential":
        return this.importExternalCredential(request.params);
      case "load_desktop_pins":
        return this.desktopPins;
      case "set_desktop_pin":
        return this.setDesktopPin(request.params);
      case "list_grouped_sessions":
        return this.groupedSessions();
      case "load_session_detail":
        return this.sessionDetail(String(request.params.sessionId ?? session.sessionId));
      case "rename_session":
        return this.renameSession(request.params);
      case "create_session":
        return this.createSession(request.params);
      case "run_agent_turn":
        return this.runAgentTurn(request.params);
      case "cancel_turn": {
        const turnId = String(request.params.turnId ?? "");
        if (this.activeTurnIds.has(turnId)) {
          return { ok: true };
        }
        return { ok: false, error: "turn not found" };
      }
      case "resolve_permission":
      case "resolve_user_question":
        return {};
      case "list_provider_models":
        return {
          providerId: String(request.params.providerId ?? "codex"),
          models: this.modelsForProvider(String(request.params.providerId ?? "codex"))
        };
      case "update_config":
        return this.updateConfig(request.params);
      case "list_permissions":
        return this.permissions;
      case "save_permissions":
        return this.savePermissions(request.params);
      case "list_mcp_servers":
        return { servers: this.mcpServers };
      case "add_mcp_server":
        return this.addMcpServer(request.params);
      case "pty_list":
        return this.ptyState(String(request.params.sessionId ?? session.sessionId));
      case "pty_open":
        return this.openPty(request.params);
      case "pty_focus":
        return this.focusPty(String(request.params.ptyId ?? ""));
      case "pty_replay":
        return { chunks: [] };
      case "pty_resize":
      case "pty_write":
        return {};
      case "pty_close":
        return this.closePty(String(request.params.ptyId ?? ""));
      case "browser_agent":
        return this.browserAgent(request.params);
      case "browser_open":
        return this.openBrowser(request.params);
      case "browser_navigate":
        return this.navigateBrowser(request.params);
      case "browser_reload":
      case "browser_history":
      case "browser_input":
        return {};
      case "browser_resize":
        return this.resizeBrowser(request.params);
      case "browser_cursor":
        return { cursor: "text" };
      case "browser_copy_selection":
        return { text: "selected fixture text", copiedFrom: String(request.params.sessionId ?? "") };
      case "browser_close":
        return {};
      case "browser_recording":
        return { frames: this.browserRecordings.get(String(request.params.sessionId ?? "")) ?? [] };
      case "workflow_list":
        return this.workflowListResponse();
      case "workflow_save":
        return this.saveWorkflow(request.params);
      case "workflow_toggle":
        return this.toggleWorkflow(request.params);
      case "list_dir":
        return this.listDir(request.params);
      case "load_file_tabs":
        return this.fileTabsState ?? { tabs: fileTabs, activePath: fileTabs[0].path };
      case "save_file_tabs":
        return { tabs: request.params.tabs ?? [], activePath: request.params.activePath ?? null };
      case "read_file":
        return this.readFile(request.params);
      case "write_file":
        return this.writeFile(request.params);
      case "lsp_inspect":
        return this.lspInspect(request.params);
      case "fs_watch":
        return { watchId: String(request.params.watchId ?? "watch-fixture") };
      case "fs_unwatch":
        return {};
      default:
        throw new Error(`Unhandled fake daemon method: ${request.method}`);
    }
  }

  private workflowListResponse(): JsonRecord {
    return {
      workflows: this.workflowSnapshot.workflows.map((workflow) => ({ ...workflow })),
      runs: this.workflowSnapshot.runs.map((run) => ({ ...run })),
      connectors: this.workflowSnapshot.connectors?.map((connector) => ({ ...connector })) ?? [],
      connections: this.workflowSnapshot.connections?.map((connection) => ({ ...connection })) ?? [],
      connector_error: this.workflowSnapshot.connector_error ?? null,
      workflow_bindings: this.workflowSnapshot.workflow_bindings?.map((binding) => ({ ...binding })) ?? [],
      workflow_binding_error: this.workflowSnapshot.workflow_binding_error ?? null,
      monitor_tasks: this.workflowSnapshot.monitor_tasks?.map((task) => ({ ...task })) ?? [],
      monitor_task_error: this.workflowSnapshot.monitor_task_error ?? null
    };
  }

  private saveWorkflow(params: JsonRecord): JsonRecord {
    const workflow = params.workflow as JsonRecord | undefined;
    const slug = String(workflow?.slug ?? "");
    if (!workflow || !slug) throw new Error("missing workflow");
    this.workflowSnapshot = {
      ...this.workflowSnapshot,
      workflows: [
        ...this.workflowSnapshot.workflows.filter((candidate) => candidate.slug !== slug),
        { ...workflow }
      ].sort((a, b) => String(a.slug ?? "").localeCompare(String(b.slug ?? "")))
    };
    return this.workflowListResponse();
  }

  private toggleWorkflow(params: JsonRecord): JsonRecord {
    const slug = String(params.slug ?? "");
    const enabled = Boolean(params.enabled);
    if (!slug) throw new Error("missing workflow slug");
    let matched = false;
    this.workflowSnapshot = {
      ...this.workflowSnapshot,
      workflows: this.workflowSnapshot.workflows.map((workflow) => {
        if (workflow.slug !== slug) return workflow;
        matched = true;
        return { ...workflow, enabled };
      }),
      workflow_bindings: this.workflowSnapshot.workflow_bindings?.map((binding) => {
        if (binding.slug !== slug) return binding;
        matched = true;
        return { ...binding, enabled, status: enabled ? "enabled" : "paused" };
      })
    };
    if (!matched) throw new Error(`workflow ${slug} not found`);
    return this.workflowListResponse();
  }

  private throwQueuedFailure(method: string): void {
    const failures = this.methodFailures.get(method);
    if (!failures || failures.length === 0) return;
    const [error, ...rest] = failures;
    if (rest.length === 0) {
      this.methodFailures.delete(method);
    } else {
      this.methodFailures.set(method, rest);
    }
    throw new Error(error);
  }

  private modelsForProvider(providerId: string): JsonRecord[] {
    return this.providerModels[providerId] ?? [
      {
        id: "test-model",
        displayName: "Test model",
        provider: providerId,
        api: "openai-responses",
        contextWindow: 128000,
        maxOutputTokens: 4096,
        supportsReasoning: true,
        thinkingOptions: [
          {
            id: "low",
            label: "Low",
            description: "Use low reasoning effort for this turn.",
            isDefault: true
          },
          {
            id: "high",
            label: "High",
            description: "Use high reasoning effort for this turn.",
            isDefault: false
          }
        ],
        defaultThinkingOptionId: "low",
        isDefault: true
      }
    ];
  }

  private createSession(params: JsonRecord): JsonRecord {
    const sessionId = `session-created-${this.sessions.size + 1}`;
    const providerId = typeof params.providerId === "string" ? params.providerId : "codex";
    const modelId = typeof params.modelId === "string" ? params.modelId : "test-model";
    const cwd = typeof params.cwd === "string" ? params.cwd : session.cwd;
    const created = sessionMeta({
      sessionId,
      displayName: "New Session",
      title: "New Session",
      cwd,
      folderPath: cwd,
      createdAtMs: Date.now(),
      updatedAtMs: Date.now(),
      eventCount: 0,
      providerId,
      modelId,
      timeline: []
    });
    this.sessions.set(sessionId, created);
    this.timelines.set(sessionId, []);
    return {
      sessionId,
      displayName: created.displayName,
      generatedTitle: created.generatedTitle,
      cwd,
      createdAtMs: created.createdAtMs,
      updatedAtMs: created.updatedAtMs,
      slug: created.slug,
      providerId,
      modelId
    };
  }

  private runAgentTurn(params: JsonRecord): JsonRecord {
    const sessionId = String(params.sessionId ?? session.sessionId);
    const turnId = `turn-${sessionId}`;
    const metadata = this.sessions.get(sessionId);
    if (metadata) {
      const providerId = typeof params.providerId === "string" ? params.providerId.trim() : "";
      const modelId = typeof params.modelId === "string" ? params.modelId.trim() : "";
      if (providerId) metadata.providerId = providerId;
      if (modelId) metadata.modelId = modelId;
      if (providerId || modelId) metadata.updatedAtMs = Date.now();
      this.sessions.set(sessionId, metadata);
    }
    this.activeTurnIds.add(turnId);
    return { turnId };
  }

  private renameSession(params: JsonRecord): JsonRecord {
    const sessionId = String(params.sessionId ?? session.sessionId);
    const title = String(params.title ?? "").trim();
    const metadata = this.sessions.get(sessionId) ?? sessionMeta({ sessionId });
    metadata.displayName = title || null;
    metadata.updatedAtMs = Date.now();
    this.sessions.set(sessionId, metadata);
    if (title) {
      const timeline = this.timelines.get(sessionId) ?? defaultTimeline();
      this.timelines.set(sessionId, [
        ...timeline,
        {
          kind: "system_message",
          id: `rename-${Date.now()}`,
          text: `Session renamed to ${title}.`,
          createdAtMs: Date.now()
        }
      ]);
    }
    return this.sessionDetail(sessionId);
  }

  private updateConfig(params: JsonRecord): JsonRecord {
    if ("defaultProvider" in params) {
      this.settingsConfig.defaultProvider =
        typeof params.defaultProvider === "string" ? params.defaultProvider : null;
    }
    if ("defaultModel" in params) {
      this.settingsConfig.defaultModel =
        typeof params.defaultModel === "string" ? params.defaultModel : null;
    }
    return this.settingsSnapshot();
  }

  private loginProvider(params: JsonRecord, kind: "api_key" | "oauth"): JsonRecord {
    const providerId = String(params.providerId ?? "");
    if (!providerId) return this.settingsSnapshot();
    this.authStatuses = [
      ...this.authStatuses.filter((item) => item.providerId !== providerId),
      {
        providerId,
        kind,
        email: kind === "oauth" ? "tester@example.com" : null,
        expiresAtMs: null,
        scopes: [],
        planType: kind === "oauth" ? "test" : null,
        organizationName: null
      }
    ];
    return this.settingsSnapshot();
  }

  private logoutProvider(params: JsonRecord): JsonRecord {
    const providerId = String(params.providerId ?? "");
    this.authStatuses = this.authStatuses.filter((item) => item.providerId !== providerId);
    return this.settingsSnapshot();
  }

  private importExternalCredential(params: JsonRecord): JsonRecord {
    const providerId = String(params.providerId ?? "");
    const source = String(params.source ?? "");
    const credential = this.externalCredentials.find(
      (item) => item.providerId === providerId && item.source === source
    );
    if (credential) {
      this.authStatuses = [
        ...this.authStatuses.filter((item) => item.providerId !== providerId),
        {
          providerId,
          kind: credential.kind ?? "api_key",
          email: null,
          expiresAtMs: null,
          scopes: [],
          planType: null,
          organizationName: null
        }
      ];
    }
    return this.settingsSnapshot();
  }

  private savePermissions(params: JsonRecord): JsonRecord {
    this.permissions = {
      path: this.permissions.path,
      tools:
        typeof params.tools === "object" && params.tools !== null
          ? { ...(params.tools as JsonRecord) }
          : {}
    };
    return this.permissions;
  }

  private setDesktopPin(params: JsonRecord): JsonRecord {
    const kind = String(params.kind ?? "");
    const id = String(params.id ?? "");
    const pinned = params.pinned === true;
    const key = kind === "workspace" ? "pinnedWorkspacePaths" : "pinnedAgentIds";
    const current = Array.isArray(this.desktopPins[key])
      ? (this.desktopPins[key] as unknown[]).filter((value): value is string => typeof value === "string")
      : [];
    const next = current.filter((value) => value !== id);
    this.desktopPins = {
      ...this.desktopPins,
      [key]: pinned ? [id, ...next] : next
    };
    this.emit("desktop:pins:changed", this.desktopPins);
    return this.desktopPins;
  }

  private addMcpServer(params: JsonRecord): JsonRecord {
    const id = String(params.id ?? "");
    const server = {
      id,
      displayName: String(params.displayName ?? id),
      description: String(params.description ?? ""),
      transport: String(params.transport ?? "stdio"),
      endpoint: String(params.endpoint ?? ""),
      target: String(params.target ?? ""),
      sourceKind: String(params.scope ?? "local"),
      sourcePath: `/tmp/puffer/.puffer/mcp_servers/${id}.json`
    };
    this.mcpServers = [
      ...this.mcpServers.filter((item) => item.id !== id),
      server
    ];
    return { servers: this.mcpServers };
  }

  private settingsSnapshot(): JsonRecord {
    return {
      workspaceRoot: this.workspaceRoot,
      workspaceConfigFile: `${this.workspaceRoot}/.puffer/config.json`,
      userConfigFile: "/tmp/home/.puffer/config.json",
      authStoreFile: `${this.workspaceRoot}/.puffer/auth.json`,
      builtinResourcesDir: `${this.workspaceRoot}/resources`,
      config: {
        appName: "Puffer Code",
        defaultProvider: this.settingsConfig.defaultProvider,
        defaultModel: this.settingsConfig.defaultModel,
        openaiBaseUrl: null,
        theme: "system",
        mascotId: "puffer",
        mascotDisplayName: "Puffer",
        mascotEnabled: true,
        uiNoAltScreen: false,
        uiTmuxGoldenMode: false
      },
      resources: {
        providers: 2,
        tools: 1,
        agents: 0,
        prompts: 0,
        hooks: 0,
        skills: 0,
        mascots: 1,
        plugins: 0,
        mcpServers: 1,
        ides: 0
      },
      sessions: { totalSessions: 1, folderGroups: 1 },
      auth: this.authStatuses,
      providers: this.providerSummaries ?? [
        {
          id: "codex",
          displayName: "Codex",
          baseUrl: "",
          defaultApi: "responses",
          modelCount: 1,
          authModes: ["oauth"],
          sourceKind: "test",
          sourcePath: null
        },
        {
          id: "anthropic",
          displayName: "Anthropic",
          baseUrl: "",
          defaultApi: "anthropic-messages",
          modelCount: 1,
          authModes: ["api_key"],
          sourceKind: "test",
          sourcePath: null
        }
      ]
    };
  }

  private groupedSessions(): JsonRecord[] {
    const groups = new Map<string, JsonRecord>();
    for (const metadata of this.sessions.values()) {
      if (this.groupedSessionFilter && !this.groupedSessionFilter(metadata)) continue;
      const folderPath = String(metadata.folderPath ?? metadata.cwd ?? "/tmp/puffer");
      const group = groups.get(folderPath) ?? {
        folderId: folderPath,
        folderLabel: folderPath.split("/").filter(Boolean).at(-1) ?? folderPath,
        folderPath,
        sessionCount: 0,
        sessions: []
      };
      (group.sessions as JsonRecord[]).push(metadata);
      group.sessionCount = (group.sessions as JsonRecord[]).length;
      groups.set(folderPath, group);
    }
    for (const group of groups.values()) {
      (group.sessions as JsonRecord[]).sort(
        (left, right) => Number(right.updatedAtMs ?? 0) - Number(left.updatedAtMs ?? 0)
      );
    }
    return Array.from(groups.values());
  }

  private sessionDetail(sessionId: string): JsonRecord {
    const metadata = this.sessions.get(sessionId) ?? session;
    const timeline = this.timelines.get(sessionId) ?? defaultTimeline();
    const detail = this.details.get(sessionId) ?? {
      latestDiff: null,
      diffHistory: [],
      repoStatus: null,
      agentDiff: { files: [], entries: [] },
      divergence: { agentOnly: [], gitOnly: [], agentTotal: 0, gitTotal: 0 }
    };
    return {
      ...metadata,
      eventCount: timeline.length,
      timeline,
      latestDiff: detail.latestDiff,
      diffHistory: detail.diffHistory,
      repoStatus: {
        sessionId,
        cwd: String(metadata.cwd ?? session.cwd),
        repoRoot: String(metadata.folderPath ?? session.folderPath),
        branch: "codex/desktop-gui-e2e-fixes",
        headSha: "abcdef0",
        isClean: true,
        statusLines: [],
        hasGh: false,
        ghAuthenticated: false,
        canCreatePullRequest: false,
        canMergePullRequest: false,
        createPullRequestReason: "gh unavailable in tests",
        mergePullRequestReason: "gh unavailable in tests",
        openPullRequest: null,
        warnings: [],
        ...(detail.repoStatus ?? {})
      },
      agentDiff: detail.agentDiff,
      divergence: detail.divergence
    };
  }

  private browserAgent(params: JsonRecord): unknown {
    const action = String(params.action ?? "list");
    const sessionId = String(params.sessionId ?? session.sessionId);
    if (action === "list") return this.tabState(sessionId);
    if (action === "focus") {
      const tabId = String(params.tabId ?? "tab-1");
      const set = this.tabSet(sessionId);
      set.activeTabId = tabId;
      this.refreshActiveFlags(set);
      return set.tabs.find((tab) => tab.tabId === tabId) ?? browserTabInfo(tabId);
    }
    if (action === "close") {
      const tabId = String(params.tabId ?? "tab-1");
      const set = this.tabSet(sessionId);
      set.tabs = set.tabs.filter((tab) => tab.tabId !== tabId);
      set.activeTabId = (set.tabs[0]?.tabId as string | undefined) ?? null;
      this.refreshActiveFlags(set);
      return this.tabState(sessionId);
    }
    if (action === "open") {
      const set = this.tabSet(sessionId);
      if (typeof params.tabId !== "string" && set.tabs.length > 0) {
        return set.tabs.find((tab) => tab.active === true) ?? set.tabs[0];
      }
      const tabId = typeof params.tabId === "string" ? params.tabId : `t${this.nextTab++}`;
      return this.upsertTab(sessionId, browserTabInfo(tabId, String(params.url ?? "about:blank")));
    }
    throw new Error(`Unhandled browser_agent action: ${action}`);
  }

  private openBrowser(params: JsonRecord): unknown {
    const sessionId = String(params.sessionId ?? "");
    const url = String(params.url ?? "about:blank");
    this.recordBrowserOpen(sessionId, url);
    if (this.emitBrowserOpenFrame) this.emitBrowserFrame(sessionId, "frame-1");
    return browserState(url);
  }

  private resizeBrowser(params: JsonRecord): unknown {
    const sessionId = String(params.sessionId ?? "");
    if (this.emitBrowserResizeFrame) this.emitBrowserFrame(sessionId, "frame-resize");
    return {};
  }

  private emitBrowserFrame(sessionId: string, frameId: string): void {
    queueMicrotask(() => {
      this.emit(`browser:${sessionId}:frame`, {
        frameId,
        mimeType: "image/png",
        encoding: "base64",
        data: ONE_PIXEL_PNG,
        width: 960,
        height: 720
      });
    });
  }

  private tabSet(sessionId: string): TabSet {
    const existing = this.browserTabs.get(sessionId);
    if (existing) return existing;
    const created: TabSet = { activeTabId: null, tabs: [] };
    this.browserTabs.set(sessionId, created);
    return created;
  }

  private tabState(sessionId: string): JsonRecord {
    const set = this.tabSet(sessionId);
    return { activeTabId: set.activeTabId, tabs: set.tabs };
  }

  private upsertTab(sessionId: string, tab: JsonRecord): JsonRecord {
    const set = this.tabSet(sessionId);
    set.tabs = [...set.tabs.filter((item) => item.tabId !== tab.tabId), tab];
    set.activeTabId = String(tab.tabId);
    this.refreshActiveFlags(set);
    return tab;
  }

  private refreshActiveFlags(set: TabSet): void {
    set.tabs = set.tabs.map((tab) => ({
      ...tab,
      active: tab.tabId === set.activeTabId
    }));
  }

  private recordBrowserOpen(backendSessionId: string, url: string): void {
    const marker = ":browser:";
    const markerIndex = backendSessionId.indexOf(marker);
    if (markerIndex === -1) return;
    const rootSessionId = backendSessionId.slice(0, markerIndex);
    const tabId = backendSessionId.slice(markerIndex + marker.length);
    if (!rootSessionId || !tabId) return;
    this.upsertTab(rootSessionId, browserTabInfo(tabId, url));
  }

  private ptySet(sessionId: string): PtySet {
    const existing = this.ptys.get(sessionId);
    if (existing) return existing;
    const created: PtySet = { initialized: false, activePtyId: null, tabs: [] };
    this.ptys.set(sessionId, created);
    return created;
  }

  private ptyState(sessionId: string): JsonRecord {
    const set = this.ptySet(sessionId);
    this.refreshPtyActiveFlags(set);
    return {
      initialized: set.initialized,
      tabs: set.tabs
    };
  }

  private openPty(params: JsonRecord): JsonRecord {
    const sessionId = String(params.sessionId ?? session.sessionId);
    const set = this.ptySet(sessionId);
    const ptyId = `pty-${this.nextPty++}`;
    const title = String(params.title ?? `Terminal ${set.tabs.length + 1}`);
    const tab = {
      ptyId,
      sessionId,
      title,
      cwd: String(params.cwd ?? "/tmp/puffer"),
      cols: Number(params.cols ?? 80),
      rows: Number(params.rows ?? 24),
      createdAtMs: Date.now(),
      active: true
    };
    set.initialized = true;
    set.activePtyId = ptyId;
    set.tabs = [...set.tabs, tab];
    this.refreshPtyActiveFlags(set);
    return { ptyId };
  }

  private focusPty(ptyId: string): JsonRecord {
    for (const set of this.ptys.values()) {
      if (set.tabs.some((tab) => tab.ptyId === ptyId)) {
        set.activePtyId = ptyId;
        this.refreshPtyActiveFlags(set);
        break;
      }
    }
    return {};
  }

  private closePty(ptyId: string): JsonRecord {
    for (const set of this.ptys.values()) {
      if (!set.tabs.some((tab) => tab.ptyId === ptyId)) continue;
      set.tabs = set.tabs.filter((tab) => tab.ptyId !== ptyId);
      if (set.activePtyId === ptyId) {
        set.activePtyId = set.tabs.length > 0 ? String(set.tabs[0].ptyId) : null;
      }
      this.refreshPtyActiveFlags(set);
      break;
    }
    return {};
  }

  private refreshPtyActiveFlags(set: PtySet): void {
    if (!set.activePtyId && set.tabs.length > 0) {
      set.activePtyId = String(set.tabs[0].ptyId);
    }
    set.tabs = set.tabs.map((tab) => ({
      ...tab,
      active: tab.ptyId === set.activePtyId
    }));
  }

  private navigateBrowser(params: JsonRecord): unknown {
    const sessionId = String(params.sessionId ?? "");
    const rawUrl = String(params.url ?? "about:blank");
    const url = rawUrl.includes("://") || rawUrl === "about:blank" ? rawUrl : `https://${rawUrl}`;
    queueMicrotask(() => {
      this.emit(`browser:${sessionId}:state`, browserState(url));
    });
    return {};
  }

  private listDir(params: JsonRecord): JsonRecord {
    const path = String(params.path ?? "");
    const entries = new Map<string, JsonRecord>();
    if (path === "/tmp/puffer") {
      entries.set("src", { name: "src", kind: "directory", size: 0, modifiedMs: now });
    }
    if (path === "/tmp/puffer/src") {
      entries.set("main.rs", { name: "main.rs", kind: "file", size: 42, modifiedMs: now });
      entries.set("lib.rs", { name: "lib.rs", kind: "file", size: 41, modifiedMs: now });
    }
    for (const [filePath, value] of this.files) {
      const parent = parentPath(filePath);
      if (parent !== path) continue;
      const name = filePath.split("/").pop();
      if (!name) continue;
      entries.set(name, {
        name,
        kind: "file",
        size: fakeFileSize(value),
        modifiedMs: now
      });
    }
    return {
      entries: Array.from(entries.values()).sort((left, right) => {
        const leftKind = String(left.kind);
        const rightKind = String(right.kind);
        if (leftKind !== rightKind) return leftKind === "directory" ? -1 : 1;
        return String(left.name).localeCompare(String(right.name));
      })
    };
  }

  private readFile(params: JsonRecord): JsonRecord {
    const path = String(params.path ?? "");
    const content = this.files.get(path) ?? defaultFileContent(path);
    this.files.set(path, content);
    const resultPath = this.canonicalFilePaths.get(path) ?? path;
    const maxBytesValue = Number(params.maxBytes ?? params.max_bytes ?? Number.POSITIVE_INFINITY);
    const maxBytes = Number.isFinite(maxBytesValue) && maxBytesValue >= 0
      ? Math.floor(maxBytesValue)
      : Number.POSITIVE_INFINITY;
    if (typeof content !== "string") {
      const bytes = Buffer.from(content.content, "base64");
      const visible = maxBytes === Number.POSITIVE_INFINITY ? bytes : bytes.subarray(0, maxBytes);
      return {
        path: resultPath,
        encoding: content.encoding,
        content: visible.toString("base64"),
        size: content.size,
        truncated: visible.length < bytes.length,
        ...(content.textPreview ? { textPreview: content.textPreview } : {}),
        ...(content.htmlPreview ? { htmlPreview: content.htmlPreview } : {})
      };
    }
    const bytes = Buffer.from(content, "utf8");
    const visible = maxBytes === Number.POSITIVE_INFINITY ? bytes : bytes.subarray(0, maxBytes);
    return {
      path: resultPath,
      encoding: "utf8",
      content: visible.toString("utf8"),
      size: bytes.length,
      truncated: visible.length < bytes.length
    };
  }

  private writeFile(params: JsonRecord): JsonRecord {
    const path = String(params.path ?? "");
    const content = String(params.content ?? "");
    this.files.set(path, content);
    return {
      path,
      encoding: "utf8",
      content,
      size: content.length,
      truncated: false
    };
  }

  private lspInspect(params: JsonRecord): JsonRecord {
    const path = String(params.path ?? "/tmp/puffer/src/lib.rs");
    const cwd = String(params.cwd ?? "/tmp/puffer");
    const line = Number(params.line ?? 0);
    const character = Number(params.character ?? 0);
    const symbol = path.endsWith("main.rs") ? "main" : "fixture";
    const defaultLocation = path.endsWith("main.rs") ? "src/main.rs:1:4" : "src/lib.rs:1:8";
    const location = this.lspLocations.get(path) ?? defaultLocation;
    return {
      path,
      cwd,
      line,
      character,
      operations: {
        hover: {
          operation: "hover",
          filePath: path,
          result: `${symbol}() -> demo value`
        },
        goToDefinition: {
          operation: "goToDefinition",
          filePath: path,
          result: `- ${location}`,
          resultCount: 1,
          fileCount: 1
        },
        findReferences: {
          operation: "findReferences",
          filePath: path,
          result: `${location.split(":")[0]}:\n  - line ${location.split(":")[1]}:${location.split(":")[2]}`,
          resultCount: 1,
          fileCount: 1
        }
      }
    };
  }
}
