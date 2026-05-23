/**
 * Global toast store (Svelte 5 runes).
 *
 * Exposes a $state array of active toasts plus a `pushToast(message, kind?)`
 * helper. Each toast auto-dismisses after `TOAST_TTL_MS` (3s); callers can
 * also remove one early via `dismissToast(id)`.
 *
 * Renderer: src-v2/components/common/Toast.svelte (mounted once in App.svelte).
 */

export type ToastKind = "info" | "success" | "error";

export interface Toast {
  id: number;
  message: string;
  kind: ToastKind;
}

const TOAST_TTL_MS = 3000;

/** Reactive list of active toasts; the renderer iterates this directly. */
export const toasts = $state<Toast[]>([]);

let nextId = 1;

/** Push a toast onto the stack. Returns the assigned id (useful for tests). */
export function pushToast(message: string, kind: ToastKind = "info"): number {
  const id = nextId++;
  toasts.push({ id, message, kind });
  if (typeof window !== "undefined") {
    window.setTimeout(() => dismissToast(id), TOAST_TTL_MS);
  }
  return id;
}

/** Remove a toast by id. No-op if it's already gone. */
export function dismissToast(id: number): void {
  const index = toasts.findIndex((t) => t.id === id);
  if (index >= 0) toasts.splice(index, 1);
}
