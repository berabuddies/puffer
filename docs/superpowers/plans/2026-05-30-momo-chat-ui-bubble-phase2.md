# Momo Chat UI 气泡化(阶段 2)Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 momo chat 的渲染层从 desktop IDE 风格的 `ConversationView` 换成 momo 气泡 UI,渲染同一份 `TimelineItem[]`,只动渲染层。

**Architecture:** 纯气泡时序流(按 `combinedTimeline()` 数组顺序逐项渲染,无 roll-up)。核心杠杆 = 在新气泡根容器 `.momo-chat` 上做一层 **desktop→momo token 桥接**(`--background`/`--foreground`/`--muted`/`--border`/`--puffer-accent`/… 现在 momo 里**根本未定义**,组件靠 fallback 在跑)——桥接后复用的 `MessageBody`/`ToolCard`/`DiffCard`/`Approval`/`QuestionPrompt` 立刻拿到 momo 配色,无需改其内部;再在 `.momo-chat` 作用域叠 momo 气泡/pill/卡片皮。restyle 一律**只改 CSS、保留 class/data-attr/结构**,使现有 e2e 基本保持绿。

**Tech Stack:** Svelte 5(runes),Vite,Playwright(`test:desktop-ui`,fakeDaemon e2e),vitest(reducer 单测)。

**Spec:** `docs/superpowers/specs/2026-05-30-momo-chat-ui-bubble-phase2-design.md`(已批准 + 经 reviewer 修正)。
**Worktree/分支:** `.claude/worktrees/feat-momo-chat-bubble-phase2` @ `feat/momo-chat-bubble-phase2`(已建)。**所有 git 用 `git -C <worktree>`,不要 cd。** 下文 `$WT` = `/Users/shun/Data/Code/tomo/agentenv/puffer/.claude/worktrees/feat-momo-chat-bubble-phase2`,组件根目录 `$WT/apps/momo`。

---

## 关键事实(实现前必读,已核验)

1. **desktop token 未定义**:`--background`/`--foreground`/`--muted`/`--muted-foreground`/`--border`/`--accent`/`--accent-foreground`/`--puffer-accent`/`--puffer-accent-fg`/`--puffer-row-gap`/`--font-mono`/`--shadow-sm`/`--shadow-xs` 定义在 `apps/puffer-desktop/src/app.css`,**momo 没有**。momo 的 `tokens.css` 有 `--color-*`/`--font-serif`/`--font-system`/`--font-sans`(Inter)/`--space-*`/`--radius-*`。→ Task 1 的 token 桥接负责补上。
2. **渲染来源**:`Agent.svelte:129-144` 把 controller getter 喂给渲染组件;`onSubmitMessage` 是 inert 死代码(输入走 shell `Composer`)。`combinedTimeline()` = `[...persisted, ...submitted, ...live]`(数组顺序即 Bug2-safe 顺序,**view 层不得按 `createdAtMs` 重排**)。permission/question item **在 combinedTimeline 里**(按位置),`pendingPermissions()`/`pendingQuestions()` 是其 pending 子集;resolve 只 dismiss、不改 item.status(`agentChat.svelte.ts:1215-1234`)。
3. **e2e 断言的真实选择器**(`tests/agent/chat-interactions.e2e.spec.ts`):`.pf-approval`(含文案 "Approval needed" + toolName + reason + 按钮 "Approve once"/"Always allow"/"Deny")、`.pf-question` + `.pf-question-option` + 按钮 "Send answer"、`.pf-tool` + `.pf-tool-status[data-state="running"|"done"]`(含 raw toolId 文本如 "read_file")、shell Stop 按钮 `aria-label="Stop"`。**restyle 必须保留这些 class/data-attr/结构**;凡改可见文案(如英→中)的,同步改对应断言(本计划已点名)。
4. **折叠态**:`DiffCard`/`ToolCard` 的 `.pf-tool` 带 `data-collapsed`(见 `DiffCard.svelte:62`)→ pill(折叠)↔ 统一卡片(展开)可纯 CSS 靠 `[data-collapsed]` 切。
5. **MessageBody 禁改**(rAF 节流 + key + LRU memo,`MessageBody.svelte:343-421`)。只作为子组件挂,不动其内部。

---

## 文件结构

**新建**(`$WT/apps/momo/src/lib/agent/`):
- `BubbleConversation.svelte` — 顶层渲染器(token 桥接 `.momo-chat` 根 + 滚动容器 + 时序渲染开关 + 底部 typing 指示器)。替换 `ConversationView`。
- `bubble.css` — `.momo-chat` 作用域的 token 桥接 + 气泡/pill/卡片皮(集中放,避免散在各组件 `<style>`)。
- `components/ChatBubble.svelte` — user/assistant/system/command 消息气泡(markdown 走 `MessageBody`)。
- `toolLabels.ts` — 中文友好 label + `lookupToolLabel`。
- `debugFlags.ts` — `SHOW_RAW_AGENT_ACTIVITY`。

**修改**:
- `pages/Agent.svelte` — 换挂 `BubbleConversation`;去掉 `:global(.pf-composer-wrap)` 隐藏规则(新组件无内置 composer);`.agent__thread` 布局微调。
- `components/ToolCard.svelte` — head 用 `lookupToolLabel`(prod 友好/dev raw);保留 body 渲染器 + 折叠逻辑 + `.pf-tool*` class + `data-collapsed` + `data-state`。
- `components/Approval.svelte` / `QuestionPrompt.svelte` / `DiffCard.svelte` — 仅必要的 class 钩子/文案,主要靠 `bubble.css` reskin。
- `tests/agent/chat-interactions.e2e.spec.ts` — 同步改被改动的可见文案断言;新增 reload-answered + in-place 顺序用例。

**删除**(全绿后):
- `ConversationView.svelte`(momo 内唯一消费者是 Agent.svelte,已核验)。**保留 `chat.css`**(ToolCard/DiffCard/MessageBody 仍用其 `.pf-*` 类与 `--pf-chat-*-size`)。

---

## Task 1: 气泡根 + token 桥接 + 挂载(替换 ConversationView)

**Files:**
- Create: `$WT/apps/momo/src/lib/agent/BubbleConversation.svelte`
- Create: `$WT/apps/momo/src/lib/agent/bubble.css`
- Modify: `$WT/apps/momo/src/pages/Agent.svelte:24,128-145,205-210`
- Test: `$WT/apps/momo/tests/agent/chat-interactions.e2e.spec.ts`(现有 smoke 路径)

- [ ] **Step 1: 先确认测试端口避开主仓 1466**

Run: `grep -rn "1466\|port" $WT/apps/momo/playwright.config.ts $WT/apps/momo/vite.config.ts 2>/dev/null`
若 webServer/dev port = 1466,在 `playwright.config.ts` 的 `webServer.port` 与 `use.baseURL` 改成 **1477**(merge 前 revert,见 Task 9)。若已是其它端口且不与主仓冲突,跳过。

- [ ] **Step 2: 跑现有 e2e 确认基线全绿(改之前)**

Run: `cd $WT/apps/momo && npm run test:desktop-ui -- chat-interactions`
Expected: 5 个 case 全 PASS(permission / question / cancel / tool running→done)。记下基线。

- [ ] **Step 3: 写 `bubble.css` 的 token 桥接 + 容器骨架**

Create `$WT/apps/momo/src/lib/agent/bubble.css`:

```css
/* momo 气泡 chat —— desktop→momo token 桥接 + 气泡/pill/卡片皮。
   桥接:ToolCard/DiffCard/MessageBody/Approval/QuestionPrompt 用的是 desktop
   调色板别名(--muted/--foreground/--puffer-accent/…),momo 未定义。在 .momo-chat
   根映射到 momo tokens.css,使复用组件无需改内部即可拿到 momo 配色。 */
.momo-chat {
  --background: var(--color-surface-app);
  --foreground: var(--color-text-primary);
  --muted: var(--color-surface-rail);
  --muted-foreground: var(--color-text-muted);
  --border: var(--color-input-border);
  --accent: var(--color-action-cream);
  --accent-foreground: var(--color-action-cream-text);
  --puffer-accent: var(--color-action-cream-text);
  --puffer-accent-fg: #ffffff;
  --puffer-accent-soft: var(--color-action-cream);
  --puffer-row-gap: var(--space-3);
  --font-mono: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  --shadow-xs: 0 1px 1px rgba(0, 0, 0, 0.03);
  --shadow-sm: 0 1px 2px rgba(0, 0, 0, 0.05);

  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  background: var(--color-surface-app);
}
.momo-chat__thread {
  flex: 1;
  overflow-y: auto;
  padding: 36px 0 24px;
}
.momo-chat__inner {
  max-width: var(--shell-page-max);   /* 760 */
  margin: 0 auto;
  width: 100%;
  padding: 0 var(--shell-page-padding); /* 24 */
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
}
.momo-chat__loading,
.momo-chat__typing {
  color: var(--color-text-muted);
  font-family: var(--font-system);
  font-size: var(--font-size-body);
  padding: var(--space-2) 0;
}
```

- [ ] **Step 4: 写 `BubbleConversation.svelte`(骨架:桥接根 + 滚动 + 逐项渲染占位 + typing)**

Create `$WT/apps/momo/src/lib/agent/BubbleConversation.svelte`:

```svelte
<script lang="ts">
  import "./chat.css";   // 复用 .pf-* 类 + --pf-chat-*-size(ToolCard/DiffCard/MessageBody 依赖)
  import "./bubble.css"; // momo 桥接 + 气泡皮
  import MessageBody from "./components/MessageBody.svelte";
  import ToolCard from "./components/ToolCard.svelte";
  import DiffCard from "./components/DiffCard.svelte";
  import Approval from "./components/Approval.svelte";
  import QuestionPrompt from "./components/QuestionPrompt.svelte";
  import ChatBubble from "./components/ChatBubble.svelte";
  import type {
    TimelineItem, PermissionTimelineItem, UserQuestionTimelineItem,
    ToolTimelineItem, DiffTimelineItem, MessageTimelineItem
  } from "./types";
  import type { SessionListItem } from "./types";

  type Props = {
    session: SessionListItem | null;
    timeline: TimelineItem[];
    pendingPermissions: PermissionTimelineItem[];
    pendingQuestions: UserQuestionTimelineItem[];
    loading: boolean;
    turnRunning?: boolean;
    turnStartedAtMs?: number | null;
    turnThinking?: boolean;
    turnStatusHint?: string | null;
    onResolvePermission: (permissionId: string, choice: string) => void;
    onResolveUserQuestion: (
      questionId: string,
      answers: Record<string, string | string[]>,
      annotations?: Record<string, Record<string, string>>
    ) => void;
    onCancelTurn?: () => void;
  };
  let {
    session, timeline, pendingPermissions, pendingQuestions, loading,
    turnRunning = false, turnStartedAtMs = null, turnThinking = false,
    turnStatusHint = null, onResolvePermission, onResolveUserQuestion
  }: Props = $props();

  let pendingPermIds = $derived(new Set(pendingPermissions.map((p) => p.id)));
  let pendingQIds = $derived(new Set(pendingQuestions.map((q) => q.id)));

  // 渲染规则(render-layer only,保 e2e 绿):
  //  permission: 仅当仍 pending 才渲染可操作卡;已 dismiss(live resolve)/非 pending → 跳过。
  //  question:   pending → 可操作;status==="answered"(持久化历史)→ 折叠回显;否则跳过。
  function showItem(item: TimelineItem): boolean {
    if (item.kind === "permission") return pendingPermIds.has(item.id);
    if (item.kind === "question")
      return pendingQIds.has(item.id) || item.status === "answered";
    return true;
  }
</script>

<div class="momo-chat">
  <div class="momo-chat__thread">
    <div class="momo-chat__inner">
      {#if loading && timeline.length === 0}
        <div class="momo-chat__loading">Loading…</div>
      {/if}

      {#each timeline as item (item.id)}
        {#if showItem(item)}
          {#if item.kind === "tool"}
            <ToolCard item={item as ToolTimelineItem} />
          {:else if item.kind === "diff"}
            <DiffCard item={item as DiffTimelineItem} />
          {:else if item.kind === "permission"}
            <Approval
              item={item as PermissionTimelineItem}
              onResolve={onResolvePermission}
            />
          {:else if item.kind === "question"}
            <QuestionPrompt
              item={item as UserQuestionTimelineItem}
              disabled={(item as UserQuestionTimelineItem).status === "answered"}
              onResolve={onResolveUserQuestion}
            />
          {:else}
            <ChatBubble item={item as MessageTimelineItem} {MessageBody} />
          {/if}
        {/if}
      {/each}

      {#if turnRunning}
        <div class="momo-chat__typing" aria-live="polite">
          {turnStatusHint ?? (turnThinking ? "思考中…" : "处理中…")}
        </div>
      {/if}
    </div>
  </div>
</div>
```

> 注:`ChatBubble` 这里把 `MessageBody` 作为 prop 传入是为简化首版;Task 2 会让 `ChatBubble` 直接 import `MessageBody`,此处 prop 删掉。先按 Task 2 的最终签名写也可——见 Task 2。

- [ ] **Step 5: 在 Agent.svelte 换挂 BubbleConversation**

Modify `$WT/apps/momo/src/pages/Agent.svelte`:
- 第 24 行 `import ConversationView ...` → `import BubbleConversation from "../lib/agent/BubbleConversation.svelte";`
- 第 129-144 的 `<ConversationView ... />` 整块 → 删掉 `onSubmitMessage` 行,组件名换成 `BubbleConversation`,其余 props/回调不变:

```svelte
      <BubbleConversation
        {session}
        {timeline}
        {pendingPermissions}
        {pendingQuestions}
        {turnRunning}
        {turnStartedAtMs}
        {turnThinking}
        {turnStatusHint}
        {loading}
        onResolvePermission={(id, choice) => controller?.resolvePermission(id, choice)}
        onResolveUserQuestion={(id, answers, annotations) =>
          controller?.resolveUserQuestion(id, answers, annotations)}
        onCancelTurn={() => controller?.cancelCurrentTurn()}
      />
```
- 删第 205-210 的 `.agent__thread :global(.pf-composer-wrap){display:none}`(新组件无内置 composer)。

- [ ] **Step 6: 跑 check + e2e**

Run: `cd $WT/apps/momo && npm run check && npm run test:desktop-ui -- chat-interactions`
Expected: `check` 0 error;e2e —— `tool running→done`、`cancel` 应 PASS;`.pf-approval`/`.pf-question` 仍渲染(后续 task restyle 不改 class)。若 permission/question case 因"in-place vs 注入末尾"出现可见性差异,确认卡片仍在 DOM(本 task 不改其样式)。

- [ ] **Step 7: Commit**

```bash
git -C $WT add apps/momo/src/lib/agent/BubbleConversation.svelte apps/momo/src/lib/agent/bubble.css apps/momo/src/pages/Agent.svelte apps/momo/playwright.config.ts
git -C $WT commit -m "feat(momo): mount BubbleConversation with desktop->momo token bridge"
```

---

## Task 2: ChatBubble(user/assistant/system/command 气泡)

**Files:**
- Create: `$WT/apps/momo/src/lib/agent/components/ChatBubble.svelte`
- Modify: `$WT/apps/momo/src/lib/agent/BubbleConversation.svelte`(去掉 MessageBody prop 传递)
- Append: `$WT/apps/momo/src/lib/agent/bubble.css`(气泡样式)

- [ ] **Step 1: 写 ChatBubble.svelte**

```svelte
<script lang="ts">
  import MessageBody from "./MessageBody.svelte";
  import type { MessageTimelineItem } from "../types";
  let { item }: { item: MessageTimelineItem } = $props();
  let role = $derived(item.kind); // user | assistant | system | command
  let isError = $derived(item.status === "error");
</script>

<div class="mb-row" data-role={role} data-error={isError}>
  {#if role === "command"}
    <div class="mb-cmd">{item.body || item.title}</div>
  {:else if role === "system"}
    <div class="mb-system">{item.body || item.summary || item.title}</div>
  {:else}
    <!-- user / assistant 气泡;markdown 走 MessageBody(含抗抖 fix) -->
    <div class="mb-bubble">
      <div class="pf-msg-text"><MessageBody body={item.body} /></div>
    </div>
  {/if}
</div>
```

> `.pf-msg-text` 包裹是为继承 `chat.css` 里 `.message-body` 的排版规则(`chat.css:64-98`)。

- [ ] **Step 2: 在 bubble.css 追加气泡样式(精确 token,来自设计图)**

Append to `$WT/apps/momo/src/lib/agent/bubble.css`:

```css
.mb-row { display: flex; }
.mb-row[data-role="user"] { justify-content: flex-end; }
.mb-row[data-role="assistant"] { justify-content: flex-start; }
.mb-bubble {
  max-width: 80%;
  padding: 11px 15px;
  font-family: var(--font-system);
  font-size: 14px;
  line-height: 20px;
  color: var(--color-text-primary);
}
.mb-row[data-role="user"] .mb-bubble {
  background: var(--color-surface-rail);          /* #f4f4f4 */
  border-radius: 16px 4px 16px 16px;
}
.mb-row[data-role="assistant"] .mb-bubble {
  background: var(--color-surface-rail);
  border-radius: 4px 14px 14px 14px;
}
.mb-system {
  color: var(--color-text-muted);
  font-family: var(--font-system);
  font-size: var(--font-size-body);
  line-height: var(--line-height-body);
}
.mb-row[data-error="true"] .mb-system { color: #b3261e; }
.mb-cmd {
  display: inline-flex;
  padding: 4px 12px;
  border-radius: var(--radius-pill);
  background: var(--color-surface-rail);
  color: var(--color-text-secondary);
  font-family: var(--font-mono);
  font-size: 12px;
}
```

- [ ] **Step 3: 改 BubbleConversation 的 ChatBubble 调用**

把 Task 1 Step 4 里 `<ChatBubble item={...} {MessageBody} />` 改为 `<ChatBubble item={item as MessageTimelineItem} />`,并删除 BubbleConversation 中对 `MessageBody` 的 import(已下沉到 ChatBubble)。

- [ ] **Step 4: e2e — 发消息渲染用户/助手气泡**

Append test to `$WT/apps/momo/tests/agent/chat-interactions.e2e.spec.ts`:

```ts
test("user + assistant text render as left/right bubbles", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);
  const { sessionId, turnId } = await startTurnFromHome(page, daemon, "Say hi");
  emitTurnStart(daemon, { sessionId, turnId });
  daemon.emit(`session:${sessionId}:event`, { type: "text-delta", turnId, delta: "Hello there" });

  await expect(page.locator('.mb-row[data-role="user"] .mb-bubble')).toContainText("Say hi");
  await expect(page.locator('.mb-row[data-role="assistant"] .mb-bubble')).toContainText("Hello there");
});
```

Run: `cd $WT/apps/momo && npm run test:desktop-ui -- chat-interactions -g "left/right bubbles"`
Expected: 先 FAIL(组件未接前),实现后 PASS。

> 若 `text-delta` 事件字段名与此不符,先 `grep -n "text-delta" $WT/apps/momo/src/lib/agent/sessionEvents.ts` 确认 payload 再调整。

- [ ] **Step 5: check + 全量 e2e**

Run: `cd $WT/apps/momo && npm run check && npm run test:desktop-ui -- chat-interactions`
Expected: 全 PASS。

- [ ] **Step 6: Commit**

```bash
git -C $WT add apps/momo/src/lib/agent/components/ChatBubble.svelte apps/momo/src/lib/agent/bubble.css apps/momo/src/lib/agent/BubbleConversation.svelte apps/momo/tests/agent/chat-interactions.e2e.spec.ts
git -C $WT commit -m "feat(momo): ChatBubble for user/assistant/system/command messages"
```

---

## Task 3: ToolCard → 友好 pill(toolLabels)+ 折叠/展开卡片

**Files:**
- Create: `$WT/apps/momo/src/lib/agent/toolLabels.ts`
- Create: `$WT/apps/momo/src/lib/agent/debugFlags.ts`
- Modify: `$WT/apps/momo/src/lib/agent/components/ToolCard.svelte`(head 标签来源)
- Append: `$WT/apps/momo/src/lib/agent/bubble.css`(pill/卡片皮,靠 `[data-collapsed]`)

- [ ] **Step 1: 写 debugFlags.ts**

```ts
/** dev 显 raw toolId/原始 args;prod 用 toolLabels 友好文案。
 *  Vite 在 build 时把 import.meta.env.DEV tree-shake 成 false。 */
export const SHOW_RAW_AGENT_ACTIVITY = import.meta.env.DEV;
```

- [ ] **Step 2: 写 toolLabels.ts(中文)**

```ts
import type { IconName } from "./components/Icon.svelte";

export interface ToolLabel { icon: IconName; label: string; }

const TOOL_LABELS: Record<string, ToolLabel> = {
  read_file: { icon: "file", label: "正在读取文件…" },
  write_file: { icon: "edit", label: "正在写入文件…" },
  edit: { icon: "edit", label: "正在编辑文件…" },
  apply_patch: { icon: "edit", label: "正在修改文件…" },
  bash: { icon: "terminal", label: "正在执行命令…" },
  shell: { icon: "terminal", label: "正在执行命令…" },
  grep: { icon: "search", label: "正在搜索…" },
  glob: { icon: "search", label: "正在查找文件…" },
  skill: { icon: "sparkles", label: "正在调用技能…" },
  websearch: { icon: "globe", label: "正在联网搜索…" },
  webfetch: { icon: "globe", label: "正在抓取网页…" }
};

/** 嗅探 bash 命令前缀,把通用 shell 伪装成高层动作(沿用旧 momo)。 */
export function lookupToolLabel(toolId: string, input?: unknown): ToolLabel | null {
  const id = toolId.toLowerCase();
  if ((id === "bash" || id === "shell") && typeof input === "object" && input) {
    const cmd = String((input as Record<string, unknown>).command ?? "");
    if (cmd.startsWith("telegram ")) return { icon: "message-circle", label: "正在使用 Telegram…" };
    if (cmd.startsWith("email ")) return { icon: "mail", label: "正在处理邮件…" };
  }
  return TOOL_LABELS[id] ?? null;
}
```

> 先 `grep -n "export" $WT/apps/momo/src/lib/agent/components/Icon.svelte | head` 确认 `IconName` 联合里有用到的图标名(`file`/`edit`/`terminal`/`search`/`sparkles`/`globe`/`message-circle`/`mail`);缺的换成已有名或在 Icon.svelte 的 `iconMap` 补。

- [ ] **Step 3: 改 ToolCard head 用 friendly label(prod)/raw(dev)**

读 `$WT/apps/momo/src/lib/agent/components/ToolCard.svelte`,定位 head 模板里渲染 `.pf-tool-name` / `.pf-tool-icon` 的位置(约 `:831-850`)与 `item.toolName`。在 `<script>` 顶部加:

```ts
import { lookupToolLabel } from "../toolLabels";
import { SHOW_RAW_AGENT_ACTIVITY } from "../debugFlags";
// ... 既有 props ...
let friendly = $derived(lookupToolLabel(item.toolName, item.inputJson));
let headName = $derived(SHOW_RAW_AGENT_ACTIVITY ? item.toolName : (friendly?.label ?? "正在处理…"));
let headIcon = $derived(friendly?.icon ?? null);
```
把 `.pf-tool-name` 的文本从 `item.toolName` 改为 `{headName}`;`.pf-tool-icon` 的 `<Icon name=...>` 用 `{headIcon ?? <既有默认>}`。**dev 下仍显 raw toolId**(`read_file`),故 e2e line 216 的 `toContainText("read_file")` 在 dev 测试构建下保持绿;**保留 `.pf-tool-status[data-state]` 与 `.pf-tool-arg` 不动**。

- [ ] **Step 4: bubble.css 追加 ToolCard pill/卡片皮(靠 data-collapsed)**

```css
/* 复用 ToolCard 的 .pf-tool;在 momo 作用域重做皮:折叠=pill,展开=统一卡片 */
.momo-chat :global(.pf-tool) {
  border: 1px solid var(--color-input-border);
  border-radius: 12px;
  background: var(--color-surface-app);
  box-shadow: none;
  width: fit-content;
  max-width: 560px;
}
.momo-chat :global(.pf-tool[data-collapsed="false"]) { width: auto; }
.momo-chat :global(.pf-tool-head) {
  background: transparent;
  border-bottom: 0;
  min-height: 36px;
  padding: 8px 14px;
  color: var(--color-text-secondary);
  font-family: var(--font-system);
  font-size: 13px;
}
.momo-chat :global(.pf-tool[data-collapsed="false"] .pf-tool-head) {
  border-bottom: 1px solid var(--color-card-border);
}
.momo-chat :global(.pf-tool-icon) {
  background: transparent; color: var(--color-text-secondary);
  width: 16px; height: 16px;
}
.momo-chat :global(.pf-tool-body) { background: var(--color-surface-app); }
```

> `:global()` 因为 `.pf-tool*` 是 ToolCard/chat.css 的类,需穿透 Svelte scope。整体限定在 `.momo-chat` 下,不污染别处。

- [ ] **Step 5: e2e — pill 折叠 + 点开统一卡片**

Run: `cd $WT/apps/momo && npm run test:desktop-ui -- chat-interactions -g "tool-calls-requested"`
Expected: 现有 `tool running→success` case 仍 PASS(class/data-state 未变)。新增断言折叠态:

```ts
// 追加到 tool 用例尾部:默认折叠 => data-collapsed="true";点 head 展开
await expect(tool).toHaveAttribute("data-collapsed", "true");
await tool.locator(".pf-tool-head").click();
await expect(tool).toHaveAttribute("data-collapsed", "false");
```
（若 ToolCard 默认 `defaultCollapsed` 非 true 或运行中自动展开,按实际调整断言;运行中默认折叠见 spec §7.2。）

- [ ] **Step 6: check + 全量 e2e + commit**

```bash
cd $WT/apps/momo && npm run check && npm run test:desktop-ui -- chat-interactions
git -C $WT add apps/momo/src/lib/agent/toolLabels.ts apps/momo/src/lib/agent/debugFlags.ts apps/momo/src/lib/agent/components/ToolCard.svelte apps/momo/src/lib/agent/bubble.css apps/momo/tests/agent/chat-interactions.e2e.spec.ts
git -C $WT commit -m "feat(momo): friendly tool pill (toolLabels) + collapse/expand card"
```

---

## Task 4: DiffCard restyle(卡片皮)

**Files:**
- Append: `$WT/apps/momo/src/lib/agent/bubble.css`
- (DiffCard.svelte 结构/`.pf-tool` class 不动,纯 CSS reskin;它复用 Task 3 的 `.pf-tool` 皮即可)

- [ ] **Step 1: 确认 DiffCard 复用 .pf-tool 皮已生效**

DiffCard 用 `.pf-tool`/`.pf-tool-head`/`.pf-tool-body` + `data-collapsed`(见 `DiffCard.svelte:62-97`),已被 Task 3 Step 4 的 `.momo-chat :global(.pf-tool*)` 覆盖 → 自动获得 momo 卡片皮。

- [ ] **Step 2: 追加 diff 行的绿/红底(覆盖 .pf-diff 行)**

```css
.momo-chat :global(.pf-diff .row.add) { background: #e9f5ea; color: #1e7a34; }
.momo-chat :global(.pf-diff .row.del) { background: #fdecec; color: #b3261e; }
.momo-chat :global(.pf-diff .row.ctx) { color: var(--color-text-secondary); }
.momo-chat :global(.pf-diff .gutter) { color: var(--color-text-muted); }
```

> 先 `grep -n "pf-diff\|\.row\|gutter\|hunk-hdr" $WT/apps/momo/src/lib/agent/ConversationView.svelte $WT/apps/momo/src/lib/agent/chat.css` 确认 `.pf-diff .row.add/.del/.ctx` 的实际类名(DiffCard.svelte:88 用 `row {r.k}`,`r.k` ∈ add/del/ctx)。

- [ ] **Step 3: 视觉验证 + check + commit**

Run: `cd $WT/apps/momo && npm run check`
（diff 无现成 e2e;用 `/run` skill 或手动发一个含 edit 的 turn 截图核验绿/红底 + 卡片圆角。）
```bash
git -C $WT add apps/momo/src/lib/agent/bubble.css
git -C $WT commit -m "style(momo): diff card bubble skin (add/del tint)"
```

---

## Task 5: Approval → 审批卡(中文 + cream/neutral/deny 按钮)

**Files:**
- Modify: `$WT/apps/momo/src/lib/agent/components/Approval.svelte`(文案中文化 + 按钮 class 钩子)
- Append: `$WT/apps/momo/src/lib/agent/bubble.css`
- Modify: `$WT/apps/momo/tests/agent/chat-interactions.e2e.spec.ts:83`("Approval needed" 断言)

- [ ] **Step 1: Approval.svelte 文案中文 + variant 映射到 momo 按钮**

把 `Approval.svelte:24` 的 `Approval needed` 改为 `需要授权`;按钮保留 `.sc-btn data-variant data-size` **不删**(若 `.sc-btn` 全局样式在 momo 不存在则改用下方 `.mb-approval-btn`)。先 `grep -rn "sc-btn" $WT/apps/momo/src` 确认 `.sc-btn` 是否有 momo 样式;**若无**,把按钮 class 换成 `mb-approval-btn` 并加 `data-variant`,样式见 Step 2。`variantFor` 逻辑保留(deny→ghost、always/session→outline、其余→default)。

- [ ] **Step 2: bubble.css 追加审批卡 + 按钮皮**

```css
.momo-chat :global(.pf-approval) {
  background: var(--color-surface-app);
  border: 1px solid var(--color-input-border);
  border-radius: 4px 16px 16px 16px;
  max-width: 540px;
  padding: 13px 15px;
}
.momo-chat :global(.mb-approval-btn),
.momo-chat :global(.pf-approval .sc-btn) {
  height: 32px; border-radius: var(--radius-pill); padding: 0 16px;
  font-family: var(--font-sans); font-size: 12px; font-weight: 500;
  border: 1px solid transparent; cursor: pointer;
}
.momo-chat :global([data-variant="default"]) {
  background: var(--color-action-cream);
  color: var(--color-action-cream-text);
  border-color: var(--color-action-cream-border);
}
.momo-chat :global([data-variant="outline"]) {
  background: var(--color-surface-rail); color: var(--color-text-primary);
}
.momo-chat :global([data-variant="ghost"]) {
  background: var(--color-surface-app); color: #b3261e;
  border-color: var(--color-card-border);
}
```

- [ ] **Step 3: 改 e2e 文案断言**

`chat-interactions.e2e.spec.ts:83` `await expect(approval).toContainText("Approval needed");` → `await expect(approval).toContainText("需要授权");`。按钮名断言("Approve once" 等)是 `item.choices` 原文(reducer 产出英文)→ **不改**。

- [ ] **Step 4: e2e + check + commit**

```bash
cd $WT/apps/momo && npm run check && npm run test:desktop-ui -- chat-interactions -g "permission-request"
git -C $WT add apps/momo/src/lib/agent/components/Approval.svelte apps/momo/src/lib/agent/bubble.css apps/momo/tests/agent/chat-interactions.e2e.spec.ts
git -C $WT commit -m "feat(momo): approval card skin + zh label"
```

---

## Task 6: QuestionPrompt → 选项卡 + answered 历史回显

**Files:**
- Modify: `$WT/apps/momo/src/lib/agent/components/QuestionPrompt.svelte`(仅样式 class,逻辑/选择器不动)
- Append: `$WT/apps/momo/src/lib/agent/bubble.css`
- Test: `$WT/apps/momo/tests/agent/chat-interactions.e2e.spec.ts`(新增 reload-answered)

- [ ] **Step 1: 确认现有 e2e(167-169)在新规则下仍绿**

`BubbleConversation.showItem`(Task 1)对 question:live resolve 后 dismiss(不在 pendingQIds 且 status≠answered)→ 跳过 → `.pf-question` count 0。**故 167-169 无需改写**(优于 spec §9 的"需改写")。

- [ ] **Step 2: bubble.css 追加选项卡皮**

```css
.momo-chat :global(.pf-question) {
  background: var(--color-surface-app);
  border: 1px solid var(--color-input-border);
  border-radius: 4px 16px 16px 16px;
  max-width: 540px;
  padding: 13px 15px;
  display: flex; flex-direction: column; gap: 9px;
}
.momo-chat :global(.pf-question-option) {
  min-height: 34px; padding: 8px 11px; border-radius: 10px;
  border: 1px solid var(--color-card-border); background: var(--color-surface-app);
  font-family: var(--font-system); font-size: 13px; color: var(--color-text-primary);
}
.momo-chat :global(.pf-question-option[data-selected="true"]) {
  background: #fff7e8; border-color: #f2dca7;
}
/* 提交按钮 cream */
.momo-chat :global(.pf-question .sc-btn[data-variant="default"]),
.momo-chat :global(.pf-question button[type="submit"]) {
  height: 32px; border-radius: var(--radius-pill); padding: 0 16px;
  background: var(--color-action-cream); color: var(--color-action-cream-text);
  border: 1px solid var(--color-action-cream-border);
  font-family: var(--font-sans); font-size: 12px; font-weight: 500;
}
```

> 先 `grep -n "pf-question\|data-selected\|Send answer\|sc-btn\|type=\"submit\"" $WT/apps/momo/src/lib/agent/components/QuestionPrompt.svelte` 校准实际类名/属性(QuestionPrompt 已有 answered 折叠分支与 `data-selected`,见 spec §7.4)。

- [ ] **Step 3: 新增 reload 历史回显 answered 的 e2e**

```ts
test("answered question persists as collapsed card after reload", async ({ page }) => {
  // 持久化 timeline 里带 answers 的 question -> normalize 归一为 status:"answered"
  // -> QuestionPrompt answered 分支折叠回显。构造方式参考 daemon.loadSessionDetail
  // 的 fake 返回(grep fakeDaemon "load_session_detail").
  const daemon = new FakeDaemon({
    sessions: [/* seeded session with an answered askUserQuestion in its timeline */]
  });
  await bootOnboarded(page, daemon);
  await page.goto("#/agent/<seededSessionId>");
  const q = page.locator(".pf-question");
  await expect(q).toBeVisible();
  await expect(q).toContainText(/已选|已回答|<chosen label>/);
});
```
> 实现前 `grep -n "load_session_detail\|timeline" $WT/apps/momo/tests/support/fakeDaemon.ts` 看如何 seed 持久化 timeline;按其结构塞一条 `kind:"question", status:"answered", answers:{...}` 或 askUserQuestion tool(由 normalize 归一)。若 fakeDaemon 不易 seed answered question,**改为 vitest 组件级测试**:渲染 `<QuestionPrompt>` 传 `item.status="answered"` + `answers`,断言折叠回显且无可点选项。

- [ ] **Step 4: check + e2e + commit**

```bash
cd $WT/apps/momo && npm run check && npm run test:desktop-ui -- chat-interactions
git -C $WT add apps/momo/src/lib/agent/components/QuestionPrompt.svelte apps/momo/src/lib/agent/bubble.css apps/momo/tests/agent/chat-interactions.e2e.spec.ts
git -C $WT commit -m "feat(momo): question options-card skin + answered history echo"
```

---

## Task 7: 底部 typing 指示器(thinking)

**Files:**
- Modify: `$WT/apps/momo/src/lib/agent/BubbleConversation.svelte`(typing 文案/计时)
- Append: `$WT/apps/momo/src/lib/agent/bubble.css`(已有 `.momo-chat__typing`)

- [ ] **Step 1: typing 文案 + 计时(turnThinking/turnStatusHint/turnStartedAtMs)**

BubbleConversation 已在 Task 1 渲染 `.momo-chat__typing`。增强:`turnThinking` → "思考中…",否则 "处理中…";有 `turnStatusHint` 优先显它;可选追加基于 `turnStartedAtMs` 的秒数(用一个 100ms `$effect` interval,参考 `ConversationView` 的 `nowMs`/`formatElapsed`,**仅 view 内部状态**)。turn 结束(`turnRunning` 转 false)即消失。**不渲染任何 thinking timeline item**(momo 无该 item,见 spec §7.3)。

- [ ] **Step 2: e2e — running 显 typing,结束消失**

```ts
test("typing indicator shows while running and clears on turn-complete", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await bootOnboarded(page, daemon);
  const { sessionId, turnId } = await startTurnFromHome(page, daemon, "Think");
  emitTurnStart(daemon, { sessionId, turnId });
  await expect(page.locator(".momo-chat__typing")).toBeVisible();
  daemon.emit(`session:${sessionId}:event`, { type: "turn-complete", turnId });
  await expect(page.locator(".momo-chat__typing")).toHaveCount(0);
});
```
> `turn-complete` 字段名先 `grep -n "turn-complete" $WT/apps/momo/src/lib/agent/sessionEvents.ts` 确认。

- [ ] **Step 3: check + e2e + commit**

```bash
cd $WT/apps/momo && npm run check && npm run test:desktop-ui -- chat-interactions
git -C $WT add apps/momo/src/lib/agent/BubbleConversation.svelte apps/momo/src/lib/agent/bubble.css apps/momo/tests/agent/chat-interactions.e2e.spec.ts
git -C $WT commit -m "feat(momo): bottom typing indicator from turn state"
```

---

## Task 8: 删除 ConversationView + 清理 + 全量验证

**Files:**
- Delete: `$WT/apps/momo/src/lib/agent/ConversationView.svelte`
- Verify: `chat.css` 保留;无残留 import

- [ ] **Step 1: 确认无残留引用**

Run: `grep -rn "ConversationView" $WT/apps/momo/src`
Expected: 仅历史注释(Agent.svelte 注释里若提到可清理);**无 import**。若有其它引用,先处理。

- [ ] **Step 2: 删除 ConversationView.svelte**

Run: `git -C $WT rm apps/momo/src/lib/agent/ConversationView.svelte`
**不要删 `chat.css`**(ToolCard/DiffCard/MessageBody/BubbleConversation 仍 import/用它)。

- [ ] **Step 3: 全量 check + 所有 agent 测试**

Run: `cd $WT/apps/momo && npm run check && npm run test:desktop-ui`
Expected: 全 PASS,含 `multiturn-tool-grouping`(驱动 controller 数据层,渲染无关)、`agent-chat-reducer`、`chat-interactions`。

- [ ] **Step 4: Commit**

```bash
git -C $WT add -A
git -C $WT commit -m "refactor(momo): remove legacy ConversationView (replaced by BubbleConversation)"
```

---

## Task 9: 视觉走查 + 端口 revert + 收尾

- [ ] **Step 1: 真实 app 视觉走查**

用 `/run` skill 或 `cd $WT/apps/momo && npm run tauri dev`(或既有 dev 命令)起 app,走一遍:发消息(左右气泡)、工具 pill 折叠/展开、含 edit 的 diff 卡、permission 审批卡、askUserQuestion 选项卡、运行中 typing。对照 Paper "Agent" 设计图核对配色/圆角/间距(serif 标题已由 Agent header 提供)。

- [ ] **Step 2: revert 测试端口改动(若 Task 1 改过)**

把 `playwright.config.ts` 端口从 1477 改回原值(merge 前必做,见 kickoff §3)。
Run: `git -C $WT diff apps/momo/playwright.config.ts`(确认已还原)。

- [ ] **Step 3: 最终全量验证**

Run: `cd $WT/apps/momo && npm run check && npm run test:desktop-ui`
Expected: 全 PASS。

- [ ] **Step 4: Commit + 准备合回**

```bash
git -C $WT add -A
git -C $WT commit -m "chore(momo): bubble phase 2 visual pass + revert test port"
git -C $WT log --oneline feat/momo-desktop..HEAD
```
合回 `feat/momo-desktop` 由用户决策(可能与 credits/kyc session 在 Agent.svelte 上有冲突,merge 时处理)。

---

## 自检(spec 覆盖 / 占位 / 类型一致)

- **spec 覆盖**:§2 决策逐条 → 纯时序(Task1 渲染开关)、气泡(Task2)、tool pill(Task3)、thinking=typing(Task7)、question 选项卡+answered(Task6)、permission 审批卡(Task5)、diff 卡(Task4)、视觉 token(Task1 桥接 + 各 task CSS)、composer 不动(未列入,即不动)、助手总结=markdown 气泡(Task2 走 MessageBody)。§7 各元素细节 → 对应 task。§9 测试 → Task1-9 的 e2e/check;§3.6 multiturn → Task8 Step3。
- **占位扫描**:每个改 CSS/代码的 step 都给了完整 CSS/代码;少数"先 grep 确认实际类名/字段"是**校准既有代码**的明确动作(非 TODO),因 ToolCard(1266 行)/QuestionPrompt(638 行)的精确内部 class 需现场对齐,已给出确切 grep 命令 + 兜底(如 reload-answered 改 vitest 组件测试)。
- **类型一致**:`lookupToolLabel(toolId, input?)`、`SHOW_RAW_AGENT_ACTIVITY`、`ToolLabel{icon,label}`、`showItem(item)`、props 名(`onResolvePermission`/`onResolveUserQuestion`/`onCancelTurn`)全计划内一致;props 子集与 `Agent.svelte:129-144` 实传吻合(已核验,无 `onSubmitMessage`)。
