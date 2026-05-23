<!--
  AppLogo — inline SVG glyphs for the three onboarding integrations.
  We avoid bundling real brand artwork; these are simplified, brand-tinted
  marks (Gmail = red M envelope, Google Calendar = sheet with "31",
  iMessage = green chat bubble) that read at 24px inside a 40×40 circular
  rail. Pick a brand via the `name` prop.
-->
<script lang="ts">
  type Name = "gmail" | "google-calendar" | "imessage";

  interface Props {
    name: Name;
    size?: number;
  }

  let { name, size = 24 }: Props = $props();
</script>

{#if name === "gmail"}
  <svg
    width={size}
    height={size}
    viewBox="0 0 24 24"
    xmlns="http://www.w3.org/2000/svg"
    aria-hidden="true"
  >
    <!-- Simplified Gmail envelope: white body, red M roof. -->
    <rect x="2" y="5" width="20" height="14" rx="2.5" fill="#FFFFFF" stroke="#DADCE0" stroke-width="0.75" />
    <path d="M2.6 6.2L12 13l9.4-6.8" stroke="#DADCE0" stroke-width="0.75" fill="none" />
    <path d="M3 6.5l9 6.4 9-6.4V8l-9 6.4L3 8V6.5z" fill="#EA4335" />
    <path d="M3 6.5L12 12.9 21 6.5l-9 6.4z" fill="#C5221F" opacity="0.35" />
  </svg>
{:else if name === "google-calendar"}
  <svg
    width={size}
    height={size}
    viewBox="0 0 24 24"
    xmlns="http://www.w3.org/2000/svg"
    aria-hidden="true"
  >
    <!-- Google Calendar sheet with the day number. -->
    <rect x="3" y="4" width="18" height="18" rx="2.5" fill="#FFFFFF" stroke="#DADCE0" stroke-width="0.75" />
    <rect x="3" y="4" width="18" height="4" rx="2.5" fill="#4285F4" />
    <rect x="3" y="6" width="18" height="2" fill="#4285F4" />
    <rect x="6" y="2.5" width="1.5" height="4" rx="0.5" fill="#3367D6" />
    <rect x="16.5" y="2.5" width="1.5" height="4" rx="0.5" fill="#3367D6" />
    <text
      x="12"
      y="18"
      text-anchor="middle"
      font-family="'Source Serif 4', Georgia, serif"
      font-size="8"
      font-weight="500"
      fill="#4285F4"
    >31</text>
  </svg>
{:else if name === "imessage"}
  <svg
    width={size}
    height={size}
    viewBox="0 0 24 24"
    xmlns="http://www.w3.org/2000/svg"
    aria-hidden="true"
  >
    <!-- iMessage green chat bubble with tail. -->
    <path
      d="M12 3.4c-5 0-9.1 3.2-9.1 7.2 0 2.2 1.3 4.2 3.3 5.5l-.9 3 3.3-1.4c1 .3 2.2.5 3.4.5 5 0 9.1-3.2 9.1-7.2S17 3.4 12 3.4z"
      fill="#34DA52"
    />
    <path
      d="M12 3.4c-5 0-9.1 3.2-9.1 7.2 0 2.2 1.3 4.2 3.3 5.5l-.9 3 3.3-1.4c1 .3 2.2.5 3.4.5 5 0 9.1-3.2 9.1-7.2S17 3.4 12 3.4z"
      fill="url(#imessage-grad)"
      opacity="0.6"
    />
    <defs>
      <linearGradient id="imessage-grad" x1="12" y1="3.4" x2="12" y2="19.1" gradientUnits="userSpaceOnUse">
        <stop offset="0" stop-color="#5BF675" />
        <stop offset="1" stop-color="#0CB12A" />
      </linearGradient>
    </defs>
  </svg>
{/if}

<style>
  svg {
    flex-shrink: 0;
    display: block;
  }
</style>
