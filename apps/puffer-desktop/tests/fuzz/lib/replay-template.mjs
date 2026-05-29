import path from "node:path";
import { assertReplayRegistrySupported, buildReplayRegistryMetadata } from "./action-registry.mjs";

export const REPLAY_ACTION_IDS = [
  "open-workspace",
  "open-settings-providers",
  "type-search",
  "clear-search",
  "open-agent-card",
  "pin-agent",
  "open-connect-project",
  "open-workspace-picker",
  "assert-modal-initial-focus",
  "assert-modal-focus-trap",
  "emit-grouped-session-refresh",
  "emit-active-agent-change",
  "open-agent-detail",
  "open-browser-pane",
  "type-composer",
  "send-prompt",
  "rapid-send-prompt",
  "stop-turn",
  "complete-turn",
  "settle-canceled-turn",
  "switch-session",
  "emit-old-session-stream",
  "emit-permission",
  "answer-permission",
  "emit-question",
  "answer-question",
  "change-model",
  "delayed-transcript-refresh",
  "import-credential",
  "refresh-credential",
  "save-default-model",
  "save-default-model-codex",
  "save-default-model-anthropic",
  "open-new-agent",
  "switch-new-agent-provider",
  "switch-new-agent-provider-codex",
  "switch-new-agent-provider-anthropic",
  "submit-new-agent",
  "emit-auth-list-refresh",
  "emit-auth-one-provider-refresh",
  "emit-auth-none-refresh",
  "delayed-model-list",
  "type-url",
  "press-address-enter",
  "click-new-tab",
  "close-active-tab",
  "reload",
  "history",
  "switch-tab",
  "open-file",
  "edit-file",
  "save-file",
  "emit-file-restore",
  "open-terminal",
  "type-terminal",
  "close-terminal",
  "emit-late-pty-output",
  "open-settings-connectors",
  "start-connector-setup",
  "answer-connector-questions",
  "open-settings-mcp",
  "add-mcp-server",
  "emit-mcp-list-refresh",
  "late-mcp-test-result",
  "open-permissions",
  "save-permissions",
  "emit-permissions-refresh",
  "open-workflows",
  "edit-workflow-node",
  "edit-workflow-trigger",
  "emit-workflow-refresh",
  "type-page-input",
  "special-key",
  "emit-state-for-old-tab",
  "emit-empty-tab-list",
  "emit-frame-burst",
  "fail-next-browser-command",
  "hold-next-browser-response",
  "disconnect-reconnect",
  "resize-narrow",
  "resize-desktop"
];

export function selectCase(run, caseId) {
  const selected = (run.cases ?? []).find((item) => item.caseId === caseId);
  if (!selected) {
    throw new Error(`Case not found: ${caseId}`);
  }
  return selected;
}

export function buildReplayTemplate(testCase, options = {}) {
  const relativeCoverageImport = options.coverageImport ?? "./fuzz/playwright/pufferCoverage";
  const fakeDaemonImport = options.fakeDaemonImport ?? "./support/fakeDaemon";
  const daemonOptions = buildReplayDaemonOptions(testCase);
  const replayMetadata = buildReplayMetadata(testCase, daemonOptions);
  assertReplayRegistrySupported(replayMetadata.registry);
  const lines = [
    "import { expect, test } from \"@playwright/test\";",
    `import { FakeDaemon } from "${fakeDaemonImport}";`,
    `import { appendTraceEvent, collectPufferUiState, createPufferActionEdge, createTraceId, installRuntimeOracle, writeTraceJsonl } from "${relativeCoverageImport}";`,
    "",
    `const replayMetadata = ${JSON.stringify(replayMetadata, null, 2)} as const;`,
    "",
    `test("fuzz replay ${testCase.caseId}", async ({ page }, testInfo) => {`,
    `  const daemon = new FakeDaemon(${JSON.stringify(daemonOptions, null, 2)});`,
    "  const replayTurnRegistry = installUniqueReplayTurnIds(daemon);",
    "  const traceId = createTraceId(\"puffer-fuzz\");",
    "  const trace = [];",
    "  installRuntimeOracle(page, trace, traceId);",
    "  await daemon.install(page);",
    "  await daemon.open(page);",
    "  await ensureReplayBodyVisible(page);",
    "  let activeReplaySessionId = \"session-browser\";",
    "  let activeReplayTurnId: string | null = null;",
    "  let activeReplayEventTurnId: string | null = null;",
    "  let connectorReplaySessionId: string | null = null;",
    "  let connectorReplayTurnId: string | null = null;",
    "  let connectorReplayConnectionSlug: string | null = null;",
    "  let createdReplaySessionCount = 0;",
    "  let replayTurnSequence = 0;",
    "  let socketCountBeforeDisconnect = 0;",
    "  let requestIndexBefore = 0;",
    "  appendTraceEvent(trace, { type: \"state\", traceId, step: 0, state: await collectPufferUiState(page, { viewport: \"desktop\", browserOrShell: \"chromium\", fakeDaemon: true }) });",
    "  const initialBrowserState = await collectBrowserReplayState(page);",
    ""
  ];

  let step = 1;
  for (const action of testCase.steps ?? []) {
    if (action.phase === "assert") continue;
    lines.push(`  // Step ${step}: ${action.action}${action.params ? ` ${JSON.stringify(action.params)}` : ""}`);
    lines.push(`  const beforeState${step} = await collectPufferUiState(page, { viewport: "desktop", browserOrShell: "chromium", fakeDaemon: true });`);
    lines.push("  {");
    for (const command of commandsForAction(action)) {
      lines.push(`    ${command}`);
    }
    lines.push("  }");
    lines.push(`  const afterState${step} = await collectPufferUiState(page, { viewport: "desktop", browserOrShell: "chromium", fakeDaemon: true });`);
    lines.push(`  appendTraceEvent(trace, { type: "action", traceId, step: ${step}, action: ${JSON.stringify(action)}, beforeState: beforeState${step}, afterState: afterState${step}, edge: createPufferActionEdge(${JSON.stringify(action)}, beforeState${step}, afterState${step}, ${JSON.stringify(action.coverage ?? [])}) });`);
    lines.push("");
    step += 1;
  }

  lines.push("  expect(page.isClosed()).toBe(false);");
  lines.push("  const finalBrowserState = await collectBrowserReplayState(page);");
  lines.push("  await assertReplayInvariants(page, daemon, trace, traceId, replayMetadata, initialBrowserState, finalBrowserState, activeReplaySessionId);");
  lines.push("  const tracePath = testInfo.outputPath(`${traceId}.jsonl`);");
  lines.push("  writeTraceJsonl(tracePath, trace);");
  lines.push("});");
  lines.push("");
  lines.push(...replayHelperLines());
  return lines.join("\n");
}

export function formatReplayMarkdown(testCase, outputPath) {
  const lines = [
    "# Puffer Fuzz Replay Scaffold",
    "",
    `Case: ${testCase.caseId}`,
    `Seed: ${testCase.seedId}`,
    `Suggested spec path: ${outputPath}`,
    "",
    "## Steps",
    ""
  ];
  for (const step of testCase.steps ?? []) {
    lines.push(`- ${step.phase}: ${step.action} ${step.params ? JSON.stringify(step.params) : ""}`.trim());
  }
  lines.push("", "## Notes", "");
  lines.push("- The scaffold includes concrete FakeDaemon reconnect handling, trace capture, and baseline browser invariants.");
  lines.push("- Unsupported generated actions fail fast so false replay passes do not inflate coverage.");
  lines.push("- Run the spec three times before filing a confirmed bug, then shrink the sequence to the smallest stable reproducer.");
  return `${lines.join("\n")}\n`;
}

function buildReplayMetadata(testCase, daemonOptions = {}) {
  const coverage = testCase.coverage ?? [];
  const steps = testCase.steps ?? [];
  return {
    caseId: testCase.caseId,
    seedId: testCase.seedId,
    coverage,
    registry: buildReplayRegistryMetadata(testCase, REPLAY_ACTION_IDS),
    initialSessionCount: Array.isArray(daemonOptions.sessions) ? daemonOptions.sessions.length : 1,
    typedUrls: steps
      .filter((step) => step.action === "type-url" && step.params?.url)
      .map((step) => String(step.params.url)),
    recoverableTypedUrls: recoverableTypedUrls(steps),
    typedMessages: steps
      .filter((step) => step.action === "type-composer" && step.params?.text)
      .map((step) => String(step.params.text)),
    expectedNavigateCountsByUrl: expectedNavigateCountsByUrl(steps),
    expectedTurnCountsByMessage: expectedTurnCountsByMessage(steps),
    staleUrls: steps
      .filter((step) => step.action === "emit-state-for-old-tab" && step.params?.url)
      .map((step) => String(step.params.url)),
    hasDroppedResponse: coverage.includes("async:dropped-response") ||
      steps.some((step) => step.action === "hold-next-browser-response"),
    hasInjectedFailure: steps.some((step) => step.action?.startsWith("fail-next-")),
    hasReconnect: coverage.includes("async:reconnect") ||
      steps.some((step) => step.action === "disconnect-reconnect"),
    hasDuplicateSubmit: coverage.includes("async:duplicate-submit")
  };
}

function expectedNavigateCountsByUrl(steps) {
  const counts = {};
  let currentUrl = "";
  let submittedCurrentIntent = false;
  for (const step of steps) {
    if (step.action === "type-url" && step.params?.url) {
      currentUrl = String(step.params.url);
      submittedCurrentIntent = false;
      continue;
    }
    if (step.action !== "press-address-enter" || !currentUrl || submittedCurrentIntent) continue;
    counts[currentUrl] = (counts[currentUrl] ?? 0) + 1;
    submittedCurrentIntent = true;
  }
  return counts;
}

function expectedTurnCountsByMessage(steps) {
  const counts = {};
  let currentMessage = "";
  let submittedCurrentIntent = false;
  for (const step of steps) {
    if (step.action === "start-connector-setup" || step.action === "answer-connector-questions") {
      const connectionSlug = String(step.params?.connectionSlug ?? "fuzz-connector");
      const message = `/connect telegram-login ${connectionSlug}`;
      counts[message] = (counts[message] ?? 0) + 1;
      continue;
    }
    if (step.action === "type-composer" && step.params?.text) {
      currentMessage = String(step.params.text);
      submittedCurrentIntent = false;
      continue;
    }
    if (!["send-prompt", "rapid-send-prompt"].includes(step.action) || !currentMessage || submittedCurrentIntent) continue;
    counts[currentMessage] = (counts[currentMessage] ?? 0) + 1;
    submittedCurrentIntent = true;
  }
  return counts;
}

function recoverableTypedUrls(steps) {
  const urls = [];
  for (let index = 0; index < steps.length; index += 1) {
    const step = steps[index];
    if (step.action !== "type-url" || !step.params?.url) continue;
    const tail = steps.slice(index + 1);
    const nextTypedUrlIndex = tail.findIndex((candidate) => candidate.action === "type-url");
    const window = nextTypedUrlIndex >= 0 ? tail.slice(0, nextTypedUrlIndex) : tail;
    const hasInjectedFailure = window.some((candidate) => candidate.action?.startsWith("fail-next-"));
    const changesActiveTab = window.some((candidate) => [
      "click-new-tab",
      "close-active-tab",
      "emit-empty-tab-list",
      "switch-tab"
    ].includes(candidate.action));
    if (hasInjectedFailure && !changesActiveTab) urls.push(String(step.params.url));
  }
  return urls;
}

function commandsForAction(step) {
  const params = step.params ?? {};
  switch (step.action) {
    case "open-workspace":
      return [
        "await ensureWorkspaceOpen(page);"
      ];
    case "open-settings-providers":
      return [
        "await ensureProvidersOpen(page);"
      ];
    case "type-search":
      return [
        "await ensureWorkspaceOpen(page);",
        `await page.getByLabel("Search workspace").fill(${JSON.stringify(params.text ?? "browser")}, { timeout: 5_000 });`
      ];
    case "clear-search":
      return [
        "await ensureWorkspaceOpen(page);",
        "await page.getByRole(\"button\", { name: \"Clear search\" }).click({ timeout: 1_000 }).catch(async () => {",
        "  await page.getByLabel(\"Search workspace\").fill(\"\", { timeout: 5_000 });",
        "});"
      ];
    case "open-agent-card":
      return [
        "await closeReplayModalIfOpen(page);",
        "requestIndexBefore = daemon.requests.length;",
        `await clickReplaySession(page, ${JSON.stringify(params.session ?? "session-browser")}, "open workspace/session card");`,
        `activeReplaySessionId = ${JSON.stringify(params.session ?? "session-browser")};`,
        "activeReplayTurnId = null;",
        "activeReplayEventTurnId = null;",
        "await waitForNewDaemonRequest(daemon, \"load_session_detail\", requestIndexBefore, (request) => request.params.sessionId === activeReplaySessionId);"
      ];
    case "pin-agent":
      return [
        "await closeReplayModalIfOpen(page);",
        "daemon.delayResponse(\"set_desktop_pin\", () => true, 250);",
        `const agentRow = page.locator(".pf-sidebar-agent-row").filter({ hasText: ${JSON.stringify(sessionDisplayName(params.session ?? "session-browser"))} }).first();`,
        "await expect(agentRow).toBeVisible({ timeout: 5_000 });",
        "requestIndexBefore = daemon.requests.length;",
        "await agentRow.getByRole(\"button\", { name: /Pin agent|Unpin agent/ }).evaluate((button) => {",
        "  (button as HTMLButtonElement).click();",
        "  (button as HTMLButtonElement).click();",
        "});",
        "await waitForNewDaemonRequest(daemon, \"set_desktop_pin\", requestIndexBefore, () => true, 1_000).catch(() => undefined);",
        "await page.waitForTimeout(50);",
        "expect(daemon.requests.slice(requestIndexBefore).filter((request) => request.method === \"set_desktop_pin\").length).toBeLessThanOrEqual(1);"
      ];
    case "open-connect-project":
      return [
        "await ensureWorkspaceOpen(page);",
        "await page.getByRole(\"button\", { name: \"Create Project\" }).click({ timeout: 5_000 });",
        "await expect(page.getByRole(\"dialog\", { name: \"Create Project\" })).toBeVisible({ timeout: 5_000 });"
      ];
    case "open-workspace-picker":
      return [
        "await ensureWorkspaceOpen(page);",
        "await page.getByRole(\"button\", { name: \"Switch workspace\" }).click({ timeout: 5_000 });",
        "await expect(page.getByRole(\"dialog\", { name: \"Switch workspace\" })).toBeVisible({ timeout: 5_000 });"
      ];
    case "assert-modal-initial-focus":
      return [
        `await assertDialogOwnsFocus(page, ${JSON.stringify(params.name ?? "dialog")});`
      ];
    case "assert-modal-focus-trap":
      return [
        `await assertDialogTrapsTabFocus(page, ${JSON.stringify(params.name ?? "dialog")}, ${Number(params.tabs ?? 12)});`
      ];
    case "emit-grouped-session-refresh":
      return [
        "requestIndexBefore = daemon.requests.length;",
        "daemon.emit(\"workspace:sessions:changed\", { reason: \"fuzz-grouped-refresh\", at: Date.now() });",
        "await waitForNewDaemonRequest(daemon, \"list_grouped_sessions\", requestIndexBefore, () => true, 3_000).catch(() => undefined);"
      ];
    case "emit-active-agent-change":
      return [
        "daemon.setSessionTimeline(activeReplaySessionId, [{ kind: \"assistant_message\", id: `fuzz-${Date.now()}`, text: `Fuzz update for ${activeReplaySessionId}`, createdAtMs: Date.now() }]);",
        "requestIndexBefore = daemon.requests.length;",
        "daemon.emit(\"workspace:sessions:changed\", { reason: \"fuzz-active-agent-change\", sessionId: activeReplaySessionId, at: Date.now() });",
        "await waitForNewDaemonRequest(daemon, \"list_grouped_sessions\", requestIndexBefore, () => true, 3_000).catch(() => undefined);"
      ];
    case "open-agent-detail":
      return [
        "await clickFirstVisible(page, [",
        "  () => page.locator(\".pf-sidebar-agents-list\").getByRole(\"button\", { name: /^Browser regression\\b/ }),",
        "  () => page.getByRole(\"button\", { name: /Browser regression/ }).first()",
        "], \"open Browser regression agent\");"
      ];
    case "open-browser-pane":
      return [
        "await clickFirstVisible(page, [",
        "  () => page.locator(\".pf-agent-tabs\").getByRole(\"button\", { name: \"Browser\", exact: true }),",
        "  () => page.getByRole(\"button\", { name: \"Browser\", exact: true })",
        "], \"open Browser panel\");",
        "await waitForDaemonRequest(daemon, \"browser_open\");"
      ];
    case "type-composer":
      return [
        `await fillReplayComposerIfPossible(page, ${JSON.stringify(params.text ?? "test prompt")});`
      ];
    case "send-prompt":
      return [
        "const turnRequest = await sendReplayPromptIfPossible(page, daemon, false);",
        "if (turnRequest) {",
        "  replayTurnSequence += 1;",
        "  activeReplayTurnId = replayTurnRegistry.latestForSession(String(turnRequest.params.sessionId ?? activeReplaySessionId));",
        "  if (!activeReplayTurnId) throw new Error(\"Fake daemon did not record a replay turn id\");",
        "  activeReplayEventTurnId = activeReplayTurnId;",
        "}"
      ];
    case "rapid-send-prompt":
      return [
        "const turnRequest = await sendReplayPromptIfPossible(page, daemon, true);",
        "if (turnRequest) {",
        "  replayTurnSequence += 1;",
        "  activeReplayTurnId = replayTurnRegistry.latestForSession(String(turnRequest.params.sessionId ?? activeReplaySessionId));",
        "  if (!activeReplayTurnId) throw new Error(\"Fake daemon did not record a replay turn id\");",
        "  activeReplayEventTurnId = activeReplayTurnId;",
        "}"
      ];
    case "stop-turn":
      return [
        "if (activeReplayTurnId) {",
        "  const stopButton = page.getByRole(\"button\", { name: \"Stop turn\" });",
        "  if ((await stopButton.count().catch(() => 0)) > 0 && (await stopButton.isEnabled().catch(() => false))) {",
        "    requestIndexBefore = daemon.requests.length;",
        "    await stopButton.click({ timeout: 2_000 }).catch(() => undefined);",
        "    await waitForNewDaemonRequest(daemon, \"cancel_turn\", requestIndexBefore, () => true, 3_000).catch(() => undefined);",
        "  }",
        "}"
      ];
    case "complete-turn":
      return [
        "if (activeReplayTurnId) {",
        "  daemon.emit(`session:${activeReplaySessionId}:event`, { type: \"turn-complete\", turnId: activeReplayEventTurnId ?? activeReplayTurnId, assistantText: \"Fuzz completed turn\" });",
        "  await page.getByRole(\"button\", { name: \"Send\", exact: true }).waitFor({ state: \"visible\", timeout: 5_000 }).catch(() => undefined);",
        "  replayTurnRegistry.clear(activeReplayTurnId);",
        "  activeReplayTurnId = null;",
        "  activeReplayEventTurnId = null;",
        "}"
      ];
    case "settle-canceled-turn":
      return [
        "if (activeReplayTurnId) {",
        "  daemon.emit(`session:${activeReplaySessionId}:event`, { type: \"turn-error\", turnId: activeReplayTurnId, error: \"Canceled by fuzz\" });",
        "  await page.getByRole(\"button\", { name: \"Send\", exact: true }).waitFor({ state: \"visible\", timeout: 5_000 }).catch(() => undefined);",
        "  replayTurnRegistry.clear(activeReplayTurnId);",
        "  activeReplayTurnId = null;",
        "  activeReplayEventTurnId = null;",
        "}"
      ];
    case "switch-session":
      return [
        "requestIndexBefore = daemon.requests.length;",
        `await clickReplaySession(page, ${JSON.stringify(params.session ?? "session-browser")}, "switch session ${params.session ?? "session-browser"}");`,
        `activeReplaySessionId = ${JSON.stringify(params.session ?? "session-browser")};`,
        "activeReplayTurnId = null;",
        "activeReplayEventTurnId = null;",
        "await waitForNewDaemonRequest(daemon, \"load_session_detail\", requestIndexBefore, (request) => request.params.sessionId === activeReplaySessionId);"
      ];
    case "emit-old-session-stream":
      return [
        `daemon.emit(\`session:\${staleReplaySessionId(activeReplaySessionId)}:event\`, { type: "turn-complete", turnId: "turn-stale-fuzz", assistantText: ${JSON.stringify(params.delta ?? "late token")} });`
      ];
    case "emit-permission":
      return [
        "if (activeReplayTurnId) {",
        "  activeReplayEventTurnId = activeReplayTurnId;",
        `  daemon.emit(\`session:\${activeReplaySessionId}:event\`, { type: "permission-request", turnId: activeReplayEventTurnId, requestId: ${JSON.stringify(`request-${step.id}`)}, toolId: ${JSON.stringify(params.tool ?? "bash")}, summary: "Fuzz permission request", reason: "Fuzz permission reason" });`,
        "  await page.getByText(\"Approval needed\").waitFor({ state: \"visible\", timeout: 5_000 }).catch(() => undefined);",
        "}"
      ];
    case "answer-permission":
      return [
        `const permissionButton = page.getByRole("button", { name: ${JSON.stringify(permissionButtonName(params.answer))} });`,
        "if ((await permissionButton.count().catch(() => 0)) > 0) {",
        "  requestIndexBefore = daemon.requests.length;",
        "  await permissionButton.click({ timeout: 2_000 }).catch(() => undefined);",
        "  await waitForNewDaemonRequest(daemon, \"resolve_permission\", requestIndexBefore, () => true, 3_000).catch(() => undefined);",
        "}"
      ];
    case "emit-question":
      return [
        "if (activeReplayTurnId) {",
        "  activeReplayEventTurnId = activeReplayTurnId;",
        `  daemon.emit(\`session:\${activeReplaySessionId}:event\`, { type: "user-question-request", turnId: activeReplayEventTurnId, requestId: ${JSON.stringify(`request-${step.id}`)}, questions: [{ header: "Fuzz", question: "Which path should I use?", options: [{ label: "src", description: "Use src." }, { label: "tests", description: "Use tests." }] }] });`,
        "  await page.getByText(\"Which path should I use?\").waitFor({ state: \"visible\", timeout: 5_000 }).catch(() => undefined);",
        "}"
      ];
    case "answer-question":
      return [
        "if ((await page.getByRole(\"button\", { name: \"Send answer\" }).count().catch(() => 0)) > 0) {",
        `  await page.getByPlaceholder("Type another answer").fill(${JSON.stringify(params.answer ?? "custom answer")}, { timeout: 2_000 }).catch(async () => { await page.locator("textarea").last().fill(${JSON.stringify(params.answer ?? "custom answer")}, { timeout: 2_000 }).catch(() => undefined); });`,
        "  requestIndexBefore = daemon.requests.length;",
        "  await page.getByRole(\"button\", { name: \"Send answer\" }).click({ timeout: 2_000 }).catch(() => undefined);",
        "  await waitForNewDaemonRequest(daemon, \"resolve_user_question\", requestIndexBefore, () => true, 3_000).catch(() => undefined);",
        "}"
      ];
    case "change-model":
      return [
        `await changeReplayModel(page, daemon, ${JSON.stringify(params.model ?? "codex/test-model")});`
      ];
    case "delayed-transcript-refresh":
      return [
        "daemon.delayResponse(\"load_session_detail\", () => true, 350);"
      ];
    case "import-credential":
      return [
        "await ensureProvidersOpen(page);",
        `const providerId = normalizeReplayProviderId(${JSON.stringify(params.provider ?? "codex")});`,
        "requestIndexBefore = daemon.requests.length;",
        "await providerCard(page, providerId).locator(\"button.import\").first().click({ timeout: 5_000 });",
        "await waitForNewDaemonRequest(daemon, \"import_external_credential\", requestIndexBefore, (request) => requestProviderMatches(request, providerId));"
      ];
    case "refresh-credential":
      return [
        "await ensureProvidersOpen(page);",
        `const providerId = normalizeReplayProviderId(${JSON.stringify(params.provider ?? "codex")});`,
        "requestIndexBefore = daemon.requests.length;",
        "if (providerId === \"anthropic\") {",
        "  const card = providerCard(page, providerId);",
        "  await card.getByLabel(\"API key for Anthropic\").fill(`sk-fuzz-${Date.now()}`, { timeout: 5_000 });",
        "  await card.getByRole(\"button\", { name: /Connect|Update key/ }).click({ timeout: 5_000 });",
        "  await waitForNewDaemonRequest(daemon, \"login_with_api_key\", requestIndexBefore, (request) => requestProviderMatches(request, providerId));",
        "} else {",
        "  await providerCard(page, providerId).getByRole(\"button\", { name: /Connect with OAuth|Reconnect with OAuth/ }).click({ timeout: 5_000 });",
        "  await waitForNewDaemonRequest(daemon, \"login_with_oauth\", requestIndexBefore, (request) => requestProviderMatches(request, providerId));",
        "}"
      ];
    case "save-default-model":
    case "save-default-model-codex":
    case "save-default-model-anthropic":
      return [
        "await ensureProvidersOpen(page);",
        `await saveReplayDefaultModel(page, daemon, ${JSON.stringify(params.provider ?? "codex")}, ${JSON.stringify(params.model ?? "test-model")});`
      ];
    case "open-new-agent":
      return [
        "await ensureWorkspaceOpen(page);",
        "await clickFirstVisible(page, [",
        "  () => page.getByRole(\"button\", { name: \"New agent in puffer\" }),",
        "  () => page.getByRole(\"button\", { name: \"New agent in default workspace\" }),",
        "  () => page.locator(\".pf-pw-project\").first().getByRole(\"button\", { name: \"New agent\" })",
        "], \"open New agent modal\");",
        "await expect(page.getByRole(\"dialog\", { name: \"New agent\" })).toBeVisible({ timeout: 5_000 });"
      ];
    case "switch-new-agent-provider":
    case "switch-new-agent-provider-codex":
    case "switch-new-agent-provider-anthropic":
      return [
        `await page.getByRole("dialog", { name: "New agent" }).getByRole("radio", { name: ${providerLabelRegex(params.provider ?? "codex")} }).click({ timeout: 5_000 });`
      ];
    case "submit-new-agent":
      return [
        "daemon.delayResponse(\"create_session\", () => true, 250);",
        "requestIndexBefore = daemon.requests.length;",
        "await page.getByRole(\"dialog\", { name: \"New agent\" }).getByRole(\"button\", { name: \"Start agent\" }).evaluate((button) => {",
        "  (button as HTMLButtonElement).click();",
        "  (button as HTMLButtonElement).click();",
        "});",
        "await waitForNewDaemonRequest(daemon, \"create_session\", requestIndexBefore);",
        "await page.waitForTimeout(50);",
        "expect(daemon.requests.slice(requestIndexBefore).filter((request) => request.method === \"create_session\")).toHaveLength(1);",
        "createdReplaySessionCount += 1;",
        "activeReplaySessionId = `session-created-${replayMetadata.initialSessionCount + createdReplaySessionCount}`;",
        "activeReplayTurnId = null;",
        "activeReplayEventTurnId = null;",
        "await expect(page.locator(\".pf-composer textarea\")).toBeVisible({ timeout: 5_000 });"
      ];
    case "emit-auth-list-refresh":
    case "emit-auth-one-provider-refresh":
    case "emit-auth-none-refresh":
      return [
        "if ((await page.locator(\".login-page .refresh-btn\").count().catch(() => 0)) === 0) {",
        "  await ensureProvidersOpen(page);",
        "}",
        `daemon.setAuthStatuses(replayAuthStatuses(${JSON.stringify(params.authState ?? authStateForAction(step.action))}));`,
        "requestIndexBefore = daemon.requests.length;",
        "await clickFirstVisible(page, [",
        "  () => page.locator(\".login-page .refresh-btn\"),",
        "  () => page.locator(\".pf-settings-pane\").getByRole(\"button\", { name: \"Refresh\" }).first()",
        "], \"refresh provider/auth list\");",
        "await waitForNewDaemonRequest(daemon, \"load_settings_snapshot\", requestIndexBefore);"
      ];
    case "delayed-model-list":
      return [
        "daemon.delayResponse(\"list_provider_models\", () => true, 350);"
      ];
    case "type-url":
      return [
        "const address = page.locator(\".pf-browser-address\").first();",
        "if (await address.isEnabled().catch(() => false)) {",
        `  await address.fill(${JSON.stringify(params.url ?? "about:blank")}, { timeout: 2_000 });`,
        "}"
      ];
    case "press-address-enter":
      return [
        "const address = page.locator(\".pf-browser-address\").first();",
        "if (await address.isEnabled().catch(() => false)) {",
        "  requestIndexBefore = daemon.requests.length;",
        "  await address.press(\"Enter\", { timeout: 2_000 });",
        "  await waitForNewDaemonRequest(daemon, \"browser_navigate\", requestIndexBefore, () => true, 2_000).catch(() => undefined);",
        "}"
      ];
    case "click-new-tab":
      return [
        "await page.locator(\".pf-browser-tab-add\").click({ timeout: 5_000 });",
        "await waitForDaemonRequest(daemon, \"browser_agent\", (request) => request.params.action === \"open\");"
      ];
    case "close-active-tab":
      return [
        "const closeButton = page.locator(\".pf-browser-tab\").filter({ hasText: /./ }).first().getByRole(\"button\");",
        "if ((await closeButton.count().catch(() => 0)) > 0) {",
        "  requestIndexBefore = daemon.requests.length;",
        "  await closeButton.click({ timeout: 1_000 }).catch(() => undefined);",
        "  await waitForNewDaemonRequest(daemon, \"browser_agent\", requestIndexBefore, (request) => request.params.action === \"close\", 2_000).catch(() => undefined);",
        "}"
      ];
    case "reload":
      return [
        "const reloadButton = page.locator(\"button[title='Reload']\");",
        "if (await reloadButton.isEnabled().catch(() => false)) {",
        "  await reloadButton.click({ timeout: 1_000 });",
        "  await waitForDaemonRequest(daemon, \"browser_reload\", () => true, 2_000).catch(() => undefined);",
        "}"
      ];
    case "history":
      return [
        `const historyButton = page.locator("button[title='${params.direction === "forward" ? "Forward" : "Back"}']");`,
        "if (await historyButton.isEnabled().catch(() => false)) {",
        "  await historyButton.click({ timeout: 1_000 });",
        "  await waitForDaemonRequest(daemon, \"browser_history\", () => true, 2_000).catch(() => undefined);",
        "}"
      ];
    case "switch-tab":
      return [
        `await clickBrowserTabIfPresent(page, ${Number(params.tabIndex ?? 0)});`
      ];
    case "open-file":
      return [
        "await openAgentTab(page, \"Files\");",
        "requestIndexBefore = daemon.requests.length;",
        "await clickReplayFileTreeFile(page);",
        "await waitForNewDaemonRequest(daemon, \"read_file\", requestIndexBefore, () => true, 2_000).catch(async () => {",
        "  await expect(page.getByRole(\"textbox\", { name: \"Edit file contents\" })).toBeVisible({ timeout: 2_000 });",
        "});"
      ];
    case "edit-file":
      return [
        "await openAgentTab(page, \"Files\");",
        "await clickFirstVisible(page, [",
        `  () => page.locator("textarea, [contenteditable='true']").last(),`,
        "  () => page.locator(\".pf-file-editor textarea, .cm-content\").first()",
        `], "focus file editor");`,
        `await page.keyboard.type(${JSON.stringify(params.text ?? "temporary draft")});`
      ];
    case "save-file":
      return [
        "await openAgentTab(page, \"Files\");",
        "await ensureReplayFileDirty(page);",
        "requestIndexBefore = daemon.requests.length;",
        "await clickFirstVisible(page, [",
        "  () => page.getByRole(\"button\", { name: /Save/i }).first(),",
        "  () => page.locator(\"button\").filter({ hasText: \"Save\" }).first()",
        "], \"save file\");",
        "await waitForNewDaemonRequest(daemon, \"write_file\", requestIndexBefore, () => true, 2_000);"
      ];
    case "emit-file-restore":
      return [
        "daemon.emit(\"workspace:fs:changed\", { path: \"/tmp/puffer/README.md\", reason: \"fuzz-restore\", at: Date.now() });"
      ];
    case "open-terminal":
      return [
        "await openAgentTab(page, \"Terminal\");",
        "requestIndexBefore = daemon.requests.length;",
        "await clickFirstVisible(page, [",
        "  () => page.getByRole(\"button\", { name: /^New terminal$/i }).first(),",
        "  () => page.getByRole(\"button\", { name: /^Open terminal$/i }).first(),",
        "  () => page.locator(\".pf-terminal-pane\").getByRole(\"button\", { name: /New terminal|Open terminal/i }).first()",
        "], \"open terminal\");",
        "await waitForNewDaemonRequest(daemon, \"pty_open\", requestIndexBefore, () => true, 2_000);"
      ];
    case "type-terminal":
      return [
        "await openAgentTab(page, \"Terminal\");",
        "await ensureReplayTerminalOpen(page, daemon);",
        "await focusReplayTerminalInput(page);",
        "requestIndexBefore = daemon.requests.length;",
        "await page.keyboard.type(" + JSON.stringify(params.text ?? "pwd") + ");",
        "await page.keyboard.press(\"Enter\");",
        "await waitForNewDaemonRequest(daemon, \"pty_write\", requestIndexBefore, () => true, 2_000);"
      ];
    case "close-terminal":
      return [
        "await openAgentTab(page, \"Terminal\");",
        "await expect(page.locator(\".pf-terminal-pane\")).toBeVisible({ timeout: 5_000 });",
        "const terminalCloseButton = page.locator(\".pf-terminal-pane\").getByRole(\"button\", { name: /^Close Terminal \\d+$/i }).last();",
        "const terminalCloseFallback = page.locator(\".pf-terminal-pane .terminal-tab-close\").last();",
        "if (!(await terminalCloseButton.isVisible({ timeout: 500 }).catch(() => false)) && !(await terminalCloseFallback.isVisible({ timeout: 500 }).catch(() => false))) return;",
        "requestIndexBefore = daemon.requests.length;",
        "await clickFirstVisible(page, [",
        "  () => terminalCloseButton,",
        "  () => terminalCloseFallback",
        "], \"close terminal\");",
        "await waitForNewDaemonRequest(daemon, \"pty_close\", requestIndexBefore, () => true, 2_000);"
      ];
    case "emit-late-pty-output":
      return [
        "daemon.emit(\"pty:fuzz:data\", { data: \"late fuzz output\\n\", at: Date.now() });"
      ];
    case "open-settings-mcp":
      return [
        "await openSettingsPane(page, \"MCP Servers\", /MCP|Servers/i);"
      ];
    case "open-settings-connectors":
      return [
        "await openSettingsPane(page, \"Connectors\", /Connectors/i);"
      ];
    case "start-connector-setup":
      return [
        "await openSettingsPane(page, \"Connectors\", /Connectors/i);",
        "const connectorPane = page.locator(\".pf-settings-pane\");",
        "const existingConnectorQuestions = connectorPane.getByLabel(\"Connector setup questions\");",
        "if (!(await existingConnectorQuestions.isVisible({ timeout: 500 }).catch(() => false))) {",
        "  const connectorSlugInput = connectorPane.getByLabel(\"Connector connection slug\");",
        "  await expect(connectorSlugInput).toBeVisible({ timeout: 3_000 });",
        "  const connectorSlugEnabled = await expect(connectorSlugInput).toBeEnabled({ timeout: 5_000 }).then(() => true).catch(() => false);",
        "  if (connectorSlugEnabled) {",
        `    await connectorSlugInput.fill(${JSON.stringify(params.connectionSlug ?? "fuzz-connector")});`,
        "    requestIndexBefore = daemon.requests.length;",
        "    await connectorPane.getByRole(\"button\", { name: /Start setup/i }).click({ timeout: 2_000 });",
        "    const connectorTurnRequest = await waitForNewDaemonRequest(daemon, \"run_agent_turn\", requestIndexBefore, () => true, 3_000);",
        "    connectorReplaySessionId = String(connectorTurnRequest.params.sessionId ?? \"session-browser\");",
        "    connectorReplayTurnId = replayTurnRegistry.latestForSession(connectorReplaySessionId);",
        "    connectorReplayConnectionSlug = " + JSON.stringify(params.connectionSlug ?? "fuzz-connector") + ";",
        "    emitReplayConnectorQuestions(daemon, connectorReplaySessionId, connectorReplayTurnId);",
        "    await connectorPane.getByLabel(\"Connector setup questions\").waitFor({ state: \"visible\", timeout: 3_000 }).catch(() => undefined);",
        "  }",
        "}"
      ];
    case "answer-connector-questions":
      return [
        "await openSettingsPane(page, \"Connectors\", /Connectors/i);",
        "const connectorPane = page.locator(\".pf-settings-pane\");",
        "let connectorQuestionPanel = connectorPane.getByLabel(\"Connector setup questions\");",
        "if (!(await connectorQuestionPanel.isVisible({ timeout: 500 }).catch(() => false))) {",
        "  await connectorQuestionPanel.waitFor({ state: \"visible\", timeout: 3_000 }).catch(() => undefined);",
        "}",
        "if (!(await connectorQuestionPanel.isVisible({ timeout: 500 }).catch(() => false))) {",
        "  const connectorSlugInput = connectorPane.getByLabel(\"Connector connection slug\");",
        "  await expect(connectorSlugInput).toBeVisible({ timeout: 3_000 });",
        "  const connectorSlugEnabled = await expect(connectorSlugInput).toBeEnabled({ timeout: 5_000 }).then(() => true).catch(() => false);",
        "  if (connectorSlugEnabled) {",
        `    await connectorSlugInput.fill(${JSON.stringify(params.connectionSlug ?? "fuzz-connector")});`,
        "    requestIndexBefore = daemon.requests.length;",
        "    await connectorPane.getByRole(\"button\", { name: /Start setup/i }).click({ timeout: 2_000 });",
        "    const connectorTurnRequest = await waitForNewDaemonRequest(daemon, \"run_agent_turn\", requestIndexBefore, () => true, 3_000);",
        "    connectorReplaySessionId = String(connectorTurnRequest.params.sessionId ?? \"session-browser\");",
        "    connectorReplayTurnId = replayTurnRegistry.latestForSession(connectorReplaySessionId);",
        "    connectorReplayConnectionSlug = " + JSON.stringify(params.connectionSlug ?? "fuzz-connector") + ";",
        "    emitReplayConnectorQuestions(daemon, connectorReplaySessionId, connectorReplayTurnId);",
        "    connectorQuestionPanel = connectorPane.getByLabel(\"Connector setup questions\");",
        "  }",
        "}",
        "await expect(connectorQuestionPanel).toBeVisible({ timeout: 3_000 });",
        "const connectorInputs = connectorQuestionPanel.locator(\".pf-connector-question input[type='text'], .pf-connector-question input[type='password']\");",
        "for (let index = 0; index < await connectorInputs.count(); index += 1) {",
        "  const input = connectorInputs.nth(index);",
        "  if (await input.isEnabled().catch(() => false)) await input.fill(\"fuzz-secret\");",
        "}",
        "const connectorCheckboxes = connectorQuestionPanel.locator(\".pf-connector-question input[type='checkbox']\");",
        "if (await connectorCheckboxes.count()) await connectorCheckboxes.first().check().catch(() => undefined);",
        "const submitConnectorAnswers = connectorQuestionPanel.getByRole(\"button\", { name: /Submit answers/i });",
        "await expect(submitConnectorAnswers).toBeEnabled({ timeout: 3_000 });",
        "requestIndexBefore = daemon.requests.length;",
        "await submitConnectorAnswers.click({ timeout: 2_000 });",
        "await waitForNewDaemonRequest(daemon, \"resolve_user_question\", requestIndexBefore, () => true, 3_000);",
        "emitReplayConnectorComplete(daemon, connectorReplaySessionId, connectorReplayTurnId, connectorReplayConnectionSlug);",
        "connectorReplaySessionId = null;",
        "connectorReplayTurnId = null;",
        "connectorReplayConnectionSlug = null;"
      ];
    case "add-mcp-server":
      return [
        "await openSettingsPane(page, \"MCP Servers\", /MCP|Servers/i);",
        `await page.getByLabel("ID").fill(${JSON.stringify(params.id ?? "tmp-server")}, { timeout: 2_000 });`,
        `await page.getByLabel("Command").fill(${JSON.stringify(params.target ?? "npx @playwright/mcp")}, { timeout: 2_000 });`,
        "requestIndexBefore = daemon.requests.length;",
        "await page.getByRole(\"button\", { name: /Add server|Add/i }).click({ timeout: 2_000 });",
        "await waitForNewDaemonRequest(daemon, \"add_mcp_server\", requestIndexBefore, () => true, 2_000);"
      ];
    case "emit-mcp-list-refresh":
    case "late-mcp-test-result":
      return [
        "daemon.emit(\"settings:mcp:changed\", { reason: \"fuzz-refresh\", at: Date.now() });",
        "daemon.delayResponse(\"list_mcp_servers\", () => true, 350);"
      ];
    case "open-permissions":
      return [
        "await openSettingsPane(page, \"Permissions\", /Permissions/i);"
      ];
    case "save-permissions":
      return [
        "await openSettingsPane(page, \"Permissions\", /Permissions/i);",
        "const permissionPane = page.locator(\".pf-settings-pane\");",
        "let permissionMode = permissionPane.locator(\"select\").first();",
        "if ((await permissionMode.count().catch(() => 0)) === 0) {",
        "  await permissionPane.getByRole(\"button\", { name: /Add rule/i }).click({ timeout: 2_000 });",
        "  permissionMode = permissionPane.locator(\"select\").first();",
        "}",
        "await expect(permissionMode).toBeEnabled({ timeout: 2_000 });",
        "const permissionValues = await permissionMode.locator(\"option\").evaluateAll((options) => options.map((option) => (option as HTMLOptionElement).value).filter(Boolean));",
        "const currentPermissionValue = await permissionMode.inputValue().catch(() => \"\");",
        "const nextPermissionValue = permissionValues.find((value) => value !== currentPermissionValue) ?? \"allow\";",
        "if (!nextPermissionValue) throw new Error(\"No permission mode option is available\");",
        "await permissionMode.selectOption(nextPermissionValue);",
        "await expect(permissionMode).toHaveValue(nextPermissionValue, { timeout: 2_000 });",
        "const savePermissionsButton = permissionPane.getByRole(\"button\", { name: \"Save\", exact: true });",
        "await expect(savePermissionsButton).toBeEnabled({ timeout: 2_000 });",
        "requestIndexBefore = daemon.requests.length;",
        "await savePermissionsButton.click({ timeout: 2_000 });",
        "await waitForNewDaemonRequest(daemon, \"save_permissions\", requestIndexBefore, () => true, 2_000);"
      ];
    case "emit-permissions-refresh":
      return [
        "daemon.emit(\"settings:permissions:changed\", { reason: \"fuzz-refresh\", at: Date.now() });",
        "daemon.delayResponse(\"list_permissions\", () => true, 350);"
      ];
    case "open-workflows":
      return [
        "await clickFirstVisible(page, [",
        "  () => page.getByRole(\"button\", { name: /Workflows/i }).first(),",
        "  () => page.locator(\".pf-sidebar-item\").filter({ hasText: /Workflows/ }).first()",
        "], \"open Workflows\");"
      ];
    case "edit-workflow-node":
    case "edit-workflow-trigger":
      return [
        "await clickFirstVisible(page, [",
        "  () => page.locator(\"input, textarea\").first(),",
        "  () => page.locator(\"[contenteditable='true']\").first()",
        "], \"focus workflow editor\");",
        `await page.keyboard.type(${JSON.stringify(params.field ?? "fuzz")});`
      ];
    case "emit-workflow-refresh":
      return [
        "daemon.emit(\"workflow:list:changed\", { reason: \"fuzz-refresh\", at: Date.now() });",
        "daemon.delayResponse(\"workflow_list\", () => true, 350);"
      ];
    case "type-page-input":
      return [
        "await ensureReplayBrowserPage(page, daemon);",
        "requestIndexBefore = daemon.requests.length;",
        "const browserCanvas = page.locator(\".pf-browser-canvas, canvas\").first();",
        "await browserCanvas.click({ position: { x: 20, y: 20 }, timeout: 2_000 });",
        `await page.keyboard.type(${JSON.stringify(params.text ?? "")});`,
        "await waitForNewDaemonRequest(daemon, \"browser_input\", requestIndexBefore, () => true, 2_000);"
      ];
    case "special-key":
      return [
        "await ensureReplayBrowserPage(page, daemon);",
        "requestIndexBefore = daemon.requests.length;",
        "const browserCanvas = page.locator(\".pf-browser-canvas, canvas\").first();",
        "await browserCanvas.click({ position: { x: 20, y: 20 }, timeout: 2_000 });",
        `await page.keyboard.press(${JSON.stringify(params.key ?? "Enter")});`,
        "await waitForNewDaemonRequest(daemon, \"browser_input\", requestIndexBefore, () => true, 2_000);"
      ];
    case "emit-state-for-old-tab":
      return [
        `daemon.emit("browser:session-browser:browser:tab-1:state", ${JSON.stringify({ url: params.url ?? "https://stale.example.test", title: "Stale page", loading: params.loading === true, updatedAtMs: 1, width: 960, height: 720 })});`
      ];
    case "emit-empty-tab-list":
      return [
        "daemon.emit(\"browser:session-browser:tabs\", { activeTabId: null, tabs: [] });"
      ];
    case "emit-frame-burst":
      return [
        "for (const frame of " + JSON.stringify(params.frames ?? []) + ") {",
        "  daemon.emit(\"browser:session-browser:browser:tab-1:frame\", { frameId: `frame-${frame.width}-${frame.height}`, mimeType: \"image/png\", encoding: \"base64\", data: \"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAFgwJ/lzTnGQAAAABJRU5ErkJggg==\", width: frame.width, height: frame.height });",
        "}"
      ];
    case "fail-next-browser-command":
      return [
        "daemon.failNext(\"browser_agent\", \"fuzz injected browser failure\");"
      ];
    case "hold-next-browser-response":
      return [
        "daemon.delayResponse(\"browser_navigate\", () => true, 60_000);"
      ];
    case "disconnect-reconnect":
      return [
        "socketCountBeforeDisconnect = getFakeDaemonSocketCount(daemon);",
        "await disconnectFakeDaemonSockets(daemon);",
        "appendTraceEvent(trace, { type: \"daemon\", traceId, step: -1, event: \"forced-disconnect\" });",
        "await waitForFakeDaemonSocketCount(daemon, 0, 1_000).catch(() => undefined);",
        "await expect(page.locator(\"body\")).toContainText(/Disconnected|not connected|closed|reconnecting/i, { timeout: 3_000 }).catch(() => undefined);",
        "if (socketCountBeforeDisconnect > 0) {",
        "  await reconnectFakeDaemonIfNeeded(page, daemon, 5_000).catch(() => undefined);",
        "}"
      ];
    case "resize-narrow":
    case "resize-desktop":
      return [
        `await page.setViewportSize({ width: ${Number(params.width ?? 800)}, height: ${Number(params.height ?? 720)} });`
      ];
    default:
      return [
        `throw new Error("Unsupported replay action: ${String(step.action).replaceAll("\"", "\\\"")}");`
      ];
  }
}

function buildReplayDaemonOptions(testCase) {
  const needsExtraSessions = (testCase.steps ?? []).some((step) => step.action === "switch-session");
  const needsCoreHarness = ["chat-turn-race", "workspace-session-race", "provider-auth-model-race", "modal-focus-race"].includes(testCase.seedId);
  if (!needsExtraSessions && !needsCoreHarness) {
    return {};
  }
  const now = Date.now();
  return {
    sessions: [
      replaySession("session-browser", "Browser regression", "Browser seed transcript", now),
      replaySession("session-second", "Second session", "Second seed transcript", now - 1_000),
      replaySession("session-third", "Third session", "Third seed transcript", now - 2_000)
    ],
    providerModels: replayProviderModels(),
    externalCredentials: [
      {
        providerId: "codex",
        source: "codex",
        kind: "oauth",
        description: "Codex CLI OAuth credential",
        sourcePath: "/tmp/home/.codex/auth.json"
      },
      {
        providerId: "anthropic",
        source: "claude",
        kind: "api_key",
        description: "Claude CLI API key credential",
        sourcePath: "/tmp/home/.claude.json"
      }
    ]
  };
}

function replayProviderModels() {
  return {
    codex: [
      replayModel("test-model", "Test model", "codex", "openai-responses", true),
      replayModel("gpt-5.5", "GPT-5.5", "codex", "openai-responses", false)
    ],
    openai: [
      replayModel("test-model", "Test model", "openai", "openai-responses", true),
      replayModel("gpt-5.5", "GPT-5.5", "openai", "openai-responses", false)
    ],
    anthropic: [
      replayModel("test-model", "Test model", "anthropic", "anthropic-messages", true),
      replayModel("claude-sonnet-4-5", "Claude Sonnet 4.5", "anthropic", "anthropic-messages", false)
    ]
  };
}

function replayModel(id, displayName, provider, api, isDefault) {
  return {
    id,
    displayName,
    provider,
    api,
    contextWindow: 128000,
    maxOutputTokens: 4096,
    supportsReasoning: true,
    supportsTools: true,
    supportsVision: false,
    thinkingOptions: [
      { id: "low", label: "Low", description: "Low reasoning.", isDefault: true },
      { id: "high", label: "High", description: "High reasoning.", isDefault: false }
    ],
    defaultThinkingOptionId: "low",
    isDefault
  };
}

function replaySession(sessionId, displayName, text, updatedAtMs) {
  return {
    sessionId,
    displayName,
    title: displayName,
    cwd: "/tmp/puffer",
    folderPath: "/tmp/puffer",
    updatedAtMs,
    createdAtMs: updatedAtMs - 60_000,
    eventCount: 1,
    providerId: "codex",
    modelId: "test-model",
    timeline: [{ kind: "assistant_message", id: `${sessionId}-seed`, text, createdAtMs: updatedAtMs - 30_000 }]
  };
}

function sessionNameRegex(sessionId) {
  if (sessionId === "session-second") return "/^Second session\\b/";
  if (sessionId === "session-third") return "/^Third session\\b/";
  return "/^Browser regression\\b/";
}

function sessionDisplayName(sessionId) {
  if (sessionId === "session-second") return "Second session";
  if (sessionId === "session-third") return "Third session";
  return "Browser regression";
}

function providerLabelRegex(providerId) {
  if (providerId === "anthropic" || providerId === "claude") return "/Anthropic/i";
  return "/Codex|OpenAI/i";
}

function permissionButtonName(answer) {
  return answer === "deny" ? "Deny" : "Allow once";
}

function authStateForAction(action) {
  if (action === "emit-auth-one-provider-refresh") return "one-provider";
  if (action === "emit-auth-none-refresh") return "none";
  return "multi-provider";
}

export function defaultReplaySpecPath(testCase) {
  return path.join(
    "apps",
    "puffer-desktop",
    "tests",
    "fuzz",
    ".runs",
    "manual-replay",
    "tests",
    `${testCase.caseId}.replay.spec.ts`
  );
}

function replayHelperLines() {
  return [
    "type BrowserReplayState = {",
    "  addressValue: string;",
    "  activeTabText: string;",
    "  activeTabCount: number;",
    "  tabCount: number;",
    "  statusText: string;",
    "  errorText: string;",
    "  loadingText: string;",
    "};",
    "",
    "async function clickFirstVisible(page, candidates, label: string): Promise<void> {",
    "  const errors: string[] = [];",
    "  for (const candidate of candidates) {",
    "    const locator = candidate();",
    "    try {",
    "      await locator.click({ timeout: 5_000 });",
    "      return;",
    "    } catch (error) {",
    "      errors.push(String(error));",
    "    }",
    "  }",
    "  throw new Error(`${label} failed. Tried ${candidates.length} locator(s). Last error: ${errors.at(-1) ?? \"none\"}`);",
    "}",
    "",
    "async function clickFirstVisibleFast(page, candidates, label: string): Promise<void> {",
    "  const errors: string[] = [];",
    "  for (const candidate of candidates) {",
    "    const locator = candidate();",
    "    try {",
    "      await locator.click({ timeout: 750 });",
    "      return;",
    "    } catch (error) {",
    "      errors.push(String(error));",
    "    }",
    "  }",
    "  throw new Error(`${label} failed. Tried ${candidates.length} locator(s). Last error: ${errors.at(-1) ?? \"none\"}`);",
    "}",
    "",
    "async function clickBrowserTabIfPresent(page, requestedIndex: number): Promise<void> {",
    "  const tabs = page.locator(\".pf-browser-tab\");",
    "  const count = await tabs.count().catch(() => 0);",
    "  if (count === 0) return;",
    "  await tabs.nth(Math.max(0, Math.min(requestedIndex, count - 1))).click({ timeout: 5_000 });",
    "}",
    "",
    "async function openAgentTab(page, name: string): Promise<void> {",
    "  if (!(await page.locator(\".pf-agent-tabs\").first().isVisible({ timeout: 500 }).catch(() => false))) {",
    "    await clickFirstVisible(page, [",
    "      () => page.locator(\".pf-sidebar-agents-list\").getByRole(\"button\", { name: /^Browser regression\\b/ }).first(),",
    "      () => page.getByRole(\"region\", { name: \"Session history\" }).getByRole(\"button\", { name: /^Browser regression\\b/ }).first(),",
    "      () => page.getByRole(\"button\", { name: /^Browser regression\\b/ }).first()",
    "    ], \"open Browser regression agent\");",
    "    await expect(page.locator(\".pf-agent-tabs\")).toBeVisible({ timeout: 5_000 });",
    "  }",
    "  await clickFirstVisible(page, [",
    "    () => page.locator(\".pf-agent-tabs\").getByRole(\"button\", { name, exact: true }),",
    "    () => page.getByRole(\"button\", { name, exact: true }),",
    "    () => page.getByRole(\"tab\", { name, exact: true })",
    "  ], `open ${name} tab`);",
    "}",
    "",
    "async function openSettingsPane(page, label: string, fallbackText: RegExp): Promise<void> {",
    "  await closeReplayModalIfOpen(page);",
    "  if ((await page.locator(\".pf-settings-nav-item\").count().catch(() => 0)) === 0) {",
    "    await clickFirstVisible(page, [",
    "      () => page.getByRole(\"button\", { name: \"Settings\", exact: true }),",
    "      () => page.locator(\".pf-sidebar-item\").filter({ hasText: \"Settings\" })",
    "    ], \"open Settings\");",
    "  }",
    "  await clickFirstVisible(page, [",
    "    () => page.getByRole(\"button\", { name: label, exact: true }),",
    "    () => page.getByRole(\"tab\", { name: label, exact: true }),",
    "    () => page.locator(\".pf-settings-nav-item\").filter({ hasText: fallbackText })",
    "  ], `open ${label} settings`).catch(() => undefined);",
    "}",
    "",
    "async function clickReplayFileTreeFile(page): Promise<void> {",
    "  const tree = page.locator(\".pf-files-pane aside.tree\");",
    "  await expect(tree).toBeVisible({ timeout: 5_000 });",
    "  const fileRow = tree.locator(\"button.row\").filter({ hasText: /\\.(md|rs|json|toml|ts|tsx|js|svelte|txt)$/i }).first();",
    "  if (await fileRow.isVisible({ timeout: 500 }).catch(() => false)) {",
    "    await fileRow.click({ timeout: 2_000 });",
    "    return;",
    "  }",
    "  const expandable = tree.locator(\"button.row[aria-expanded='false']\").first();",
    "  if (await expandable.isVisible({ timeout: 1_000 }).catch(() => false)) {",
    "    await expandable.click({ timeout: 2_000 });",
    "  }",
    "  await expect(fileRow).toBeVisible({ timeout: 3_000 });",
    "  await fileRow.click({ timeout: 2_000 });",
    "}",
    "",
    "async function ensureReplayFileDirty(page): Promise<void> {",
    "  const saveButton = page.getByRole(\"button\", { name: /Save/i }).first();",
    "  if (await saveButton.isVisible({ timeout: 500 }).catch(() => false)) return;",
    "  await clickFirstVisible(page, [",
    "    () => page.locator(\"textarea, [contenteditable='true']\").last(),",
    "    () => page.locator(\".pf-file-editor textarea, .cm-content\").first()",
    "  ], \"focus file editor before save\");",
    "  await page.keyboard.type(\"\\n// fuzz save\");",
    "  await expect(saveButton).toBeVisible({ timeout: 2_000 });",
    "}",
    "",
    "async function ensureReplayBrowserPage(page, daemon): Promise<void> {",
    "  const canvas = page.locator(\".pf-browser-canvas, canvas\").first();",
    "  if (await canvas.isVisible({ timeout: 500 }).catch(() => false)) return;",
    "  const addTab = page.locator(\".pf-browser-tab-add\").first();",
    "  const fallbackAddTab = page.getByRole(\"button\", { name: /^New tab$/i }).last();",
    "  if (!(await addTab.isVisible({ timeout: 500 }).catch(() => false)) && !(await fallbackAddTab.isVisible({ timeout: 500 }).catch(() => false))) return;",
    "  const startIndex = daemon.requests.length;",
    "  if (await addTab.isVisible({ timeout: 500 }).catch(() => false)) {",
    "    await addTab.click({ timeout: 2_000 }).catch(() => undefined);",
    "  } else {",
    "    await fallbackAddTab.click({ timeout: 2_000 }).catch(() => undefined);",
    "  }",
    "  await waitForNewDaemonRequest(daemon, \"browser_agent\", startIndex, (request) => request.params.action === \"open\", 2_000).catch(() => undefined);",
    "  await expect(canvas).toBeVisible({ timeout: 2_000 }).catch(() => undefined);",
    "}",
    "",
    "async function ensureReplayTerminalOpen(page, daemon): Promise<void> {",
    "  const pane = page.locator(\".pf-terminal-pane\");",
    "  const terminal = pane.locator(\".xterm, .xterm-screen, .xterm-helper-textarea\").first();",
    "  if (await terminal.isVisible({ timeout: 500 }).catch(() => false)) return;",
    "  const startIndex = daemon.requests.length;",
    "  await clickFirstVisible(page, [",
    "    () => page.getByRole(\"button\", { name: /^New terminal$/i }).first(),",
    "    () => page.getByRole(\"button\", { name: /^Open terminal$/i }).first(),",
    "    () => pane.getByRole(\"button\", { name: /New terminal|Open terminal/i }).first()",
    "  ], \"open terminal before input\").catch(() => undefined);",
    "  await waitForNewDaemonRequest(daemon, \"pty_open\", startIndex, () => true, 2_000).catch(() => undefined);",
    "  await expect(terminal).toBeVisible({ timeout: 2_000 }).catch(() => undefined);",
    "}",
    "",
    "async function focusReplayTerminalInput(page): Promise<void> {",
    "  const pane = page.locator(\".pf-terminal-pane\");",
    "  await expect(pane).toBeVisible({ timeout: 5_000 });",
    "  const input = pane.getByRole(\"textbox\", { name: \"Terminal input\" }).first();",
    "  if ((await input.count().catch(() => 0)) > 0) {",
    "    await input.focus({ timeout: 2_000 }).catch(() => undefined);",
    "    const focused = await page.evaluate(() => {",
    "      const active = document.activeElement as HTMLElement | null;",
    "      return active?.getAttribute(\"aria-label\") === \"Terminal input\" || active?.classList.contains(\"xterm-helper-textarea\") === true;",
    "    });",
    "    if (focused) return;",
    "  }",
    "  await clickFirstVisible(page, [",
    "    () => pane.locator(\".xterm-helper-textarea\").first(),",
    "    () => pane.locator(\".xterm-screen\").first(),",
    "    () => pane.locator(\".xterm\").first()",
    "  ], \"focus terminal input\");",
    "  await page.waitForTimeout(50);",
    "}",
    "",
    "async function closeReplayModalIfOpen(page): Promise<void> {",
    "  const dialogs = page.getByRole(\"dialog\");",
    "  if ((await dialogs.count().catch(() => 0)) === 0) return;",
    "  await page.keyboard.press(\"Escape\").catch(() => undefined);",
    "  await expect(dialogs.first()).toBeHidden({ timeout: 1_000 }).catch(() => undefined);",
    "}",
    "",
    "async function ensureReplayBodyVisible(page): Promise<void> {",
    "  await page.waitForLoadState(\"domcontentloaded\", { timeout: 15_000 }).catch(() => undefined);",
    "  if (await page.locator(\"body\").isVisible().catch(() => false)) return;",
    "  await page.reload({ waitUntil: \"domcontentloaded\", timeout: 15_000 }).catch(() => undefined);",
    "  await expect(page.locator(\"body\")).toBeVisible({ timeout: 20_000 });",
    "}",
    "",
    "async function ensureWorkspaceOpen(page): Promise<void> {",
    "  await closeReplayModalIfOpen(page);",
    "  if ((await page.getByLabel(\"Search workspace\").count().catch(() => 0)) > 0) return;",
    "  const backToWorkspace = page.getByRole(\"button\", { name: \"Back\", exact: true });",
    "  if ((await backToWorkspace.count().catch(() => 0)) > 0) {",
    "    await page.getByRole(\"button\", { name: /Reconnect/ }).click({ timeout: 750 }).catch(() => undefined);",
    "    await page.waitForTimeout(100);",
    "    await backToWorkspace.click({ timeout: 1_000 }).catch(async () => {",
    "      await backToWorkspace.click({ timeout: 1_000, force: true }).catch(async () => {",
    "        await backToWorkspace.evaluate((button) => (button as HTMLButtonElement).click()).catch(() => undefined);",
    "      });",
    "    });",
    "    if (await page.getByLabel(\"Search workspace\").isVisible({ timeout: 3_000 }).catch(() => false)) return;",
    "  }",
    "  await clickFirstVisible(page, [",
    "    () => page.getByRole(\"button\", { name: \"Project\", exact: true }),",
    "    () => page.locator(\".pf-sidebar-item\").filter({ hasText: \"Project\" })",
    "  ], \"open Project workspace\");",
    "  await expect(page.getByLabel(\"Search workspace\")).toBeVisible({ timeout: 5_000 });",
    "}",
    "",
    "async function ensureProvidersOpen(page): Promise<void> {",
    "  await closeReplayModalIfOpen(page);",
    "  await clickFirstVisible(page, [",
    "    () => page.getByRole(\"button\", { name: \"Settings\", exact: true }),",
    "    () => page.locator(\".pf-sidebar-item\").filter({ hasText: \"Settings\" })",
    "  ], \"open Settings\");",
    "  await clickFirstVisible(page, [",
    "    () => page.getByRole(\"button\", { name: \"Providers\", exact: true }),",
    "    () => page.getByRole(\"tab\", { name: \"Providers\", exact: true }),",
    "    () => page.locator(\".pf-settings-nav-item\").filter({ hasText: \"Providers\" })",
    "  ], \"open Providers settings\");",
    "  await expect(page.locator(\".pf-settings-pane\")).toContainText(\"Providers\", { timeout: 5_000 });",
    "}",
    "",
    "async function clickReplaySession(page, sessionId: string, label: string): Promise<void> {",
    "  const name = replaySessionRegex(sessionId);",
    "  if ((await page.getByRole(\"button\", { name }).count().catch(() => 0)) === 0 &&",
    "      (await page.getByRole(\"button\", { name: \"Back\" }).count().catch(() => 0)) > 0) {",
    "    await page.getByRole(\"button\", { name: \"Back\" }).click({ timeout: 5_000 });",
    "    await expect(page.getByLabel(\"Search workspace\")).toBeVisible({ timeout: 5_000 });",
    "  }",
    "  await clickFirstVisible(page, [",
    "    () => page.locator(\".pf-sidebar-agents-list\").getByRole(\"button\", { name }).first(),",
    "    () => page.getByRole(\"region\", { name: \"Session history\" }).getByRole(\"button\", { name }).first(),",
    "    () => page.getByRole(\"button\", { name }).first()",
    "  ], label);",
    "}",
    "",
    "async function assertDialogOwnsFocus(page, dialogName: string): Promise<void> {",
    "  const dialog = page.getByRole(\"dialog\", { name: dialogName });",
    "  await expect(dialog).toBeVisible({ timeout: 5_000 });",
    "  const activeSummary = await page.evaluate(() => {",
    "    const active = document.activeElement as HTMLElement | null;",
    "    return {",
    "      tag: active?.tagName ?? \"none\",",
    "      text: (active?.innerText || active?.textContent || active?.getAttribute(\"aria-label\") || active?.getAttribute(\"title\") || \"\").trim().slice(0, 120)",
    "    };",
    "  });",
    "  const focusInside = await dialog.evaluate((node) => node.contains(document.activeElement));",
    "  expect(focusInside, `${dialogName} should receive focus when opened; active=${JSON.stringify(activeSummary)}`).toBe(true);",
    "}",
    "",
    "async function assertDialogTrapsTabFocus(page, dialogName: string, tabs: number): Promise<void> {",
    "  const dialog = page.getByRole(\"dialog\", { name: dialogName });",
    "  await expect(dialog).toBeVisible({ timeout: 5_000 });",
    "  const focusable = dialog.locator(\"button, [href], input, select, textarea, [tabindex]:not([tabindex='-1'])\").first();",
    "  await expect(focusable).toBeVisible({ timeout: 5_000 });",
    "  await focusable.focus();",
    "  for (let index = 0; index < tabs; index += 1) {",
    "    await page.keyboard.press(index % 5 === 4 ? \"Shift+Tab\" : \"Tab\");",
    "    const focusInside = await dialog.evaluate((node) => node.contains(document.activeElement));",
    "    const activeSummary = await page.evaluate(() => {",
    "      const active = document.activeElement as HTMLElement | null;",
    "      return {",
    "        tag: active?.tagName ?? \"none\",",
    "        text: (active?.innerText || active?.textContent || active?.getAttribute(\"aria-label\") || active?.getAttribute(\"title\") || \"\").trim().slice(0, 120)",
    "      };",
    "    });",
    "    expect(focusInside, `${dialogName} focus escaped after Tab ${index + 1}; active=${JSON.stringify(activeSummary)}`).toBe(true);",
    "  }",
    "}",
    "",
    "function replaySessionRegex(sessionId: string): RegExp {",
    "  if (sessionId === \"session-second\") return /^Second session\\b/;",
    "  if (sessionId === \"session-third\") return /^Third session\\b/;",
    "  return /^Browser regression\\b/;",
    "}",
    "",
    "function replaySessionDisplayName(sessionId: string): string {",
    "  if (sessionId === \"session-second\") return \"Second session\";",
    "  if (sessionId === \"session-third\") return \"Third session\";",
    "  if (sessionId.startsWith(\"session-created-\")) return \"New Session\";",
    "  return \"Browser regression\";",
    "}",
    "",
    "function normalizeReplayProviderId(providerId: string): string {",
    "  const normalized = String(providerId || \"codex\").trim().toLowerCase();",
    "  if (normalized === \"openai\") return \"codex\";",
    "  if (normalized === \"claude\") return \"anthropic\";",
    "  return normalized || \"codex\";",
    "}",
    "",
    "function canonicalReplayProviderId(providerId: string): string {",
    "  const normalized = String(providerId || \"\").trim().toLowerCase();",
    "  if (normalized === \"codex\" || normalized === \"openai\") return \"openai\";",
    "  if (normalized === \"claude\" || normalized === \"anthropic\") return \"anthropic\";",
    "  return normalized;",
    "}",
    "",
    "function providerIdsMatch(left: string, right: string): boolean {",
    "  return canonicalReplayProviderId(left) === canonicalReplayProviderId(right);",
    "}",
    "",
    "function providerDisplayName(providerId: string): string {",
    "  return providerIdsMatch(providerId, \"anthropic\") ? \"Anthropic\" : \"Codex\";",
    "}",
    "",
    "function providerCard(page, providerId: string) {",
    "  return page.locator(\".provider-card\").filter({ hasText: providerDisplayName(providerId) }).first();",
    "}",
    "",
    "function requestProviderMatches(request, providerId: string): boolean {",
    "  return providerIdsMatch(String(request.params.providerId ?? request.params.defaultProvider ?? \"\"), providerId);",
    "}",
    "",
    "async function ensureReplayComposerRoute(page): Promise<void> {",
    "  if ((await page.locator(\".pf-composer textarea\").first().isVisible().catch(() => false))) return;",
    "  await page.locator(\".pf-sidebar-agents-list\").getByRole(\"button\", { name: /^Browser regression\\b/ }).click({ timeout: 1_000 }).catch(() => undefined);",
    "  await page.getByRole(\"button\", { name: /Browser regression/ }).first().click({ timeout: 1_000 }).catch(() => undefined);",
    "  await page.waitForTimeout(50);",
    "}",
    "",
    "async function fillReplayComposerIfPossible(page, text: string): Promise<void> {",
    "  await ensureReplayComposerRoute(page);",
    "  const composer = page.locator(\".pf-composer textarea\").first();",
    "  if (!(await composer.isVisible().catch(() => false))) return;",
    "  if (!(await composer.isEnabled().catch(() => false))) return;",
    "  await composer.fill(text, { timeout: 2_000 }).catch(() => undefined);",
    "}",
    "",
    "async function sendReplayPromptIfPossible(page, daemon, rapid: boolean) {",
    "  await ensureReplayComposerRoute(page);",
    "  const send = page.getByRole(\"button\", { name: \"Send\", exact: true });",
    "  if ((await send.count().catch(() => 0)) === 0) return null;",
    "  if (!(await send.isEnabled().catch(() => false))) return null;",
    "  const startIndex = daemon.requests.length;",
    "  if (rapid) {",
    "    await send.evaluate((button) => {",
    "      (button as HTMLButtonElement).click();",
    "      (button as HTMLButtonElement).click();",
    "    }).catch(() => undefined);",
    "  } else {",
    "    await send.click({ timeout: 2_000 }).catch(() => undefined);",
    "  }",
    "  return await waitForNewDaemonRequest(daemon, \"run_agent_turn\", startIndex, () => true, 3_000).catch(() => null);",
    "}",
    "",
    "function replayAuthStatuses(authState: string) {",
    "  if (authState === \"none\") {",
    "    return [];",
    "  }",
    "  if (authState === \"one-provider\") {",
    "    return [{ providerId: \"codex\", kind: \"oauth\", email: \"tester@example.com\", expiresAtMs: null, scopes: [], planType: \"test\", organizationName: null }];",
    "  }",
    "  return [",
    "    { providerId: \"codex\", kind: \"oauth\", email: \"tester@example.com\", expiresAtMs: null, scopes: [], planType: \"test\", organizationName: null },",
    "    { providerId: \"anthropic\", kind: \"api_key\", email: null, expiresAtMs: null, scopes: [], planType: null, organizationName: null }",
    "  ];",
    "}",
    "",
    "async function changeReplayModel(page, daemon, modelSpec: string): Promise<void> {",
    "  const slash = modelSpec.indexOf(\"/\");",
    "  const providerId = normalizeReplayProviderId(slash > 0 ? modelSpec.slice(0, slash) : \"codex\");",
    "  const modelId = slash > 0 ? modelSpec.slice(slash + 1) : modelSpec;",
    "  const picker = page.locator(\".pf-composer .picker\").first();",
    "  const trigger = picker.locator(\".trigger\");",
    "  if ((await trigger.count().catch(() => 0)) === 0) return;",
    "  if (!(await trigger.isEnabled().catch(() => false))) return;",
    "  let startIndex = daemon.requests.length;",
    "  await trigger.click({ timeout: 5_000 });",
    "  await waitForNewDaemonRequest(daemon, \"list_provider_models\", startIndex, () => true, 3_000).catch(() => undefined);",
    "  const providerButton = picker.locator(\".providers\").getByRole(\"button\", { name: providerDisplayName(providerId) });",
    "  if ((await providerButton.count().catch(() => 0)) > 0) {",
    "    startIndex = daemon.requests.length;",
    "    await providerButton.click({ timeout: 5_000 });",
    "    await waitForNewDaemonRequest(daemon, \"list_provider_models\", startIndex, (request) => requestProviderMatches(request, providerId), 3_000).catch(() => undefined);",
    "  }",
    "  const target = picker.locator(\".row\").filter({ hasText: modelId }).first();",
    "  if ((await target.count().catch(() => 0)) > 0) {",
    "    await target.click({ timeout: 5_000 });",
    "  } else {",
    "    await picker.locator(\".row\").first().click({ timeout: 5_000 });",
    "  }",
    "  await expect(picker.locator(\".menu\")).toHaveCount(0, { timeout: 5_000 });",
    "}",
    "",
    "async function saveReplayDefaultModel(page, daemon, provider: string, model: string): Promise<void> {",
    "  const providerId = normalizeReplayProviderId(provider);",
    "  const pane = page.locator(\".pf-settings-pane\");",
    "  const providerSelect = pane.getByLabel(\"Provider\");",
    "  let startIndex = daemon.requests.length;",
    "  await providerSelect.selectOption(providerId).catch(async () => {",
    "    await providerSelect.selectOption(canonicalReplayProviderId(providerId));",
    "  });",
    "  await waitForNewDaemonRequest(daemon, \"list_provider_models\", startIndex, (request) => requestProviderMatches(request, providerId), 3_000).catch(() => undefined);",
    "  const modelSelect = pane.getByLabel(\"Model\");",
    "  await expect(modelSelect).toBeEnabled({ timeout: 5_000 });",
    "  await selectReplayModelOption(modelSelect, model);",
    "  startIndex = daemon.requests.length;",
    "  await pane.getByRole(\"button\", { name: \"Save default\" }).click({ timeout: 5_000 });",
    "  await waitForNewDaemonRequest(daemon, \"update_config\", startIndex, (request) => requestProviderMatches(request, providerId));",
    "}",
    "",
    "async function selectReplayModelOption(modelSelect, preferredModel: string): Promise<void> {",
    "  const values = await modelSelect.locator(\"option\").evaluateAll((options) => options.map((option) => (option as HTMLOptionElement).value).filter(Boolean));",
    "  const value = values.includes(preferredModel) ? preferredModel : values[0];",
    "  if (!value) throw new Error(\"No selectable model option is available\");",
    "  await modelSelect.selectOption(value);",
    "}",
    "",
    "function installUniqueReplayTurnIds(daemon) {",
    "  const unsafe = daemon as any;",
    "  const originalDispatch = typeof unsafe.dispatch === \"function\" ? unsafe.dispatch.bind(daemon) : null;",
    "  const activeTurnIds = new Set<string>();",
    "  const latestBySession = new Map<string, string>();",
    "  let counter = 0;",
    "  if (originalDispatch) {",
    "    unsafe.dispatch = (request) => {",
    "      if (request.method === \"run_agent_turn\") {",
    "        const sessionId = String(request.params?.sessionId ?? \"session-browser\");",
    "        const turnId = `turn-${sessionId}-${++counter}`;",
    "        activeTurnIds.add(turnId);",
    "        latestBySession.set(sessionId, turnId);",
    "        return { turnId };",
    "      }",
    "      if (request.method === \"cancel_turn\") {",
    "        const turnId = String(request.params?.turnId ?? \"\");",
    "        if (activeTurnIds.has(turnId)) return { ok: true };",
    "      }",
    "      return originalDispatch(request);",
    "    };",
    "  }",
    "  return {",
    "    latestForSession(sessionId: string): string | null {",
    "      return latestBySession.get(sessionId) ?? null;",
    "    },",
    "    clear(turnId: string | null): void {",
    "      if (turnId) activeTurnIds.delete(turnId);",
    "    }",
    "  };",
    "}",
    "",
    "function emitReplayConnectorQuestions(daemon, sessionId: string | null, turnId: string | null): void {",
    "  if (!sessionId || !turnId) return;",
    "  daemon.emit(`session:${sessionId}:event`, {",
    "    type: \"user-question-request\",",
    "    turnId,",
    "    requestId: \"connector-setup\",",
    "    questions: [",
    "      { type: \"input\", header: \"Credential\", question: \"Connector credential\", options: [] },",
    "      {",
    "        type: \"choice\",",
    "        header: \"Mode\",",
    "        question: \"Setup mode\",",
    "        options: [",
    "          { label: \"Default\", description: \"Use recommended defaults\", preview: \"default\" },",
    "          { label: \"Manual\", description: \"Enter all values manually\", preview: \"manual\" }",
    "        ]",
    "      }",
    "    ]",
    "  });",
    "}",
    "",
    "function emitReplayConnectorComplete(daemon, sessionId: string | null, turnId: string | null, connectionSlug: string | null): void {",
    "  if (!sessionId || !turnId) return;",
    "  daemon.emit(`session:${sessionId}:event`, {",
    "    type: \"turn-complete\",",
    "    turnId,",
    "    assistantText: `Created connector connection ${connectionSlug ?? \"fuzz-connector\"}.`",
    "  });",
    "}",
    "",
    "async function waitForDaemonRequest(daemon, method: string, predicate = () => true, timeoutMs = 5_000) {",
    "  let timer: ReturnType<typeof setTimeout> | null = null;",
    "  try {",
    "    return await Promise.race([",
    "      daemon.waitForRequest(method, predicate),",
    "      new Promise((_, reject) => {",
    "        timer = setTimeout(() => reject(new Error(`Timed out waiting for daemon request ${method}`)), timeoutMs);",
    "      })",
    "    ]);",
    "  } finally {",
    "    if (timer) clearTimeout(timer);",
    "  }",
    "}",
    "",
    "async function waitForNewDaemonRequest(daemon, method: string, startIndex: number, predicate = () => true, timeoutMs = 5_000) {",
    "  return waitForDaemonRequest(daemon, method, (request) => {",
    "    const index = daemon.requests.indexOf(request);",
    "    return index >= startIndex && predicate(request);",
    "  }, timeoutMs);",
    "}",
    "",
    "function getFakeDaemonSocketCount(daemon): number {",
    "  if (typeof daemon.socketCount === \"function\") return daemon.socketCount();",
    "  const sockets = (daemon as any).sockets;",
    "  if (sockets && typeof sockets.size === \"number\") return sockets.size;",
    "  return 0;",
    "}",
    "",
    "async function disconnectFakeDaemonSockets(daemon): Promise<void> {",
    "  if (typeof daemon.disconnectAllSockets === \"function\") {",
    "    await daemon.disconnectAllSockets();",
    "    return;",
    "  }",
    "  const sockets = [...(((daemon as any).sockets ?? []) as Iterable<{ close: (options?: { code?: number; reason?: string }) => Promise<void> }>)];",
    "  await Promise.all(sockets.map((socket) => socket.close({ code: 1011, reason: \"fuzz forced reconnect\" }).catch(() => undefined)));",
    "}",
    "",
    "async function waitForFakeDaemonSocketCount(daemon, count: number, timeoutMs = 5_000): Promise<void> {",
    "  if (typeof daemon.waitForSocketCount === \"function\") {",
    "    await daemon.waitForSocketCount(count, timeoutMs);",
    "    return;",
    "  }",
    "  const deadline = Date.now() + timeoutMs;",
    "  while (getFakeDaemonSocketCount(daemon) !== count) {",
    "    if (Date.now() >= deadline) {",
    "      throw new Error(`Timed out waiting for ${count} fake daemon socket(s); saw ${getFakeDaemonSocketCount(daemon)}.`);",
    "    }",
    "    await new Promise((resolve) => setTimeout(resolve, 25));",
    "  }",
    "}",
    "",
    "async function reconnectFakeDaemonIfNeeded(page, daemon, timeoutMs = 5_000): Promise<void> {",
    "  const reconnect = page.getByRole(\"button\", { name: /Reconnect/ });",
    "  if (getFakeDaemonSocketCount(daemon) > 0 && (await reconnect.count().catch(() => 0)) === 0) return;",
    "  await reconnect.click({ timeout: 1_000 }).catch(() => undefined);",
    "  await waitForFakeDaemonSocketCount(daemon, 1, timeoutMs).catch(() => undefined);",
    "}",
    "",
    "function staleReplaySessionId(activeSessionId: string): string {",
    "  return activeSessionId === \"session-browser\" ? \"session-second\" : \"session-browser\";",
    "}",
    "",
    "async function collectBrowserReplayState(page): Promise<BrowserReplayState> {",
    "  const activeTab = page.locator(\".pf-browser-tab[aria-selected='true'], .pf-browser-tab[data-active='true'], .pf-browser-tab.active\");",
    "  const loadingText = await page.locator(\"body\").innerText({ timeout: 1_000 }).catch(() => \"\");",
    "  return {",
    "    addressValue: await page.locator(\".pf-browser-address\").first().inputValue({ timeout: 500 }).catch(() => \"\"),",
    "    activeTabText: await activeTab.first().innerText({ timeout: 500 }).catch(() => \"\"),",
    "    activeTabCount: await activeTab.count().catch(() => 0),",
    "    tabCount: await page.locator(\".pf-browser-tab\").count().catch(() => 0),",
    "    statusText: await page.locator(\".pf-browser-status\").first().innerText({ timeout: 500 }).catch(() => \"\"),",
    "    errorText: await page.locator(\".pf-browser-error\").allInnerTexts().then((items) => items.join(\"\\n\")).catch(() => \"\"),",
    "    loadingText",
    "  };",
    "}",
    "",
    "async function assertReplayInvariants(page, daemon, trace, traceId, metadata, initialBrowserState: BrowserReplayState, finalBrowserState: BrowserReplayState, activeSessionId: string): Promise<void> {",
    "  appendTraceEvent(trace, { type: \"state\", traceId, step: 9_999, state: await collectPufferUiState(page, { viewport: \"desktop\", browserOrShell: \"chromium\", fakeDaemon: true }), initialBrowserState, finalBrowserState });",
    "  expect(page.isClosed()).toBe(false);",
    "  await expect(page.locator(\"body\")).toBeVisible();",
    "  const runtimeErrors = trace.filter((event) => event.type === \"pageerror\" || (event.type === \"console\" && /TypeError|ReferenceError|Unhandled|Cannot read|Cannot set/i.test(String(event.text ?? \"\"))));",
    "  expect(runtimeErrors, JSON.stringify(runtimeErrors.slice(0, 3))).toHaveLength(0);",
    "",
    "  if (metadata.coverage.includes(\"invariant:active-tab-stable\") || metadata.coverage.includes(\"async:stale-tab-event\")) {",
    "    const state = await collectBrowserReplayState(page);",
    "    if (state.tabCount > 0) expect(state.activeTabCount, \"browser should keep one reachable active tab when tabs exist\").toBeGreaterThan(0);",
    "    for (const staleUrl of metadata.staleUrls) {",
    "      expect(state.addressValue, `stale tab URL leaked into active address: ${staleUrl}`).not.toBe(staleUrl);",
    "    }",
    "  }",
    "",
    "  if (metadata.coverage.includes(\"invariant:no-permanent-loading\") && !metadata.hasDroppedResponse) {",
    "    await expect.poll(async () => {",
    "      const state = await collectBrowserReplayState(page);",
    "      return /loading|connecting|pending/i.test(`${state.statusText} ${state.loadingText}`);",
    "    }, { timeout: 3_000 }).toBe(false);",
    "  }",
    "",
    "  if (metadata.coverage.includes(\"invariant:draft-preserved-on-failure\") && metadata.hasInjectedFailure && metadata.recoverableTypedUrls.length > 0) {",
    "    const state = await collectBrowserReplayState(page);",
    "    const lastTypedUrl = metadata.recoverableTypedUrls[metadata.recoverableTypedUrls.length - 1];",
    "    expect(`${state.addressValue} ${state.errorText}`, \"typed browser URL should remain recoverable after injected failure\").toContain(lastTypedUrl);",
    "  }",
    "",
    "  if (metadata.coverage.includes(\"invariant:active-session-stable\") || metadata.coverage.includes(\"async:stale-session-event\")) {",
    "    if ((await page.locator(\".pf-agent-detail\").count().catch(() => 0)) > 0) {",
    "      await expect(page.locator(\".pf-agent-detail\")).toContainText(replaySessionDisplayName(activeSessionId), { timeout: 3_000 });",
    "    }",
    "  }",
    "",
    "  if (metadata.coverage.includes(\"invariant:no-cross-provider-model\")) {",
    "    const badProviderModel = daemon.requests.find((request) => providerModelLooksCrossed(request));",
    "    expect(badProviderModel, `provider/model crossed in request: ${JSON.stringify(badProviderModel)}`).toBeUndefined();",
    "  }",
    "",
    "  if (metadata.hasDuplicateSubmit) {",
    "    const excessNavigates = maxRequestCountAboveExpected(daemon.requests, \"browser_navigate\", browserNavigateDuplicateKey, metadata.expectedNavigateCountsByUrl);",
    "    const excessTurns = maxRequestCountAboveExpected(daemon.requests, \"run_agent_turn\", (params) => String(params.message ?? \"\"), metadata.expectedTurnCountsByMessage);",
    "    expect(excessNavigates, \"one browser navigate request per URL submit intent\").toBeLessThanOrEqual(0);",
    "    expect(excessTurns, \"one chat turn request per prompt submit intent\").toBeLessThanOrEqual(0);",
    "  }",
    "}",
    "",
    "function maxRequestCountAboveExpected(requests, method: string, keyForParams: (params) => string, expectedByKey: Record<string, number>): number {",
    "  const counts = new Map<string, number>();",
    "  for (const request of requests) {",
    "    if (request.method !== method) continue;",
    "    const key = keyForParams(request.params);",
    "    if (!key) continue;",
    "    counts.set(key, (counts.get(key) ?? 0) + 1);",
    "  }",
    "  let maxExcess = 0;",
    "  for (const [key, count] of counts) {",
    "    maxExcess = Math.max(maxExcess, count - Number(expectedByKey[key] ?? 0));",
    "  }",
    "  return maxExcess;",
    "}",
    "",
    "function browserNavigateDuplicateKey(params): string {",
    "  const url = String(params.url ?? \"\").trim();",
    "  if (!url || url === \"about:blank\") return \"\";",
    "  return url;",
    "}",
    "",
    "function providerModelLooksCrossed(request): boolean {",
    "  const params = request.params ?? {};",
    "  const provider = canonicalReplayProviderId(String(params.providerId ?? params.defaultProvider ?? \"\"));",
    "  const model = String(params.modelId ?? params.defaultModel ?? \"\").toLowerCase();",
    "  if (!provider || !model) return false;",
    "  if (provider === \"anthropic\") return /gpt|codex|openai/.test(model);",
    "  if (provider === \"openai\") return /claude|anthropic/.test(model);",
    "  return false;",
    "}",
    ""
  ];
}
