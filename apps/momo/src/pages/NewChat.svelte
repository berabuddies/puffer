<!--
  NewChat — the "/new" page. Intentionally minimal: just the chat Composer,
  centered. No greeting, no mascot, no model picker — the user wanted a bare
  input box (see spec 2026-05-30-momo-sidebar-new-chat).

  Clicking the sidebar "+" lands here WITHOUT creating a session. The session
  is minted only when the user sends their first message: we pass a custom
  `onsubmit` so Composer's default branch (which would treat the "/new" path as
  an active /agent/<id> session) is bypassed. `createSessionFromText` creates
  the session, registers the optimistic rail row + first-10-chars title, fires
  the first turn, and returns the new id to navigate into.
-->
<script lang="ts">
  import Composer from "../components/shell/Composer.svelte";
  import { createSessionFromText } from "../lib/agent/agentChat.svelte";
  import { navigate } from "../router.svelte";
  import { pushToast } from "../lib/toast.svelte";

  function onSubmit(text: string): void {
    void createSessionFromText(text)
      .then((sessionId) => navigate(`/agent/${sessionId}`))
      .catch(() => {
        // Stay on /new so the user can retry; surface the failure.
        pushToast("Couldn't start a new chat — try again.", "error");
      });
  }
</script>

<div class="new-chat">
  <div class="new-chat__inner">
    <Composer placeholder="Hi, Tomo. How's my luck today?" onsubmit={onSubmit} />
  </div>
</div>

<style>
  /* Fullbleed page (see routes.ts): the column is locked to viewport height
     with no padding, so we center the composer ourselves both axes. */
  .new-chat {
    flex: 1;
    min-height: 0;
    height: 100%;
    width: 100%;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 0 var(--shell-page-padding);
  }
  .new-chat__inner {
    width: 100%;
    max-width: var(--shell-page-max);
  }
  /* Composer ships its own top divider; there's nothing above it here. */
  .new-chat :global(.composer) {
    border-top: 0;
  }
</style>
