/// <reference types="svelte" />

declare module "pdfjs-dist/legacy/build/pdf.mjs" {
  export * from "pdfjs-dist";
}

declare module "pdfjs-dist/legacy/build/pdf.worker.mjs" {
  export const WorkerMessageHandler: unknown;
}

declare module "pdfjs-dist/legacy/build/pdf.worker.mjs?url" {
  const workerUrl: string;
  export default workerUrl;
}

// Build commit injected by vite.config.ts `define` (corner build badge).
declare const __COMMIT_HASH__: string;

// Minimal ambient type for the Node API used in vite.config.ts (the project
// intentionally avoids a full @types/node dependency).
declare module "node:child_process" {
  export function execSync(command: string, options?: { encoding?: string }): string;
}
