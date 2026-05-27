<!--
  AppLogo — inline SVG glyphs for Connected Apps. Simplified, brand-tinted
  marks (not the real wordmarks) sized to render at 24–32px inside a clean
  white square. Page agents request a brand via the `name` prop matching
  the AppEntry.id from data/apps.ts.

  These are intentionally minimal so they read at 24px and stay readable
  next to each other on the Connected Apps grid. Brand color hints only —
  no gradients heavy enough to compete with content type.
-->
<script lang="ts">
  type Name =
    | "google-calendar"
    | "gmail"
    | "google-meet"
    | "lark"
    | "twitter"
    | "facebook"
    | "instagram"
    | "github"
    | "notion"
    | "imessage";

  interface Props {
    name: Name;
    size?: number;
  }

  let { name, size = 24 }: Props = $props();
</script>

{#if name === "google-calendar"}
  <svg
    width={size}
    height={size}
    viewBox="0 0 24 24"
    xmlns="http://www.w3.org/2000/svg"
    aria-hidden="true"
  >
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
{:else if name === "gmail"}
  <svg
    width={size}
    height={size}
    viewBox="0 0 24 24"
    xmlns="http://www.w3.org/2000/svg"
    aria-hidden="true"
  >
    <rect x="2" y="5" width="20" height="14" rx="2.5" fill="#FFFFFF" stroke="#DADCE0" stroke-width="0.75" />
    <path d="M2.6 6.2L12 13l9.4-6.8" stroke="#DADCE0" stroke-width="0.75" fill="none" />
    <path d="M3 6.5l9 6.4 9-6.4V8l-9 6.4L3 8V6.5z" fill="#EA4335" />
    <path d="M3 6.5L12 12.9 21 6.5l-9 6.4z" fill="#C5221F" opacity="0.35" />
  </svg>
{:else if name === "google-meet"}
  <svg
    width={size}
    height={size}
    viewBox="0 0 24 24"
    xmlns="http://www.w3.org/2000/svg"
    aria-hidden="true"
  >
    <!-- Camera body (green) with blue lens accent and yellow/red side tabs. -->
    <rect x="3" y="7" width="13" height="10" rx="1.5" fill="#00897B" />
    <path d="M3 9.5h2v5H3z" fill="#FBBC04" />
    <path d="M3 7l2 2.5H3z" fill="#EA4335" />
    <path d="M16 10.5l5-2.5v8l-5-2.5z" fill="#4285F4" />
    <rect x="3" y="7" width="13" height="10" rx="1.5" fill="#FFFFFF" opacity="0.06" />
  </svg>
{:else if name === "lark"}
  <svg
    width={size}
    height={size}
    viewBox="0 0 24 24"
    xmlns="http://www.w3.org/2000/svg"
    aria-hidden="true"
  >
    <!-- Lark approximation: cyan-to-blue rounded square with waveform 'L'. -->
    <defs>
      <linearGradient id="lark-grad" x1="0" y1="0" x2="1" y2="1">
        <stop offset="0" stop-color="#00D6B9" />
        <stop offset="1" stop-color="#3370FF" />
      </linearGradient>
    </defs>
    <rect x="3" y="3" width="18" height="18" rx="4" fill="url(#lark-grad)" />
    <path
      d="M8 7.5v6.5c0 1.4 1.1 2.5 2.5 2.5H16"
      stroke="#FFFFFF"
      stroke-width="2"
      stroke-linecap="round"
      fill="none"
    />
  </svg>
{:else if name === "twitter"}
  <svg
    width={size}
    height={size}
    viewBox="0 0 24 24"
    xmlns="http://www.w3.org/2000/svg"
    aria-hidden="true"
  >
    <!-- X mark: black. -->
    <rect x="2" y="2" width="20" height="20" rx="4" fill="#000000" />
    <path
      d="M7.5 7l9 10M16.5 7l-9 10"
      stroke="#FFFFFF"
      stroke-width="2"
      stroke-linecap="round"
    />
  </svg>
{:else if name === "facebook"}
  <svg
    width={size}
    height={size}
    viewBox="0 0 24 24"
    xmlns="http://www.w3.org/2000/svg"
    aria-hidden="true"
  >
    <circle cx="12" cy="12" r="10" fill="#1877F2" />
    <path
      d="M13.5 21v-7h2.3l.4-2.8h-2.7V9.4c0-.8.2-1.4 1.4-1.4h1.5V5.5c-.3 0-1.2-.1-2.2-.1-2.2 0-3.7 1.3-3.7 3.8v2H8v2.8h2.5V21h3z"
      fill="#FFFFFF"
    />
  </svg>
{:else if name === "instagram"}
  <svg
    width={size}
    height={size}
    viewBox="0 0 24 24"
    xmlns="http://www.w3.org/2000/svg"
    aria-hidden="true"
  >
    <defs>
      <linearGradient id="ig-grad" x1="0" y1="1" x2="1" y2="0">
        <stop offset="0" stop-color="#FEDA77" />
        <stop offset="0.45" stop-color="#F58529" />
        <stop offset="0.7" stop-color="#DD2A7B" />
        <stop offset="1" stop-color="#8134AF" />
      </linearGradient>
    </defs>
    <rect x="2.5" y="2.5" width="19" height="19" rx="5.5" fill="url(#ig-grad)" />
    <rect x="6.5" y="6.5" width="11" height="11" rx="3.5" fill="none" stroke="#FFFFFF" stroke-width="1.6" />
    <circle cx="12" cy="12" r="2.8" fill="none" stroke="#FFFFFF" stroke-width="1.6" />
    <circle cx="17" cy="7" r="0.9" fill="#FFFFFF" />
  </svg>
{:else if name === "github"}
  <svg
    width={size}
    height={size}
    viewBox="0 0 24 24"
    xmlns="http://www.w3.org/2000/svg"
    aria-hidden="true"
  >
    <!-- Simplified octocat silhouette. -->
    <path
      d="M12 2.2a9.8 9.8 0 0 0-3.1 19.1c.5.1.7-.2.7-.5v-1.7c-2.7.6-3.3-1.3-3.3-1.3-.4-1.1-1.1-1.4-1.1-1.4-.9-.6.1-.6.1-.6 1 .1 1.5 1 1.5 1 .9 1.5 2.3 1.1 2.9.8.1-.6.3-1.1.6-1.3-2.2-.2-4.5-1.1-4.5-4.9 0-1.1.4-2 1-2.7-.1-.2-.4-1.2.1-2.6 0 0 .8-.3 2.7 1a9.4 9.4 0 0 1 4.9 0c1.9-1.3 2.7-1 2.7-1 .5 1.4.2 2.4.1 2.6.6.7 1 1.6 1 2.7 0 3.8-2.3 4.7-4.5 4.9.4.3.7.9.7 1.8v2.6c0 .3.2.6.7.5A9.8 9.8 0 0 0 12 2.2z"
      fill="#181717"
    />
  </svg>
{:else if name === "notion"}
  <svg
    width={size}
    height={size}
    viewBox="0 0 24 24"
    xmlns="http://www.w3.org/2000/svg"
    aria-hidden="true"
  >
    <rect x="3" y="3" width="18" height="18" rx="3" fill="#FFFFFF" stroke="#161616" stroke-width="1.2" />
    <path
      d="M8.6 7.5h2.1l4.4 6.3V7.5h1.8v9h-2L10.4 9.9V16.5H8.6z"
      fill="#161616"
    />
  </svg>
{:else if name === "imessage"}
  <svg
    width={size}
    height={size}
    viewBox="0 0 24 24"
    xmlns="http://www.w3.org/2000/svg"
    aria-hidden="true"
  >
    <path
      d="M12 3.4c-5 0-9.1 3.2-9.1 7.2 0 2.2 1.3 4.2 3.3 5.5l-.9 3 3.3-1.4c1 .3 2.2.5 3.4.5 5 0 9.1-3.2 9.1-7.2S17 3.4 12 3.4z"
      fill="#34DA52"
    />
    <path
      d="M12 3.4c-5 0-9.1 3.2-9.1 7.2 0 2.2 1.3 4.2 3.3 5.5l-.9 3 3.3-1.4c1 .3 2.2.5 3.4.5 5 0 9.1-3.2 9.1-7.2S17 3.4 12 3.4z"
      fill="url(#imessage-card-grad)"
      opacity="0.6"
    />
    <defs>
      <linearGradient id="imessage-card-grad" x1="12" y1="3.4" x2="12" y2="19.1" gradientUnits="userSpaceOnUse">
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
