import type {
  AuthProviderStatus,
  DesktopPreferences,
  DiffSnapshot,
  FolderGroup,
  ProviderSummary,
  RepoActionResult,
  RepoStatus,
  SessionDetail,
  SessionListItem,
  SettingsSnapshot,
  TimelineItem
} from "./types";

const now = Date.now();

const sessionA: SessionListItem = {
  id: "session-a",
  displayName: "Claude parity sweep",
  generatedTitle: null,
  title: "Claude parity sweep",
  cwd: "/home/c/puffer",
  folderPath: "/home/c/puffer",
  createdAtMs: now - 6_800_000,
  updatedAtMs: now - 42_000,
  eventCount: 52,
  activityStatus: "running",
  slug: "claude-parity-sweep",
  tags: ["parity", "tools"],
  note: "Tight parity work against Claude Code",
  parentSessionId: null,
  providerId: "claude",
  modelId: "sonnet"
};

const sessionB: SessionListItem = {
  id: "session-b",
  displayName: "Desktop shell",
  generatedTitle: null,
  title: "Desktop shell",
  cwd: "/home/c/puffer/.worktree/tauri-desktop-ui",
  folderPath: "/home/c/puffer",
  createdAtMs: now - 12_500_000,
  updatedAtMs: now - 130_000,
  eventCount: 31,
  activityStatus: "awaiting",
  slug: "desktop-shell",
  tags: ["desktop", "ui"],
  note: "Parallel Tauri 2 app exploration",
  parentSessionId: null,
  providerId: "codex",
  modelId: null
};

const sessionC: SessionListItem = {
  id: "session-c",
  displayName: "Python LSP validation",
  generatedTitle: null,
  title: "Python LSP validation",
  cwd: "/home/c/sample-python",
  folderPath: "/home/c/sample-python",
  createdAtMs: now - 20_000_000,
  updatedAtMs: now - 600_000,
  eventCount: 18,
  activityStatus: "idle",
  slug: "python-lsp-validation",
  tags: ["lsp", "python"],
  note: "Real pyright end-to-end check",
  parentSessionId: null,
  providerId: "puffer",
  modelId: "default"
};

const sessionADiff: DiffSnapshot = {
  id: "diff-a",
  source: "session_history",
  title: "Claude parity permission regression sweep",
  command: "/diff",
  status: "4 files changed, 39 insertions(+), 12 deletions(-)",
  unstagedDiffstat: "crates/puffer-core/runtime/*",
  stagedDiffstat: "",
  patch: [
    "@@ -44,7 +44,10 @@",
    " pub(crate) fn evaluate_permission(...) {",
    "-    return PermissionDecision::Deny;",
    "+    if context.plan_mode() {",
    "+        return PermissionDecision::Ask;",
    "+    }",
    "+",
    "+    return PermissionDecision::Deny;",
    " }"
  ].join("\n")
};

export const mockFolders: FolderGroup[] = [
  {
    id: "/home/c/puffer",
    label: "puffer",
    path: "/home/c/puffer",
    sessionCount: 2,
    sessions: [sessionA, sessionB],
    tags: []
  },
  {
    id: "/home/c/sample-python",
    label: "sample-python",
    path: "/home/c/sample-python",
    sessionCount: 1,
    sessions: [sessionC],
    tags: []
  }
];

export const mockRepoStatus: RepoStatus = {
  sessionId: sessionB.id,
  cwd: sessionB.cwd,
  isGitRepo: true,
  repoRoot: "/home/c/puffer",
  branch: "feature/tauri-desktop-ui",
  headSha: "3ca43a4",
  isClean: false,
  hasUncommittedChanges: true,
  statusLines: [" M apps/puffer-desktop/src/App.svelte", "?? apps/puffer-desktop/src-tauri/"],
  ghAvailable: true,
  ghAuthenticated: true,
  canCreatePr: true,
  canMergePr: true,
  createPrReason: null,
  mergePrReason: null,
  pullRequest: {
    number: 42,
    title: "Add Tauri desktop UI",
    url: "https://github.com/berabuddies/puffer/pull/42",
    state: "OPEN",
    mergeStateStatus: "CLEAN",
    isDraft: false,
    baseRefName: "master",
    headRefName: "feature/tauri-desktop-ui"
  },
  warnings: []
};

const latestDiff: DiffSnapshot = {
  id: "diff-latest",
  source: "session_history",
  title: "Desktop UI shell and inspector layout",
  command: "/review",
  status: "6 files changed, 214 insertions(+), 0 deletions(-)",
  unstagedDiffstat: "apps/puffer-desktop/src/*",
  stagedDiffstat: "",
  patch: [
    "@@ -0,0 +1,62 @@",
    "+<script lang=\"ts\">",
    "+  let inspectorTab = \"latest-diff\";",
    "+  let inspectorOpen = true;",
    "+  let composerValue = \"\";",
    "+</script>",
    "+",
    "+<main class=\"shell\">",
    "+  <SessionSidebar />",
    "+  <ConversationPane />",
    "+  <InspectorPane />",
    "+</main>",
    "+",
    "+<style>",
    "+  .shell {",
    "+    display: grid;",
    "+    grid-template-columns: 280px minmax(0, 1fr) 420px;",
    "+    min-height: 100vh;",
    "+  }",
    "+",
    "+  .composer {",
    "+    position: sticky;",
    "+    bottom: 0;",
    "+    padding: 1rem 1.2rem;",
    "+    background: rgba(255, 255, 255, 0.94);",
    "+    box-shadow: 0 -1px 0 rgba(30, 35, 40, 0.08);",
    "+  }",
    "+",
    "+  .diff-pane {",
    "+    overflow: auto;",
    "+    border-left: 1px solid rgba(30, 35, 40, 0.08);",
    "+  }",
    "+</style>"
  ].join("\n")
};

const olderDiff: DiffSnapshot = {
  id: "diff-older",
  source: "session_history",
  title: "Runtime session grouping API",
  command: "/diff",
  status: "3 files changed, 121 insertions(+), 0 deletions(-)",
  unstagedDiffstat: "",
  stagedDiffstat: "apps/puffer-desktop/src-tauri/src/session_data.rs",
  patch: [
    "@@ -0,0 +1,80 @@",
    "+#[tauri::command]",
    "+pub fn list_grouped_sessions() -> Result<Vec<FolderGroupDto>, String> {",
    "+    load_groups()",
    "+}"
  ].join("\n")
};

const sessionATimeline: TimelineItem[] = [
  {
    id: "a-msg-1",
    kind: "user",
    title: "User message",
    summary: "Permission prompts still diverge from Claude Code.",
    body: "Permission prompts still diverge from Claude Code when a tool call happens in plan mode.",
    meta: ["Turn 1"]
  },
  {
    id: "a-msg-2",
    kind: "assistant",
    title: "Assistant response",
    summary: "I’m comparing the runtime path against Claude-compatible behavior.",
    body: [
      "I’m comparing the runtime path against Claude-compatible behavior.",
      "",
      "The focus is on:",
      "- permission escalation rules",
      "- prompt wording",
      "- whether plan mode should ask or deny by default"
    ].join("\n"),
    meta: ["Turn 1"]
  },
  {
    id: "a-tool-1",
    kind: "tool",
    title: "Tool call: Read",
    summary: "Read the permission runtime and prompt renderer.",
    body: "Inspected the runtime permission context and the prompt text generators.",
    meta: ["Read", "ok"],
    toolName: "Read",
    status: "ok",
    input: "{\"paths\":[\"crates/puffer-core/permissions.rs\",\"crates/puffer-tui/src/approval_overlay.rs\"]}",
    output: [
      "permissions.rs: enforce_tool_call() still returns Deny in one branch",
      "approval_overlay.rs: prompt text differs from Claude wording"
    ].join("\n"),
    inputJson: {
      paths: [
        "crates/puffer-core/permissions.rs",
        "crates/puffer-tui/src/approval_overlay.rs"
      ]
    }
  },
  {
    id: "a-diff-1",
    kind: "diff",
    title: sessionADiff.title,
    summary: sessionADiff.status,
    body: sessionADiff.patch,
    meta: [sessionADiff.command],
    diff: sessionADiff
  }
];

const timeline: TimelineItem[] = [
  {
    id: "msg-1",
    kind: "user",
    title: "User message",
    summary: "Implement a Tauri UI for this.",
    body: "Implement a Tauri UI for this. It should feel minimal and production-ready.",
    meta: ["Turn 1"]
  },
  {
    id: "msg-2",
    kind: "assistant",
    title: "Assistant response",
    summary: "Scaffolding a parallel desktop app with shared session data.",
    body: [
      "I’m scaffolding the desktop app as a parallel Tauri 2 UI.",
      "",
      "The frontend will browse folders and sessions on the left, and keep the conversation with diff inspection on the right."
    ].join("\n"),
    meta: ["Turn 1"]
  },
  {
    id: "cmd-1",
    kind: "command",
    title: "/review",
    summary: "Captured a new git diff checkpoint.",
    body: "/review",
    meta: ["slash command"]
  },
  {
    id: "tool-1",
    kind: "tool",
    title: "Tool call: Bash",
    summary: "Checked local GitHub CLI availability.",
    body: "Executed `gh --version` to determine whether one-click PR actions can be enabled.",
    meta: ["Bash", "ok"],
    toolName: "Bash",
    status: "ok",
    input: "{\"command\":\"gh --version\"}",
    output: [
      "gh version 2.89.0",
      "https://github.com/cli/cli/releases/tag/v2.89.0",
      "authenticated: yes",
      "active account: fuzz-land"
    ].join("\n"),
    inputJson: { command: "gh --version" }
  },
  {
    id: "tool-2",
    kind: "tool",
    title: "Tool call: Read",
    summary: "Loaded the transcript and session metadata from the local store.",
    body: "Read the most recent session transcript and normalized the event stream for desktop rendering.",
    meta: ["Read", "ok"],
    toolName: "Read",
    status: "ok",
    input: "{\"path\":\"~/.puffer/sessions/session-b.json\"}",
    output: [
      "{",
      "  \"events\": 31,",
      "  \"latest_diff\": \"Desktop UI shell and inspector layout\",",
      "  \"permissions\": 1",
      "}"
    ].join("\n"),
    inputJson: { path: "~/.puffer/sessions/session-b.json" }
  },
  {
    id: "perm-1",
    kind: "permission",
    title: "Permission request",
    summary: "Config write requires approval before the turn can continue.",
    body: [
      "Tool: Config",
      "Scope: workspace",
      "Reason: workspace permission rule requires approval"
    ].join("\n"),
    meta: ["required"],
    toolName: "Config",
    status: "required",
    permissionDialog: {
      state: "required",
      reason: "workspace permission rule requires approval",
      summary: "Setting: theme",
      inputText: "{\"setting\":\"theme\",\"value\":\"light\"}",
      toolName: "Config",
      choices: ["Approve once", "Always allow", "Deny"]
    },
    scopeLabel: "workspace",
    choices: ["Approve once", "Always allow", "Deny"]
  },
  {
    id: "diff-1",
    kind: "diff",
    title: latestDiff.title,
    summary: latestDiff.status,
    body: latestDiff.patch,
    meta: [latestDiff.command],
    diff: latestDiff
  },
  {
    id: "msg-3",
    kind: "assistant",
    title: "Assistant response",
    summary: "Introduced a three-pane desktop layout with PR actions.",
    body: [
      "The desktop shell now includes:",
      "- folder and session navigation",
      "- a structured conversation timeline",
      "- an inspector for latest diff, history, and tool details",
      "- one-click PR and merge actions"
    ].join("\n"),
    meta: ["Turn 2"]
  },
  {
    id: "msg-4",
    kind: "assistant",
    title: "Assistant response",
    summary: "The next pass should remove dashboard framing and make the transcript primary.",
    body: [
      "The next pass should remove dashboard framing and make the transcript primary.",
      "",
      "What matters most in this view is:",
      "- the current conversation",
      "- what tools actually did",
      "- the diff at the moment the work changed",
      "",
      "That means fewer status boxes and stronger transcript readability."
    ].join("\n"),
    meta: ["Turn 3"]
  }
];

// Mock agent-diff payloads — three example edits the design uses to
// fill out the Agent / Divergence sub-tabs in screenshot reviews.
const mockAgentDiffB = {
  files: [
    {
      path: "apps/puffer-desktop/src/lib/shell/Sidebar.svelte",
      latestKind: "replace",
      editCount: 2,
      latestSummary:
        "-  let inspectorTab = \"latest-diff\";\n+  let inspectorTab: InspectorTab = \"latest-diff\";\n"
    },
    {
      path: "apps/puffer-desktop/src/lib/components/InspectorPane.svelte",
      latestKind: "write",
      editCount: 1,
      latestSummary:
        "+<script lang=\"ts\">\n+  export let tab: InspectorTab;\n+</script>\n"
    }
  ],
  entries: []
};

const mockDivergenceB = {
  agentOnly: [],
  // The repo formatter rewrote a file the agent never touched —
  // exactly the kind of drift the divergence tab exists to surface.
  gitOnly: ["apps/puffer-desktop/src/app.css"],
  agentTotal: 2,
  gitTotal: 3
};

export const mockSessionDetail: SessionDetail = {
  session: sessionB,
  timeline,
  latestDiff,
  diffHistory: [latestDiff, olderDiff],
  repoStatus: mockRepoStatus,
  agentDiff: mockAgentDiffB,
  divergence: mockDivergenceB
};

const mockSessionDetailA: SessionDetail = {
  session: sessionA,
  timeline: sessionATimeline,
  latestDiff: sessionADiff,
  diffHistory: [sessionADiff],
  repoStatus: {
    ...mockRepoStatus,
    sessionId: sessionA.id,
    cwd: sessionA.cwd,
    branch: "fix/permission-parity",
    pullRequest: null
  },
  agentDiff: {
    files: [
      {
        path: "crates/puffer-core/runtime/permissions.rs",
        latestKind: "replace",
        editCount: 1,
        latestSummary:
          "-return PermissionDecision::Deny;\n+if context.plan_mode() {\n+  return PermissionDecision::Ask;\n+}\n+return PermissionDecision::Deny;\n"
      }
    ],
    entries: []
  },
  divergence: {
    agentOnly: [],
    gitOnly: [],
    agentTotal: 1,
    gitTotal: 1
  }
};

const mockSessionDetailC: SessionDetail = {
  session: sessionC,
  timeline: [
    {
      id: "c-msg-1",
      kind: "user",
      title: "User message",
      summary: "Validate pyright diagnostics on the sample workspace.",
      body: "Validate pyright diagnostics on the sample workspace and confirm the overlay behavior.",
      meta: ["Turn 1"]
    },
    {
      id: "c-tool-1",
      kind: "tool",
      title: "Tool call: Bash",
      summary: "Ran pyright over the sample repository.",
      body: "Executed pyright and captured diagnostics output.",
      meta: ["Bash", "ok"],
      toolName: "Bash",
      status: "ok",
      input: "{\"command\":\"pyright\"}",
      output: "0 errors, 0 warnings, 0 informations",
      inputJson: { command: "pyright" }
    }
  ],
  latestDiff: null,
  diffHistory: [],
  repoStatus: {
    ...mockRepoStatus,
    sessionId: sessionC.id,
    cwd: sessionC.cwd,
    branch: "main",
    pullRequest: null,
    hasUncommittedChanges: false,
    isClean: true,
    statusLines: []
  },
  agentDiff: { files: [], entries: [] },
  divergence: { agentOnly: [], gitOnly: [], agentTotal: 0, gitTotal: 0 }
};

export function mockSessionDetailFor(sessionId: string): SessionDetail {
  if (sessionId === sessionA.id) {
    return mockSessionDetailA;
  }
  if (sessionId === sessionC.id) {
    return mockSessionDetailC;
  }
  return mockSessionDetail;
}

export function mockCreatePrResult(): RepoActionResult {
  return {
    ok: true,
    action: "create_pull_request",
    message: "Created pull request #42.",
    repoStatus: mockRepoStatus,
    pullRequest: mockRepoStatus.pullRequest
  };
}

export function mockMergePrResult(): RepoActionResult {
  return {
    ok: true,
    action: "merge_pull_request",
    message: "Pull request merged successfully.",
    repoStatus: {
      ...mockRepoStatus,
      isClean: true,
      hasUncommittedChanges: false,
      canMergePr: false,
      mergePrReason: "No active pull request exists for the current branch.",
      pullRequest: null
    },
    pullRequest: null
  };
}

export const mockDesktopPreferences: DesktopPreferences = {
  rememberSession: true,
  rememberInspectorLayout: true,
  launchInspectorOpen: true,
  defaultInspectorTab: "latest-diff",
  defaultInspectorWidth: 50,
  remoteEnabled: false,
  remoteTarget: "",
  remoteCwd: ""
};

const mockAuth: AuthProviderStatus[] = [
  {
    providerId: "anthropic",
    kind: "oauth",
    email: "developer@example.com",
    expiresAtMs: Date.now() + 86_400_000,
    scopes: ["org:create_api_key", "user:profile"],
    planType: "max",
    organizationName: "Berabuddies"
  },
  {
    providerId: "openai",
    kind: "api_key",
    email: null,
    expiresAtMs: null,
    scopes: [],
    planType: null,
    organizationName: null
  }
];

const mockProviders: ProviderSummary[] = [
  {
    id: "anthropic",
    displayName: "Anthropic",
    baseUrl: "https://api.anthropic.com",
    defaultApi: "anthropic-messages",
    modelCount: 3,
    authModes: ["api_key", "oauth"],
    sourceKind: "resourcepack",
    sourcePath: "resources/providers/anthropic.yaml"
  },
  {
    id: "openai",
    displayName: "OpenAI",
    baseUrl: "https://api.openai.com",
    defaultApi: "openai-responses",
    modelCount: 6,
    authModes: ["api_key", "oauth"],
    sourceKind: "resourcepack",
    sourcePath: "resources/providers/openai.yaml"
  }
];

export const mockSettingsSnapshot: SettingsSnapshot = {
  workspaceRoot: "/home/c/puffer",
  workspaceConfigFile: "/home/c/puffer/.puffer/config.toml",
  userConfigFile: "/home/c/.puffer/config.toml",
  authStoreFile: "/home/c/.puffer/auth.json",
  builtinResourcesDir: "/home/c/puffer/resources",
  config: {
    appName: "Puffer Code",
    defaultProvider: "anthropic",
    defaultModel: "claude-sonnet-4-5",
    openaiBaseUrl: null,
    theme: "puffer",
    mascotId: "clawd",
    mascotDisplayName: "Clawd",
    mascotEnabled: true,
    uiNoAltScreen: false,
    uiTmuxGoldenMode: false,
    browserChromeProfile: null
  },
  resources: {
    providers: 2,
    tools: 38,
    agents: 4,
    prompts: 7,
    hooks: 0,
    skills: 1,
    mascots: 1,
    plugins: 1,
    mcpServers: 2,
    ides: 1
  },
  sessions: {
    totalSessions: 3,
    folderGroups: 2
  },
  auth: mockAuth,
  providers: mockProviders,
  browserProfiles: [
    {
      id: "Default",
      name: "Personal",
      email: "demo@example.com",
      googleAccounts: [
        {
          email: "demo@example.com",
          name: "Demo User",
          gaiaId: "demo-gaia"
        }
      ],
      path: "/Users/demo/Library/Application Support/Google/Chrome/Default",
      isLastUsed: true,
      isSelected: true
    }
  ]
};
