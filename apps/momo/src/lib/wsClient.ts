/**
 * Tiny JSON-RPC-over-WebSocket client for the puffer Tauri backend.
 *
 * The puffer host exposes its agent API on `ws://127.0.0.1:1421/ws` (see
 * `src-tauri/src/websocket.rs`). V2 talks to that wire protocol and nothing
 * else — we deliberately do NOT import from v1 (`src/lib/api/*`) or call
 * `@tauri-apps/api` invoke. The V2 surface is meant to be extractable into
 * its own repo with this client as the only bridge.
 *
 * Protocol (mirrors `src-tauri/src/websocket.rs::handle_request`):
 *   client → server:  { id, method, params }                          (JSON)
 *   server → client:  { type: "response", id, ok: true,  result }
 *                     { type: "response", id, ok: false, error }
 *                     { type: "event",    event, payload }            (unsolicited)
 *
 * Behaviours:
 *   - Lazy connect on first `request` / `subscribe`.
 *   - Reconnects with backoff (1s, 2s, 5s, capped at 5s).
 *   - In-flight requests pending during a disconnect are rejected with an
 *     `Error("websocket disconnected")` — caller decides whether to retry.
 *   - Subscriptions persist across reconnects.
 */

type PendingRequest = {
  resolve: (value: unknown) => void;
  reject: (reason: Error) => void;
};

type EventHandler = (payload: unknown) => void;

const DEFAULT_URL = "ws://127.0.0.1:1421/ws";

function resolveUrl(): string {
  const envUrl = (import.meta as unknown as { env?: Record<string, string | undefined> }).env
    ?.VITE_PUFFER_WS_URL;
  if (typeof envUrl === "string" && envUrl.trim().length > 0) return envUrl.trim();
  return DEFAULT_URL;
}

const BACKOFF_MS = [1000, 2000, 5000];

let socket: WebSocket | null = null;
let isConnecting = false;
let reconnectAttempt = 0;
let nextRequestId = 1;
const pending = new Map<string, PendingRequest>();
const subscribers = new Map<string, Set<EventHandler>>();
let connectPromise: Promise<WebSocket> | null = null;

function scheduleReconnect(): void {
  const delay = BACKOFF_MS[Math.min(reconnectAttempt, BACKOFF_MS.length - 1)];
  reconnectAttempt += 1;
  window.setTimeout(() => {
    if (socket && socket.readyState === WebSocket.OPEN) return;
    void connect();
  }, delay);
}

function connect(): Promise<WebSocket> {
  if (socket && socket.readyState === WebSocket.OPEN) return Promise.resolve(socket);
  if (connectPromise) return connectPromise;
  isConnecting = true;
  connectPromise = new Promise<WebSocket>((resolve, reject) => {
    let ws: WebSocket;
    try {
      ws = new WebSocket(resolveUrl());
    } catch (err) {
      isConnecting = false;
      connectPromise = null;
      reject(err instanceof Error ? err : new Error(String(err)));
      scheduleReconnect();
      return;
    }
    socket = ws;
    ws.onopen = () => {
      isConnecting = false;
      reconnectAttempt = 0;
      connectPromise = null;
      resolve(ws);
    };
    ws.onmessage = (event) => {
      handleMessage(typeof event.data === "string" ? event.data : "");
    };
    ws.onerror = () => {
      // Surface via onclose; WebSocket errors don't carry useful info.
    };
    ws.onclose = () => {
      isConnecting = false;
      connectPromise = null;
      socket = null;
      // Reject every in-flight request — caller can retry on a new connection.
      for (const [, p] of pending) {
        p.reject(new Error("websocket disconnected"));
      }
      pending.clear();
      reject(new Error("websocket closed"));
      scheduleReconnect();
    };
  });
  return connectPromise;
}

function handleMessage(text: string): void {
  if (!text) return;
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    return;
  }
  if (!parsed || typeof parsed !== "object") return;
  const msg = parsed as { type?: string };
  if (msg.type === "response") {
    const { id, ok, result, error } = parsed as {
      id?: unknown;
      ok?: boolean;
      result?: unknown;
      error?: string;
    };
    if (id === undefined || id === null) return;
    const key = String(id);
    const entry = pending.get(key);
    if (!entry) return;
    pending.delete(key);
    if (ok) {
      entry.resolve(result);
    } else {
      entry.reject(new Error(typeof error === "string" ? error : "request failed"));
    }
    return;
  }
  if (msg.type === "event") {
    const { event, payload } = parsed as { event?: string; payload?: unknown };
    if (typeof event !== "string") return;
    const handlers = subscribers.get(event);
    if (!handlers) return;
    for (const handler of handlers) {
      try {
        handler(payload);
      } catch (err) {
        // Don't let one bad subscriber kill the whole dispatch.
        console.error("[wsClient] subscriber threw", err);
      }
    }
  }
}

export async function request<T>(method: string, params: Record<string, unknown> = {}): Promise<T> {
  const ws = await connect();
  const id = String(nextRequestId++);
  return new Promise<T>((resolve, reject) => {
    pending.set(id, {
      resolve: (value) => resolve(value as T),
      reject,
    });
    try {
      ws.send(JSON.stringify({ id, method, params }));
    } catch (err) {
      pending.delete(id);
      reject(err instanceof Error ? err : new Error(String(err)));
    }
  });
}

export function subscribe(eventName: string, handler: EventHandler): () => void {
  let set = subscribers.get(eventName);
  if (!set) {
    set = new Set();
    subscribers.set(eventName, set);
  }
  set.add(handler);
  // Kick off a connection so events start flowing.
  void connect().catch(() => {
    /* connection failures bubble up through scheduled reconnects */
  });
  return () => {
    const live = subscribers.get(eventName);
    if (!live) return;
    live.delete(handler);
    if (live.size === 0) subscribers.delete(eventName);
  };
}

/** Exposed for tests / diagnostics. */
export function _internalState() {
  return {
    connected: socket?.readyState === WebSocket.OPEN,
    connecting: isConnecting,
    pendingCount: pending.size,
    subscriberCount: subscribers.size,
  };
}
