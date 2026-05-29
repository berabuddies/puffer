# Momo Chat 迁 daemon — Implementation Plan (阶段 1)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 momo 的 chat 从 `spawn 'puffer non-interactive'` 迁到「前端直连 puffer daemon」,照搬 puffer-desktop 的聊天前端(阶段 1 用 desktop 风格 UI 跑通)。

**Architecture:** 前端复用 task monitor 已落地的 `daemonClient`(直连 daemon)。新增瘦 RPC 层 `lib/agent/daemonChat.ts` + 摘自 desktop App.svelte 的 keyed-by-sessionId 状态机 `lib/agent/agentChat.svelte.ts` + 搬来的 `ConversationView` 组件树。session 由 daemon 管,worldrouter key 经 `login_with_api_key("openai")` + base_url override 接入。momo backend 的 chat 路径变死代码(stub 掉)。

**Tech Stack:** Tauri + Svelte 5 (runes), TypeScript, Playwright (`test:desktop-ui`), Rust (daemon RPC over ws)。

**Spec:** `docs/superpowers/specs/2026-05-29-momo-chat-daemon-migration-design.md`

---

## 文件结构(决策锁定)

**新建(`apps/momo/src/lib/agent/`):**
- `sessionEvents.ts` — `SessionStreamEvent` 联合类型 + `subscribeSessionEvents`(搬自 desktop `lib/api/sessionEvents.ts`,client 换 momo `ensureDaemonClient`)
- `types.ts` — `TimelineItem` 族 + `SessionDetail`/`SessionListItem`/`AskUserQuestionItem`(搬自 desktop `lib/types.ts` 的 chat 子集)
- `daemonChat.ts` — 瘦 RPC 封装(create_session/run_agent_turn/resolve_*/cancel/load_session_detail/list_grouped_sessions) + `normalizeTimelineItem`/`normalizeSessionDetail`(搬自 desktop `desktop.ts`,**剥 Tauri-invoke/browser-preview/mockData**)
- `agentChat.svelte.ts` — 状态机 controller(keyed by sessionId)
- `ConversationView.svelte` + `components/`(ToolCard/QuestionPrompt/Approval/DiffCard/MessageBody/HighlightedLine/Icon/Puffer/BrandLogo) + `codeHighlight.ts` + `chat.css`(搬自 desktop,删 ModelPicker/browser-recording)

**修改:**
- `apps/momo/src/pages/Agent.svelte` — thread 区 → `<ConversationView>`
- `apps/momo/src/components/shell/Composer.svelte` — 接新 controller
- `apps/momo/src/lib/sessionStore.svelte.ts` — `loadSessions` → daemon `list_grouped_sessions` + default cwd 过滤
- `apps/momo/src/lib/auth.svelte.ts`(及 onboarding 回调)— key 改走 daemon `login_with_api_key("openai")` + `update_config`
- `apps/momo/src-tauri/src/backend.rs` — stub 1431 `run_agent_turn` 报错
- `apps/momo/tests/support/fakeDaemon.ts` — chat 事件合成

**废弃(末尾删):** `apps/momo/src/lib/chat.svelte.ts`、`apps/momo/src/components/agent/*`、`apps/momo/src/lib/agentClient.ts`(chat 部分)

**任务依赖:** T1(login)独立 → T2(类型+RPC层) → T3(组件搬运,依赖 T2 类型) → T4(controller,依赖 T2) → T5(落点,依赖 T3/T4) → T6(sidebar,依赖 T2) → T7(后端 stub + 废弃) → T8(测试贯穿,关键断言在最后收口)

---

## Task 1: worldrouter key 接入 daemon(鉴权前提)

**Files:**
- Modify: `apps/momo/src/lib/auth.svelte.ts`(现 `registerKeyWithHost` ~:581-600,调 1431 `login_with_api_key {providerId:"puffer"}`)
- Create: `apps/momo/src/lib/agent/daemonAuth.ts`
- Test: `apps/momo/tests/agent/daemon-auth.spec.ts`

- [ ] **Step 1: 写失败测试** — `daemonAuth.ts` 的 `loginWorldRouter(key)` 应发两条 daemon RPC:`login_with_api_key{providerId:"openai",apiKey:key}` 和 `update_config{openaiBaseUrl, defaultProvider:"openai", defaultModel}`。

`apps/momo/tests/agent/daemon-auth.spec.ts`:
```ts
import { test, expect } from "@playwright/test";
import { FakeDaemon } from "../support/fakeDaemon";

test("loginWorldRouter sends login + config to daemon", async ({ page }) => {
  const daemon = new FakeDaemon();
  const calls: { method: string; params: any }[] = [];
  daemon.onRequest((method, params) => { calls.push({ method, params }); return { ok: true }; });
  await daemon.attach(page); // injects ensureDaemonClient stub that records calls
  await page.goto("/");
  await page.evaluate((k) => (window as any).__loginWorldRouter(k), "sk-worldrouter-TEST");
  expect(calls.find(c => c.method === "login_with_api_key")?.params).toMatchObject({ providerId: "openai", apiKey: "sk-worldrouter-TEST" });
  expect(calls.find(c => c.method === "update_config")?.params).toMatchObject({ openaiBaseUrl: "https://inference-api.worldrouter.ai/v1", defaultProvider: "openai", defaultModel: "gpt-5.4" });
});
```

- [ ] **Step 2: 跑测试看失败** — Run: `cd apps/momo && npx playwright test tests/agent/daemon-auth.spec.ts`。Expected: FAIL(`__loginWorldRouter` undefined / daemonAuth 不存在)。

- [ ] **Step 3: 写实现** — `apps/momo/src/lib/agent/daemonAuth.ts`:
```ts
import { ensureDaemonClient } from "../daemonClient";

export const WORLDROUTER_BASE_URL = "https://inference-api.worldrouter.ai/v1";
export const WORLDROUTER_DEFAULT_MODEL = "gpt-5.4";

/** Register a minted worldrouter key with the daemon as the OpenAI-compatible
 *  provider, and point the openai provider's base_url at worldrouter. */
export async function loginWorldRouter(apiKey: string): Promise<void> {
  const client = await ensureDaemonClient();
  await client.request("login_with_api_key", { providerId: "openai", apiKey });
  await client.request("update_config", {
    openaiBaseUrl: WORLDROUTER_BASE_URL,
    defaultProvider: "openai",
    defaultModel: WORLDROUTER_DEFAULT_MODEL,
  });
}
```

- [ ] **Step 4: 接入 auth 链路** — `apps/momo/src/lib/auth.svelte.ts`:把现有 `registerKeyWithHost`(调 1431 `login_with_api_key {providerId:"puffer"}` 那段,~:581-600)替换为 `import { loginWorldRouter } from "./agent/daemonAuth"` 并调用 `await loginWorldRouter(key)`。登出处(~:803 `logout_provider {providerId:"puffer"}`)改成 daemon `logout_provider {providerId:"openai"}`。测试桥:在该模块导出处加 `if (import.meta.env.DEV) (window as any).__loginWorldRouter = loginWorldRouter;`。

- [ ] **Step 5: 跑测试看通过** — Run: `cd apps/momo && npx playwright test tests/agent/daemon-auth.spec.ts`。Expected: PASS。

- [ ] **Step 6: Commit**
```bash
git add apps/momo/src/lib/agent/daemonAuth.ts apps/momo/src/lib/auth.svelte.ts apps/momo/tests/agent/daemon-auth.spec.ts
git commit -m "feat(momo): route worldrouter key to daemon openai provider + base_url"
```

---

## Task 2: 搬类型 + 瘦 RPC 层 `daemonChat.ts`

**Files:**
- Create: `apps/momo/src/lib/agent/types.ts`, `apps/momo/src/lib/agent/sessionEvents.ts`, `apps/momo/src/lib/agent/daemonChat.ts`
- Source: `apps/puffer-desktop/src/lib/types.ts`, `.../lib/api/sessionEvents.ts`, `.../lib/api/desktop.ts`
- Test: `apps/momo/tests/agent/daemon-chat.spec.ts`

- [ ] **Step 1: 搬类型(无逻辑)** — 复制 desktop `lib/types.ts` 中 chat 相关类型到 `apps/momo/src/lib/agent/types.ts`:`TimelineItem` 联合及其变体(`UserMessage/AssistantMessage/ToolTimelineItem/PermissionTimelineItem/UserQuestionTimelineItem/DiffTimelineItem`)、`SessionDetail`、`SessionListItem`、`AskUserQuestionItem`、`AgentTurnOptions`、`AgentPermissionMode`。删除非 chat 类型(repo/diff-history/browser)。

- [ ] **Step 2: 搬 sessionEvents** — 复制 desktop `lib/api/sessionEvents.ts` 到 `apps/momo/src/lib/agent/sessionEvents.ts`。改 import:`ensureLocalDaemonClient` → momo 的 `ensureDaemonClient`(from `../daemonClient`);`subscribeSessionEvents(sessionId, handler)` 内部用 `client.on(\`session:${sessionId}:event\`, handler)`。保留 `SessionStreamEvent` 全集类型不动。

- [ ] **Step 3: 写失败测试** — 瘦 RPC 层的 `runAgentTurn` 应发 `run_agent_turn{sessionId, message, permissionMode}` 并返回 `turnId`;`createSession` 应发 `create_session{cwd}` 返回 sessionId。

`apps/momo/tests/agent/daemon-chat.spec.ts`:
```ts
import { test, expect } from "@playwright/test";
import { FakeDaemon } from "../support/fakeDaemon";

test("runAgentTurn posts run_agent_turn with workspace-write and returns turnId", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.onRequest((m) => m === "run_agent_turn" ? { turnId: "t-1" } : { ok: true });
  await daemon.attach(page);
  await page.goto("/");
  const out = await page.evaluate(() =>
    (window as any).__daemonChat.runAgentTurn("s-1", "hello"));
  expect(out).toBe("t-1");
  expect(daemon.lastParams("run_agent_turn")).toMatchObject({ sessionId: "s-1", message: "hello", permissionMode: "workspace-write" });
});
```

- [ ] **Step 4: 跑测试看失败** — Run: `cd apps/momo && npx playwright test tests/agent/daemon-chat.spec.ts`。Expected: FAIL。

- [ ] **Step 5: 写瘦 RPC 层** — `apps/momo/src/lib/agent/daemonChat.ts`(自写,不搬 desktop.ts 的 fallback):
```ts
import { ensureDaemonClient } from "../daemonClient";
import type { AgentTurnOptions, SessionDetail, TimelineItem } from "./types";
import { normalizeSessionDetail } from "./normalize";

export async function createSession(cwd: string): Promise<string> {
  const c = await ensureDaemonClient();
  const r = await c.request<{ sessionId: string }>("create_session", { cwd });
  return r.sessionId;
}
export async function runAgentTurn(sessionId: string, message: string, options: AgentTurnOptions = {}): Promise<string> {
  const c = await ensureDaemonClient();
  const r = await c.request<{ turnId: string }>("run_agent_turn", {
    sessionId, message, permissionMode: options.permissionMode ?? "workspace-write",
    ...(options.mode ? { mode: options.mode } : {}),
  });
  return r.turnId;
}
export async function cancelTurn(turnId: string): Promise<void> {
  const c = await ensureDaemonClient(); await c.request("cancel_turn", { turnId });
}
export async function resolvePermission(turnId: string, requestId: string, action: string): Promise<void> {
  const c = await ensureDaemonClient(); await c.request("resolve_permission", { turnId, requestId, action });
}
export async function resolveUserQuestion(turnId: string, requestId: string, answers: Record<string, string|string[]>, annotations: Record<string, Record<string,string>> = {}): Promise<void> {
  const c = await ensureDaemonClient(); await c.request("resolve_user_question", { turnId, requestId, answers, annotations });
}
export async function loadSessionDetail(sessionId: string): Promise<SessionDetail> {
  const c = await ensureDaemonClient();
  const raw = await c.request("load_session_detail", { sessionId });
  return normalizeSessionDetail(raw);
}
export async function listGroupedSessions(): Promise<any[]> {
  const c = await ensureDaemonClient(); return c.request("list_grouped_sessions", {});
}
```
并搬 desktop `desktop.ts` 的 `normalizeTimelineItem`/`normalizeSessionDetail` 到 `apps/momo/src/lib/agent/normalize.ts`(只搬这两个函数 + 其纯辅助,不带 invoke/mock)。测试桥:`if (import.meta.env.DEV) (window as any).__daemonChat = { runAgentTurn, createSession, ... };`(临时,T5 接好后删)。

- [ ] **Step 6: 跑测试看通过** — Run: `cd apps/momo && npx playwright test tests/agent/daemon-chat.spec.ts`。Expected: PASS。

- [ ] **Step 7: Commit**
```bash
git add apps/momo/src/lib/agent/{types.ts,sessionEvents.ts,daemonChat.ts,normalize.ts} apps/momo/tests/agent/daemon-chat.spec.ts
git commit -m "feat(momo): thin daemon chat RPC layer + session event types (ported)"
```

---

## Task 3: 搬 ConversationView 组件树

**Files:**
- Create(复制): `apps/momo/src/lib/agent/ConversationView.svelte` + `apps/momo/src/lib/agent/components/{ToolCard,QuestionPrompt,Approval,DiffCard,MessageBody,HighlightedLine,Icon,Puffer,BrandLogo}.svelte` + `apps/momo/src/lib/agent/codeHighlight.ts` + `apps/momo/src/lib/agent/chat.css`
- Source: 对应 desktop `lib/screens/agent/*` / `lib/components/*` / `lib/design/*` / `lib/codeHighlight.ts`

- [ ] **Step 1: 复制设计资产 + 工具** — 复制 desktop `lib/design/{Icon,Puffer,BrandLogo}.svelte`、`lib/design/chat.css`、`lib/codeHighlight.ts`、`lib/components/{MessageBody,HighlightedLine}.svelte` 到 `apps/momo/src/lib/agent/components/`(及 `codeHighlight.ts`/`chat.css` 到 `lib/agent/`)。调整相对 import 路径到新位置。

- [ ] **Step 2: 复制叶子组件** — 复制 desktop `lib/screens/agent/{Approval,QuestionPrompt,DiffCard}.svelte` 到 `apps/momo/src/lib/agent/components/`。改 import:`../../design/Icon.svelte` → `./Icon.svelte`、`../../types` → `../types`、`../../components/HighlightedLine.svelte` → `./HighlightedLine.svelte`。

- [ ] **Step 3: 复制 ToolCard 并删 browser-recording** — 复制 desktop `lib/screens/agent/ToolCard.svelte`。删除 browser-recording 段(import `ensureLocalDaemonClient`/`browserRecording` 及其调用块,desktop ToolCard ~:917-990)。改 import 同 Step 2。

- [ ] **Step 4: 复制 ConversationView 并删 ModelPicker** — 复制 desktop `lib/screens/agent/ConversationView.svelte`。删除 `ModelPicker` import + 模板中 `<ModelPicker>` 用法 + `settingsSnapshot`/`listProviderModels`/`providerIds` 相关 props 与逻辑(composer 不显示模型切换)。改 import 到 `lib/agent` 内部路径 + `../types`。

- [ ] **Step 5: 编译验证** — Run: `cd apps/momo && npm run check`。Expected: 0 错误(若有未解析 import/类型,逐个修到新位置;不得引入 `lib/screens/agent` 之外的 desktop 文件)。

- [ ] **Step 6: Commit**
```bash
git add apps/momo/src/lib/agent/
git commit -m "feat(momo): port desktop ConversationView component tree (no ModelPicker/browser-recording)"
```

---

## Task 4: 状态机 controller `agentChat.svelte.ts`

**Files:**
- Create: `apps/momo/src/lib/agent/agentChat.svelte.ts`
- Source: 摘自 desktop `App.svelte`(handleSessionEvent/submitMessage/resolve*/cancel/refresh/去重族)
- Test: `apps/momo/tests/agent/agent-chat-reducer.spec.ts`

- [ ] **Step 1: 写失败测试(reducer 核心)** — controller `handleSessionEvent` 处理 `text-delta` 累积到 assistant、`turn-complete` 收尾;replay 的 text-delta 不重复累积。

`apps/momo/tests/agent/agent-chat-reducer.spec.ts`:
```ts
import { test, expect } from "@playwright/test";
test("text-delta accumulates; replay text-delta is ignored", async ({ page }) => {
  await page.goto("/");
  const text = await page.evaluate(() => {
    const c = (window as any).__agentChat.createController("s-1");
    c.bindTurn("turn-1");
    c.handleSessionEvent({ type: "turn-start", turnId: "turn-1" });
    c.handleSessionEvent({ type: "text-delta", turnId: "turn-1", delta: "Hel" });
    c.handleSessionEvent({ type: "text-delta", turnId: "turn-1", delta: "lo" });
    c.handleSessionEvent({ type: "text-delta", turnId: "turn-1", delta: "Hello", replay: true });
    c.handleSessionEvent({ type: "turn-complete", turnId: "turn-1", assistantText: "Hello" });
    return c.combinedTimeline().filter((i:any)=>i.kind==="assistant").map((i:any)=>i.text).join("|");
  });
  expect(text).toBe("Hello"); // 不是 "HelloHello"
});
```

- [ ] **Step 2: 跑测试看失败** — Run: `cd apps/momo && npx playwright test tests/agent/agent-chat-reducer.spec.ts`。Expected: FAIL。

- [ ] **Step 3: 摘状态机** — 新建 `agentChat.svelte.ts`。从 desktop `App.svelte` 摘以下函数,**逻辑原样保留**,但封装成 `createController(sessionId)` 返回的对象(state 存模块级 `Record<sessionId, ChatState>`):
  - state(per session):`liveStreamItems`、`submittedMessages`、`submittedMessageBaselineIds`、`currentTurnId`、`cancelingTurnId`、`turnStartedAtMs`、`turnThinking`、`turnStatusHint`、`settledTurnKeys`、`turnPermissionLookup`、`turnQuestionLookup`、`replayTextByTurn`、`dismissedPermissionIds/QuestionIds`、`sessionDetail`。
  - 函数:`handleSessionEvent`(只搬 switch 部分,**删多会话分支** desktop :3290-3303)、`upsertStreamingAssistant`/`replaySafeDelta`、`appendLive`/`appendAgentError`、`markTurnActive/Settled`/`rememberSettledTurn`/`isTurnSettled`/`turnKey`、live id 生成器(`streamingAssistantId`/`livePermissionId`/`liveQuestionId`/`liveToolId`/`liveItemBelongsToTurn`/`withoutLiveItemsForTurn`)、去重族(`reuseTransientMessageIds`/`transientMessageSignature`/`transientToolSignature`/`transientGateSignature`/`stillMissingFromPersisted`/`wasPersistedBeforeSubmit`/`withCompletionAssistantFallback`)、`refreshSessionAfterTurn`、`resetLiveTurnState`、`safeParseJson`/`normalizeUserQuestions`/`mapPermissionAction`。
  - 派生(用 Svelte `$derived` 或 getter 函数,避免模块顶层裸 `$state` 误用,参考 momo `chat.svelte.ts` 的 keyed-store 写法):`combinedTimeline()` = `[...sessionDetail.timeline, ...renderedSubmittedMessages, ...renderedLiveStreamItems]`、`pendingPermissions()`、`pendingQuestions()`、`turnRunning()`。
  - **隐藏依赖**:`connectionState` 从 `currentDaemonClient()?.onConnectionChange` 派生;**删** `settingsSnapshot`/provider 鉴权/`setLiveSidebarAgentState`/`cacheBackground*`/localStorage 草稿。
  - 动作(调 T2 的 daemonChat):`submitMessage(sessionId, message)`(推 submittedMessage → `runAgentTurn` → 绑 `currentTurnId`)、`resolvePermission`、`resolveUserQuestion`、`cancelCurrentTurn`、`createSessionFromText(text)`(取 default cwd → `createSession` → `submitMessage`)、`appendUserMessage(sessionId, text)`。
  - 订阅:`ensureSubscription(sessionId)` 用 T2 `subscribeSessionEvents`。
  - 测试桥:`if (import.meta.env.DEV) (window as any).__agentChat = { createController };`。

- [ ] **Step 4: 跑测试看通过** — Run: `cd apps/momo && npx playwright test tests/agent/agent-chat-reducer.spec.ts`。Expected: PASS。

- [ ] **Step 5: 加权限/问题去重测试 + 跑通** — 追加 reducer 测试:`permission-request` 进 `pendingPermissions()`,`resolvePermission` 后 dismiss;`tool-calls-requested` 建 running、`tool-invocations` 翻 success(同 callId/turnId)。实现已在 Step 3,补测试断言。Run 同上,Expected: PASS。

- [ ] **Step 6: Commit**
```bash
git add apps/momo/src/lib/agent/agentChat.svelte.ts apps/momo/tests/agent/agent-chat-reducer.spec.ts
git commit -m "feat(momo): chat state-machine controller (keyed by sessionId, ported from desktop)"
```

---

## Task 5: momo 落点(Agent 页 + Composer + 路由)

**Files:**
- Modify: `apps/momo/src/pages/Agent.svelte`, `apps/momo/src/components/shell/Composer.svelte`

- [ ] **Step 1: Agent.svelte 接 ConversationView** — 把 `pages/Agent.svelte` 的 `<section class="agent__thread">`(:132-222)整段替换为:
```svelte
<ConversationView
  timeline={controller.combinedTimeline()}
  pendingPermissions={controller.pendingPermissions()}
  pendingQuestions={controller.pendingQuestions()}
  turnRunning={controller.turnRunning()}
  loading={controller.loading()}
  onSubmitMessage={(m) => controller.appendUserMessage(taskId, m)}
  onResolvePermission={(id, choice) => controller.resolvePermission(id, choice)}
  onResolveUserQuestion={(id, a, an) => controller.resolveUserQuestion(id, a, an)}
  onCancelTurn={() => controller.cancelCurrentTurn(taskId)}
/>
```
顶部 `import ConversationView from "../lib/agent/ConversationView.svelte"; import { createController } from "../lib/agent/agentChat.svelte"; const controller = createController(taskId);` 删除旧 `chat.svelte` import + `ChatBubble/ThinkingBlock/ToolCallPill/AnswerForm` 渲染。`$effect` 里 `controller.ensureSubscription(taskId)` + `loadSessionDetail` hydration。

- [ ] **Step 2: Composer 接 controller** — `components/shell/Composer.svelte`:`import` 从 `chat.svelte` 改为 `../lib/agent/agentChat.svelte`;`handleSubmit`(:74-91)里 `appendUserMessage`/`createSessionFromText` 指向 controller;`running`/`onCancel`(:37/:39)接 `controller.turnRunning(activeId)`/`controller.cancelCurrentTurn(activeId)`。

- [ ] **Step 3: 编译 + 冒烟** — Run: `cd apps/momo && npm run check && npx playwright test tests/chat-smoke.spec.ts`。Expected: check 0 错误;chat-smoke 可能需更新(见 T8),先确认 check 通过。

- [ ] **Step 4: Commit**
```bash
git add apps/momo/src/pages/Agent.svelte apps/momo/src/components/shell/Composer.svelte
git commit -m "feat(momo): wire Agent page + Composer to daemon chat controller"
```

---

## Task 6: sidebar / session 列表迁 daemon

**Files:**
- Modify: `apps/momo/src/lib/sessionStore.svelte.ts`
- Test: `apps/momo/tests/agent/session-list-filter.spec.ts`

- [ ] **Step 1: 写失败测试** — `sessionStore.loadSessions` 应调 daemon `list_grouped_sessions` 并只保留 `folderPath === <default project cwd>` 的会话。

```ts
import { test, expect } from "@playwright/test";
import { FakeDaemon } from "../support/fakeDaemon";
test("session list keeps only default-project cwd, drops monitor sessions", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.onRequest((m) => m === "list_grouped_sessions" ? [
    { folderPath: "/home/u/.momo/projects/default", sessions: [{ sessionId: "chat-1" }] },
    { folderPath: "/home/u", sessions: [{ sessionId: "monitor-1" }] },
  ] : { ok: true });
  await daemon.attach(page, { defaultCwd: "/home/u/.momo/projects/default" });
  await page.goto("/");
  const ids = await page.evaluate(() => (window as any).__sessionStore.list().map((s:any)=>s.sessionId));
  expect(ids).toEqual(["chat-1"]);
});
```

- [ ] **Step 2: 跑测试看失败** — Run: `cd apps/momo && npx playwright test tests/agent/session-list-filter.spec.ts`。Expected: FAIL。

- [ ] **Step 3: 改 loadSessions** — `sessionStore.svelte.ts` 的 `loadSessions`:从 `agent.listGroupedSessions()`(1431) 改为 `import { listGroupedSessions } from "./agent/daemonChat"`;取 default project cwd(经 `projectStore` 的 `getProjectCwd("default")`,该值来自 1431 `list_projects`),`flatMap` 出 `sessions` 后按 `group.folderPath === defaultCwd` 过滤。测试桥 `__sessionStore.list()`。

- [ ] **Step 4: 跑测试看通过** — Run: `cd apps/momo && npx playwright test tests/agent/session-list-filter.spec.ts`。Expected: PASS。

- [ ] **Step 5: Commit**
```bash
git add apps/momo/src/lib/sessionStore.svelte.ts apps/momo/tests/agent/session-list-filter.spec.ts
git commit -m "feat(momo): source sidebar session list from daemon, filter by default cwd"
```

---

## Task 7: 后端 stub + 废弃旧 chat

**Files:**
- Modify: `apps/momo/src-tauri/src/backend.rs`(`run_agent_turn` handler)
- Delete: `apps/momo/src/lib/chat.svelte.ts`, `apps/momo/src/components/agent/*`, `apps/momo/src/lib/agentClient.ts` chat 部分
- Test: `apps/momo/src-tauri/src/backend.rs`(单测)

- [ ] **Step 1: 写失败单测** — 1431 `run_agent_turn` 应直接返回错误(防止任何遗留路径 spawn `puffer non-interactive`)。

`backend.rs` 测试模块:
```rust
#[test]
fn run_agent_turn_is_stubbed_off() {
    let state = BackendState::new_for_test();
    let err = state.handle("run_agent_turn", json!({"sessionId":"s","message":"x"})).unwrap_err();
    assert!(err.to_string().contains("migrated to daemon"));
}
```

- [ ] **Step 2: 跑测试看失败** — Run: `cargo test --manifest-path apps/momo/src-tauri/Cargo.toml run_agent_turn_is_stubbed_off`。Expected: FAIL。

- [ ] **Step 3: stub handler** — `backend.rs` 的 `"run_agent_turn" =>` 分支改为 `return Err(anyhow!("run_agent_turn migrated to daemon; frontend should call the daemon directly"));`。保留 `run_agent_turn_inner`/`codex_app_server` 代码(死代码,阶段 2 删)但不再被 dispatch。

- [ ] **Step 4: 跑测试看通过** — Run: `cargo test --manifest-path apps/momo/src-tauri/Cargo.toml run_agent_turn_is_stubbed_off`。Expected: PASS。

- [ ] **Step 5: 删旧前端 chat** — 删 `apps/momo/src/lib/chat.svelte.ts`、`apps/momo/src/components/agent/{ChatBubble,ThinkingBlock,ToolCallPill,AnswerForm,ToolBlock,AgentText,OptionsCard,ResultCard}.svelte`、`agentClient.ts` 的 chat 函数(`runAgentTurn/cancelTurn/resolveUserQuestion/createSession/loadSessionDetail/subscribeSessionEvents`,保留 connectors/auth 用的)。Run `cd apps/momo && npm run check` 修残留 import。

- [ ] **Step 6: Commit**
```bash
git add -A apps/momo
git commit -m "refactor(momo): stub 1431 run_agent_turn, remove legacy chat frontend"
```

---

## Task 8: 测试基建 + 端到端收口

**Files:**
- Modify: `apps/momo/tests/support/fakeDaemon.ts`
- Modify: `apps/momo/tests/chat-smoke.spec.ts`, `apps/momo/tests/chat/stop-button.spec.ts`

- [ ] **Step 1: 扩 fakeDaemon 加 chat 事件合成** — `tests/support/fakeDaemon.ts` 增 `emitSessionEvent(sessionId, payload)`,以 daemon **真实帧** `{ event: \`session:${sessionId}:event\`, payload }` 经 ws 推送;增 `attach(page, opts)` 注入 `ensureDaemonClient` 替身(记录 request、转发 emit 到 `on` 监听)。帧格式严格 `{id,result}`/`{event,payload}`(非 `{type,ok}`)。

- [ ] **Step 2: 写 e2e — 发消息渲染** — `chat-smoke.spec.ts` 更新为:输入框发 "hi" → fakeDaemon 回 `run_agent_turn{turnId}` → `emitSessionEvent` 发 `text-delta`/`turn-complete` → 断言 ConversationView 出现助手文本。Run: `cd apps/momo && npx playwright test tests/chat-smoke.spec.ts`。Expected: PASS。

- [ ] **Step 3: 写 e2e — permission/question/cancel** — 新增 `tests/chat/permission.spec.ts`、`tests/chat/question.spec.ts`:emit `permission-request` → Approval 出现 → 点允许 → 断言发 `resolve_permission{action:"allow_once"}`;emit `user-question-request` → QuestionPrompt → 答 → 断言 `resolve_user_question`;`stop-button.spec.ts` 更新为断言发 `cancel_turn`。Run: `cd apps/momo && npx playwright test tests/chat/`。Expected: PASS。

- [ ] **Step 4: 全回归** — Run: `cd apps/momo && npm run check && npm run test:desktop-ui && cargo test --manifest-path src-tauri/Cargo.toml && cargo check --manifest-path src-tauri/Cargo.toml`。Expected: 全 PASS。

- [ ] **Step 5: 手动验证 worldrouter wire(需真 key)** — 登录 → 观察 daemon `login_with_api_key`+`update_config` 发出 → 发消息 → 确认从 worldrouter 收到回复(验证 spec §4 wire)。若 daemon 出 `turn-error`(wire 不匹配),按 spec §11 fallback 调整 provider 形态。

- [ ] **Step 6: Commit**
```bash
git add apps/momo/tests
git commit -m "test(momo): daemon chat e2e (message/tool/permission/question/cancel) + fakeDaemon chat events"
```

---

## Self-Review 检查项(执行者开工前确认)
- T1–T8 覆盖 spec §4(login)/§5a(RPC层)/§5b(controller)/§5c(组件)/§5d(落点)/§5e(sidebar)/§6(后端stub)/§9(测试)。spec §5f(cwd 时序)在 T4 `createSessionFromText` + T6 default cwd 取值中落实。
- 类型一致:`TimelineItem`(T2 定义)贯穿 T3 组件 / T4 controller / T5 落点;`runAgentTurn` 返回 `string`(turnId)在 T2/T4 一致。
- 无 placeholder:搬运步骤给出精确源/目标/改动 import;新写逻辑给出完整代码。
