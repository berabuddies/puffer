<script lang="ts">
  import "./chat.css";   // 复用 .pf-* 类 + --pf-chat-*-size(ToolCard/DiffCard/MessageBody 依赖)
  import "./bubble.css"; // momo 桥接 + 气泡皮
  import MessageBody from "./components/MessageBody.svelte";
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
    timeline, pendingPermissions, pendingQuestions, loading,
    turnRunning = false, turnThinking = false,
    turnStatusHint = null, onResolvePermission, onResolveUserQuestion
  }: Props = $props();

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
            <Approval item={item as PermissionTimelineItem} onResolve={onResolvePermission} />
          {:else if item.kind === "question"}
            <QuestionPrompt
              item={item as UserQuestionTimelineItem}
              disabled={(item as UserQuestionTimelineItem).status === "answered"}
              onResolve={onResolveUserQuestion}
            />
          {:else}
            <!-- user/assistant/system/command 消息;markdown 走 MessageBody。Task 2 抽成 ChatBubble。 -->
            <div class="mb-row" data-role={(item as MessageTimelineItem).kind}>
              <div class="mb-bubble">
                <div class="pf-msg-text"><MessageBody body={(item as MessageTimelineItem).body} /></div>
              </div>
            </div>
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
