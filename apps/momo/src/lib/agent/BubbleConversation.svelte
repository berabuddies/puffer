<script lang="ts">
  import "./chat.css";   // 复用 .pf-* 类 + --pf-chat-*-size(ToolCard/DiffCard/MessageBody 依赖)
  import "./bubble.css"; // momo 桥接 + 气泡皮
  import { tick } from "svelte";
  import ChatBubble from "./components/ChatBubble.svelte";
  import ToolCard from "./components/ToolCard.svelte";
  import DiffCard from "./components/DiffCard.svelte";
  import Approval from "./components/Approval.svelte";
  import QuestionPrompt from "./components/QuestionPrompt.svelte";
  import type {
    TimelineItem, PermissionTimelineItem, UserQuestionTimelineItem,
    ToolTimelineItem, DiffTimelineItem, MessageTimelineItem, SessionListItem
  } from "./types";

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
  // `onCancelTurn` is reserved (kept so Agent.svelte's binding stays stable);
  // momo currently drives Stop from the shell <Composer>, not from here.

  // View-internal clock for the typing indicator's elapsed-seconds suffix.
  // Ticks only while a turn runs; the $effect cleanup clears the interval when
  // the turn ends or the component unmounts (no runaway timer). The effect
  // depends on `turnRunning` only — it writes `nowMs` but never reads it, so it
  // never retriggers itself.
  let nowMs = $state(0);
  $effect(() => {
    if (!turnRunning) return;
    nowMs = Date.now();
    const id = setInterval(() => { nowMs = Date.now(); }, 100);
    return () => clearInterval(id);
  });
  let typingLabel = $derived(turnStatusHint ?? (turnThinking ? "思考中…" : "处理中…"));
  let elapsedSuffix = $derived(
    turnRunning && turnStartedAtMs && nowMs > turnStartedAtMs
      ? ` (${((nowMs - turnStartedAtMs) / 1000).toFixed(1)}s)`
      : ""
  );

  let threadEl: HTMLDivElement | undefined;
  let lastSessionId: string | null = null;

  // Reset to the top when switching sessions so a new thread doesn't inherit
  // the previous one's scroll position. Guarded on an actual id change so it
  // doesn't fight the auto-scroll effect below on every timeline tick (ported
  // from ConversationView's lastSessionId pattern).
  $effect(() => {
    const nextSessionId = session?.id ?? null;
    if (nextSessionId === lastSessionId) return;
    lastSessionId = nextSessionId;
    void tick().then(() => {
      if (threadEl) threadEl.scrollTop = 0;
    });
  });

  // Auto-scroll to the newest content when the timeline grows or a turn starts/
  // stops (ported from ConversationView). `tick()` lets the new rows mount
  // before we measure scrollHeight.
  $effect(() => {
    // Touch the reactive deps so the effect re-runs on new content.
    void timeline.length;
    void turnRunning;
    void tick().then(() => {
      if (threadEl) threadEl.scrollTop = threadEl.scrollHeight;
    });
  });

  let pendingPermIds = $derived(new Set(pendingPermissions.map((p) => p.id)));
  let pendingQIds = $derived(new Set(pendingQuestions.map((q) => q.id)));

  // 渲染规则(render-layer only,保 e2e 绿):
  //  permission: 仅当仍 pending 才渲染可操作卡;已 dismiss(live resolve)→跳过。
  //  question:   pending → 可操作;status==="answered"(历史)→ 折叠回显;否则跳过。
  function showItem(item: TimelineItem): boolean {
    if (item.kind === "permission") return pendingPermIds.has(item.id);
    if (item.kind === "question")
      return pendingQIds.has(item.id) || item.status === "answered";
    return true;
  }
</script>

<div class="momo-chat">
  <div class="momo-chat__thread" bind:this={threadEl}>
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
            <Approval item={item as PermissionTimelineItem} onResolve={onResolvePermission} />
          {:else if item.kind === "question"}
            <QuestionPrompt
              item={item as UserQuestionTimelineItem}
              disabled={(item as UserQuestionTimelineItem).status === "answered"}
              onResolve={onResolveUserQuestion}
            />
          {:else}
            <!-- user/assistant/system/command 消息;每角色样式 + markdown 由 ChatBubble 处理。 -->
            <ChatBubble item={item as MessageTimelineItem} />
          {/if}
        {/if}
      {/each}

      {#if turnRunning}
        <div class="momo-chat__typing" aria-live="polite">
          {typingLabel}{elapsedSuffix}
        </div>
      {/if}
    </div>
  </div>
</div>
