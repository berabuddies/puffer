<!--
  Dev-only chat UI gallery.

  Renders the REAL `BubbleConversation` with a hand-built fixture timeline that
  exercises every chat surface at once — so you can eyeball the whole bubble UI
  (and click to expand tool pills / resolve cards) without driving a live agent
  turn into each state one message at a time.

  Reachable at `#/dev/chat-gallery` in dev builds only (the route is registered
  behind `import.meta.env.DEV` in routes.ts, so Vite drops it from prod). Pure
  fixture data — no controller, no daemon, no network.
-->
<script lang="ts">
  import BubbleConversation from "../../lib/agent/BubbleConversation.svelte";
  import type {
    TimelineItem,
    PermissionTimelineItem,
    UserQuestionTimelineItem
  } from "../../lib/agent/types";

  // TimelineBase requires id/kind/title/summary/body/meta; the rest is optional.
  function msg(id: string, kind: "user" | "assistant" | "system" | "command", body: string, status: string | null = null): TimelineItem {
    return { id, kind, title: "", summary: "", body, meta: [], status, actor: null };
  }

  let timeline = $state<TimelineItem[]>([
    msg("u1", "user", "把首页登录按钮换成品牌色,顺便跑下测试"),
    msg(
      "a1",
      "assistant",
      "好的,我来改。计划:\n\n- 读取 `LoginButton.svelte`\n- 把背景换成 `--brand-500`,加 `data-variant=\"primary\"`\n- 跑 `npm test` 验证\n\n开始执行。"
    ),
    {
      id: "t1", kind: "tool", title: "", summary: "", body: "", meta: [], status: "success",
      toolName: "read_file",
      input: '{"path":"src/components/LoginButton.svelte"}',
      output: '<button class="login" style="background: #3b82f6">Sign in</button>',
      inputJson: { path: "src/components/LoginButton.svelte" }
    },
    {
      id: "d1", kind: "diff", title: "", summary: "", body: "", meta: [], status: null,
      diff: {
        id: "diff-1", source: "agent", title: "src/components/LoginButton.svelte",
        command: "edit_file", status: "applied", unstagedDiffstat: "", stagedDiffstat: "",
        patch:
          "@@ -9,4 +9,5 @@\n" +
          "   <button\n" +
          '-    class="login" style="background: #3b82f6"\n' +
          '+    class="login" style="background: var(--brand-500)"\n' +
          '+    data-variant="primary"\n' +
          "   >Sign in</button>"
      }
    },
    {
      id: "p1", kind: "permission", title: "", summary: "Run `npm test`", body: "", meta: [], status: "pending",
      toolName: "bash", scopeLabel: null,
      permissionDialog: {
        state: "pending", reason: "在工作区里运行一条 shell 命令验证改动", summary: "Run `npm test`",
        inputText: "npm test", toolName: "bash", choices: ["Approve once", "Always allow", "Deny"]
      },
      choices: ["Approve once", "Always allow", "Deny"]
    },
    {
      id: "t2", kind: "tool", title: "", summary: "", body: "", meta: [], status: "failed",
      toolName: "bash",
      input: '{"command":"npm run lint"}',
      output: "$ npm run lint\n✖ 1 problem (1 error, 0 warnings)\n  LoginButton.svelte:10  unexpected token",
      inputJson: { command: "npm run lint" }
    },
    {
      id: "q1", kind: "question", title: "", summary: "", body: "", meta: [], status: "pending",
      questions: [
        {
          question: "用哪个品牌色变量?", header: "配色", type: "choice", multiSelect: false,
          options: [
            { label: "--brand-500", description: "主品牌色,登录 / CTA 通用" },
            { label: "--brand-600", description: "深一档,用于 hover 态" }
          ]
        }
      ]
    },
    {
      id: "q2", kind: "question", title: "", summary: "", body: "", meta: [], status: "answered",
      questions: [
        {
          question: "确认部署目标?", header: "部署", type: "choice", multiSelect: false,
          options: [
            { label: "Staging", description: "安全预览" },
            { label: "Production", description: "线上流量" }
          ]
        }
      ],
      answers: { "确认部署目标?": "Staging" }
    },
    msg("s1", "system", "已切换到 workspace-write 权限。", null),
    msg("a2", "assistant", "**搞定 ✅** 登录按钮已改用 `--brand-500` 并加了 `data-variant=\"primary\"`,测试 12 个全过。")
  ]);

  let pendingPermissions = $state<PermissionTimelineItem[]>([
    timeline.find((t) => t.id === "p1") as PermissionTimelineItem
  ]);
  let pendingQuestions = $state<UserQuestionTimelineItem[]>([
    timeline.find((t) => t.id === "q1") as UserQuestionTimelineItem
  ]);

  let turnRunning = $state(true);

  // Light interactivity so the cards behave like the real thing: resolving a
  // permission dismisses it; answering the pending question collapses it into
  // its answered echo.
  function onResolvePermission(permissionId: string, _choice: string): void {
    pendingPermissions = pendingPermissions.filter((p) => p.id !== permissionId);
  }
  function onResolveUserQuestion(
    questionId: string,
    answers: Record<string, string | string[]>
  ): void {
    const item = timeline.find((t) => t.id === questionId) as UserQuestionTimelineItem | undefined;
    if (item) {
      item.status = "answered";
      item.answers = answers;
    }
    pendingQuestions = pendingQuestions.filter((q) => q.id !== questionId);
  }
</script>

<div class="gallery">
  <header class="gallery__bar">
    <span class="gallery__title">Chat UI gallery</span>
    <span class="gallery__hint">fixture data · 真实 BubbleConversation · 点工具 pill 可展开 · 审批/答题可交互</span>
    <label class="gallery__toggle">
      <input type="checkbox" bind:checked={turnRunning} /> typing 指示
    </label>
  </header>
  <div class="gallery__stage">
    <BubbleConversation
      session={null}
      {timeline}
      {pendingPermissions}
      {pendingQuestions}
      loading={false}
      {turnRunning}
      turnStartedAtMs={turnRunning ? Date.now() - 3200 : null}
      turnThinking={true}
      turnStatusHint={null}
      {onResolvePermission}
      {onResolveUserQuestion}
      onCancelTurn={() => {}}
    />
  </div>
</div>

<style>
  .gallery {
    display: flex;
    flex-direction: column;
    height: 100vh;
    width: 100%;
    background: var(--color-surface-app);
  }
  .gallery__bar {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 10px 24px;
    border-bottom: 1px solid var(--color-card-border);
    font-family: var(--font-system);
  }
  .gallery__title {
    font-family: var(--font-serif);
    font-size: 18px;
    color: var(--color-text-primary);
  }
  .gallery__hint {
    font-size: 12px;
    color: var(--color-text-muted);
  }
  .gallery__toggle {
    margin-left: auto;
    font-size: 12px;
    color: var(--color-text-secondary);
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  /* BubbleConversation is flex:1/min-height:0 — give it a flex slot to fill. */
  .gallery__stage {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
</style>
