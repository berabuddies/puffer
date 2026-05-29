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
  /** U-card wallet REST base. Hit directly in prod; the Vite dev proxy
   *  target in dev (see vite.config.ts / backendFetch.ts). */
  readonly VITE_BACKEND_BASE_URL?: string;
  /** Vite-injected. True under `vite dev`, false under `vite build`. */
  readonly DEV?: boolean;
  /** Vite-injected. Inverse of DEV. */
  readonly PROD?: boolean;
  /** Vite-injected. Other VITE_-prefixed env vars are surfaced as strings. */
  readonly [key: string]: string | boolean | undefined;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}

/**
 * The `qrcode` npm package ships only JS — its README documents
 * `toDataURL(text, options)` but there's no shipped .d.ts and we don't
 * want to take a separate `@types/qrcode` dependency just for one call.
 * Declare the narrow subset used by TopUpModal.
 */
declare module "qrcode" {
  interface QRCodeToDataURLOptions {
    margin?: number;
    width?: number;
    color?: { dark?: string; light?: string };
  }
  function toDataURL(
    text: string,
    options?: QRCodeToDataURLOptions
  ): Promise<string>;
  const _default: { toDataURL: typeof toDataURL };
  export default _default;
  export { toDataURL };
}
