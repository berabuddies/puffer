/// <reference types="svelte" />

/**
 * Ambient declaration for the Vite-injected `import.meta.env`. We can't add
 * `vite/client` to tsconfig.types (the v1 build also depends on the
 * existing narrow types: ["svelte"]), so we declare just the subset of
 * ImportMeta that v2 actually reads.
 *
 * Add new entries here as you introduce new VITE_-prefixed env vars.
 */
interface ImportMetaEnv {
  readonly VITE_AUTH_STATION_URL?: string;
  readonly VITE_WORLDROUTER_CONTROL_URL?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
