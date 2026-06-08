<script lang="ts">
  import { onDestroy } from "svelte";
  import {
    rankTelegramRelationships,
    subscribeTelegramRelationships,
    type TelegramRelationshipReport,
    type TelegramRelationshipsResult
  } from "../api/desktop";

  let loading = $state(false);
  let error = $state<string | null>(null);
  let phase = $state("");
  let progress = $state("");
  let connectionSlug = $state<string | null>(null);
  let reports = $state<TelegramRelationshipReport[]>([]);
  let useLocal = $state(false); // default cloud (gpt-5.4-mini, ~$0.002/run); local = privacy
  let unsubscribe: (() => void) | null = null;

  const PHASE_LABELS: Record<string, string> = {
    ranking: "按聊天频率排序中…",
    ranked: "已选出 Top-5,开始分析…",
    analyzing: "分析中",
    analyzed: "已分析",
    done: "完成"
  };

  function teardown() {
    unsubscribe?.();
    unsubscribe = null;
  }

  function onEvent({ phase: p, data }: { connectionSlug: string; phase: string; data: unknown }) {
    phase = p;
    const d = (data ?? {}) as Record<string, unknown>;
    if (p === "ranked" && Array.isArray(d.contacts)) {
      // Seed cards with names/counts; verdicts fill in as each is analyzed.
      reports = (d.contacts as Array<Record<string, unknown>>).map((c) => ({
        chatId: Number(c.chatId ?? 0),
        name: String(c.name ?? ""),
        messageCount: Number(c.messageCount ?? 0),
        relationship: null,
        closeness: null,
        tone: null,
        evidence: null
      }));
    } else if (p === "analyzing") {
      progress = `${PHASE_LABELS.analyzing} ${d.index ?? "?"}/${d.total ?? "?"}：${d.name ?? ""}`;
    } else if (p === "analyzed") {
      const r = d as unknown as TelegramRelationshipReport;
      reports = reports.map((existing) => (existing.chatId === r.chatId ? r : existing));
    }
  }

  async function analyze() {
    if (loading) return;
    loading = true;
    error = null;
    phase = "ranking";
    progress = "";
    reports = [];
    teardown();
    try {
      unsubscribe = await subscribeTelegramRelationships(onEvent);
      const result: TelegramRelationshipsResult = await rankTelegramRelationships({ useLocal });
      connectionSlug = result.connectionSlug;
      reports = result.reports;
      phase = "done";
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
      teardown();
    }
  }

  function closenessDots(n: number | null): string {
    const v = Math.max(0, Math.min(5, n ?? 0));
    return "●".repeat(v) + "○".repeat(5 - v);
  }

  onDestroy(teardown);
</script>

<div class="pf-tg-rel">
  <header class="pf-tg-rel__head">
    <div>
      <h1>Telegram 关系分析</h1>
      <p class="pf-tg-rel__sub">
        按近 90 天聊天频率取 Top-5 联系人，由本地 qwen35 模型分析你们的关系。
        {#if connectionSlug}<span class="pf-tg-rel__slug">· {connectionSlug}</span>{/if}
      </p>
    </div>
    <div class="pf-tg-rel__actions">
      <label class="pf-tg-rel__model" title="云端更准更便宜(~$0.002/次);本地完全私密,需本地模型在跑">
        <input type="checkbox" bind:checked={useLocal} disabled={loading} />
        本地模型(隐私)
      </label>
      <button class="pf-btn" onclick={analyze} disabled={loading}>
        {loading ? "分析中…" : "分析关系"}
      </button>
    </div>
  </header>

  {#if loading}
    <p class="pf-tg-rel__status">{PHASE_LABELS[phase] ?? phase}{progress ? ` · ${progress}` : ""}</p>
  {/if}
  {#if error}
    <p class="pf-tg-rel__error">出错了：{error}</p>
  {/if}

  {#if reports.length > 0}
    <ol class="pf-tg-rel__list">
      {#each reports as r, i (r.chatId)}
        <li class="pf-tg-rel__card">
          <div class="pf-tg-rel__rank">{i + 1}</div>
          <div class="pf-tg-rel__body">
            <div class="pf-tg-rel__title">
              <span class="pf-tg-rel__name">{r.name || "(未知)"}</span>
              <span class="pf-tg-rel__count">{r.messageCount} 条</span>
            </div>
            {#if r.relationship}
              <div class="pf-tg-rel__verdict">
                <span class="pf-tg-rel__rel">{r.relationship}</span>
                {#if r.closeness != null}<span class="pf-tg-rel__dots" title="亲密度 {r.closeness}/5">{closenessDots(r.closeness)}</span>{/if}
                {#if r.tone}<span class="pf-tg-rel__tone">{r.tone}</span>{/if}
              </div>
              {#if r.evidence}<p class="pf-tg-rel__evidence">{r.evidence}</p>{/if}
            {:else}
              <p class="pf-tg-rel__pending">待分析…</p>
            {/if}
          </div>
        </li>
      {/each}
    </ol>
  {:else if !loading && !error}
    <p class="pf-tg-rel__empty">点击「分析关系」开始。需要本地 qwen35 正在运行，且已连接的 Telegram 账号有聊天记录。</p>
  {/if}
</div>

<style>
  .pf-tg-rel {
    padding: 24px;
    display: flex;
    flex-direction: column;
    gap: 16px;
    overflow-y: auto;
  }
  .pf-tg-rel__head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 16px;
  }
  .pf-tg-rel__head h1 {
    margin: 0;
    font-size: 1.25rem;
    font-weight: 650;
  }
  .pf-tg-rel__sub {
    margin: 4px 0 0;
    font-size: 0.85rem;
    opacity: 0.7;
  }
  .pf-tg-rel__slug {
    opacity: 0.6;
  }
  .pf-tg-rel__actions {
    display: flex;
    align-items: center;
    gap: 14px;
  }
  .pf-tg-rel__model {
    display: flex;
    align-items: center;
    gap: 5px;
    font-size: 0.8rem;
    opacity: 0.75;
    white-space: nowrap;
    cursor: pointer;
  }
  .pf-btn {
    padding: 8px 16px;
    border-radius: 8px;
    border: 1px solid var(--pf-border, rgba(127, 127, 127, 0.3));
    background: var(--pf-accent, #0891b2);
    color: #fff;
    font-weight: 600;
    cursor: pointer;
    white-space: nowrap;
  }
  .pf-btn:disabled {
    opacity: 0.6;
    cursor: default;
  }
  .pf-tg-rel__status {
    margin: 0;
    font-size: 0.85rem;
    opacity: 0.75;
  }
  .pf-tg-rel__error {
    margin: 0;
    font-size: 0.85rem;
    color: #dc2626;
  }
  .pf-tg-rel__empty {
    margin: 0;
    font-size: 0.85rem;
    opacity: 0.6;
  }
  .pf-tg-rel__list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .pf-tg-rel__card {
    display: flex;
    gap: 14px;
    align-items: flex-start;
    border: 1px solid var(--pf-border, rgba(127, 127, 127, 0.25));
    border-radius: 10px;
    padding: 14px 16px;
    background: var(--pf-surface, rgba(127, 127, 127, 0.06));
  }
  .pf-tg-rel__rank {
    font-size: 1.1rem;
    font-weight: 700;
    opacity: 0.5;
    min-width: 1.2em;
  }
  .pf-tg-rel__body {
    flex: 1;
    min-width: 0;
  }
  .pf-tg-rel__title {
    display: flex;
    align-items: baseline;
    gap: 10px;
  }
  .pf-tg-rel__name {
    font-weight: 600;
  }
  .pf-tg-rel__count {
    font-size: 0.8rem;
    opacity: 0.55;
  }
  .pf-tg-rel__verdict {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-top: 4px;
    flex-wrap: wrap;
  }
  .pf-tg-rel__rel {
    font-weight: 600;
    color: var(--pf-accent, #0891b2);
  }
  .pf-tg-rel__dots {
    letter-spacing: 1px;
    color: var(--pf-accent, #0891b2);
    font-size: 0.8rem;
  }
  .pf-tg-rel__tone {
    font-size: 0.8rem;
    opacity: 0.7;
  }
  .pf-tg-rel__evidence {
    margin: 6px 0 0;
    font-size: 0.85rem;
    opacity: 0.8;
    line-height: 1.4;
  }
  .pf-tg-rel__pending {
    margin: 4px 0 0;
    font-size: 0.8rem;
    opacity: 0.5;
  }
</style>
