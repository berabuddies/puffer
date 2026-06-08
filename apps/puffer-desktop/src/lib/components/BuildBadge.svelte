<script lang="ts">
  // Short build commit, injected at build time by vite.config.ts (see
  // __COMMIT_HASH__ in vite-env.d.ts). A faint, always-on corner label so a
  // user/support can identify exactly which build is running.
  const commit = (typeof __COMMIT_HASH__ !== "undefined" ? __COMMIT_HASH__ : "unknown").slice(0, 7);
</script>

<span class="pf-build-badge" title={`build ${commit}`}>{commit}</span>

<style>
  .pf-build-badge {
    position: fixed;
    right: 8px;
    bottom: 6px;
    z-index: 2147483600;
    font: 10px/1 ui-monospace, SFMono-Regular, Menlo, monospace;
    letter-spacing: 0.03em;
    color: var(--muted-foreground, #888);
    opacity: 0.4;
    pointer-events: none;
    user-select: none;
  }
</style>
