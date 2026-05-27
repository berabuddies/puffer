<!--
  AppLogo — inline SVG glyphs for Connected Apps. Simplified, brand-tinted
  marks sized to render at 24-32px inside a clean white square. Page
  agents request a brand via the `name` prop matching the AppEntry.id
  from data/apps.ts.

  Catalogue is intentionally minimal: only the connectors momo can
  actually authenticate today (Telegram + Email) get rendered here.
-->
<script lang="ts">
  type Name = "telegram" | "email";

  interface Props {
    name: Name;
    size?: number;
  }

  let { name, size = 24 }: Props = $props();
</script>

{#if name === "telegram"}
  <svg
    width={size}
    height={size}
    viewBox="0 0 24 24"
    xmlns="http://www.w3.org/2000/svg"
    aria-hidden="true"
  >
    <defs>
      <linearGradient id="telegram-grad" x1="0" y1="0" x2="0" y2="1">
        <stop offset="0" stop-color="#37AEE2" />
        <stop offset="1" stop-color="#1E96C8" />
      </linearGradient>
    </defs>
    <circle cx="12" cy="12" r="10" fill="url(#telegram-grad)" />
    <path
      d="M7.2 11.7l9.6-3.7c.5-.2.9.1.7.7L15.9 17c-.1.5-.5.6-.9.4l-2.6-1.9-1.3 1.2c-.1.1-.3.2-.5.2l.2-2.6 4.7-4.2c.2-.2 0-.3-.3-.1l-5.8 3.7-2.5-.8c-.5-.2-.5-.5.2-.7z"
      fill="#FFFFFF"
    />
  </svg>
{:else if name === "email"}
  <svg
    width={size}
    height={size}
    viewBox="0 0 24 24"
    xmlns="http://www.w3.org/2000/svg"
    aria-hidden="true"
  >
    <rect x="2.5" y="5" width="19" height="14" rx="2.5" fill="#FFFFFF" stroke="#161616" stroke-width="1.2" />
    <path d="M3.5 7.5L12 13l8.5-5.5" stroke="#161616" stroke-width="1.2" fill="none" stroke-linecap="round" />
    <path d="M3.5 7.5L12 13l8.5-5.5" stroke="#161616" stroke-width="0.6" fill="none" opacity="0.4" />
  </svg>
{/if}

<style>
  svg {
    flex-shrink: 0;
    display: block;
  }
</style>
