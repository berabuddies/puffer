/**
 * Direct WebSocket client for the local `puffer daemon`.
 *
 * Momo's agent/task features round-trip through the puffer daemon's wire
 * protocol. Unlike `wsClient.ts` (which connects to momo's own 1431 Tauri
 * backend and uses a type-less request envelope), this client talks the
 * daemon's protocol:
 *
 *   client → server:  { type: "request",  id, method, params }
 *   server → client:  { type: "response", id, ok, result | error }
 *                     { type: "event",    event, payload }            (unsolicited)
 *
 * Ported from `apps/puffer-desktop/src/lib/api/daemonClient.ts`. The only
 * adaptation is the handshake source: momo fetches `{ url, token }` via the
 * 1431 backend ws method `daemon_handshake` (see `ensureDaemonClient`)
 * instead of a tauri `invoke`.
 */

import { request as backendRequest } from "./wsClient";

export interface DaemonHandshake {
  url: string;
  token: string;
  protocolVersion?: string;
  workspaceRoot?: string;
}

export type ConnectionState = "idle" | "connecting" | "open" | "reconnecting" | "closed";

type PendingRequest = {
  resolve: (value: unknown) => void;
  reject: (error: Error) => void;
  timeout: ReturnType<typeof setTimeout>;
};

type WsResponseMessage = {
  type?: string;
  id?: number | string;
  ok?: boolean;
  result?: unknown;
  error?: string | { message?: string; code?: string };
};

type WsEventMessage = {
  type?: string;
  event?: string;
  payload?: unknown;
};

const REQUEST_TIMEOUT_MS = 30000;

export class DaemonClient {
  private connectionListeners = new Set<(state: ConnectionState) => void>();
  private eventListeners = new Map<string, Set<(payload: unknown) => void>>();
  private pending = new Map<string, PendingRequest>();
  private socket: WebSocket | null = null;
  private connectPromise: Promise<void> | null = null;
  private nextRequestId = 1;
  private _state: ConnectionState = "idle";

  constructor(public readonly handshake: DaemonHandshake) {}

  get state(): ConnectionState {
    return this._state;
  }

  onConnectionChange(handler: (state: ConnectionState) => void): () => void {
    this.connectionListeners.add(handler);
    handler(this._state);
    return () => {
      this.connectionListeners.delete(handler);
    };
  }

  async connect(): Promise<void> {
    if (this.socket?.readyState === WebSocket.OPEN) return;
    if (this.connectPromise) return this.connectPromise;

    this.setState(this._state === "closed" ? "reconnecting" : "connecting");
    this.connectPromise = new Promise((resolve, reject) => {
      const socket = new WebSocket(this.webSocketUrl());
      this.socket = socket;

      socket.onopen = () => {
        this.connectPromise = null;
        this.setState("open");
        void this.resubscribeActiveEvents();
        resolve();
      };
      socket.onmessage = (event) => {
        this.handleSocketMessage(String(event.data));
      };
      socket.onerror = () => {
        const error = new Error(`Unable to connect to Puffer daemon at ${this.handshake.url}`);
        if (this._state !== "open") {
          this.connectPromise = null;
          this.setState("closed");
          reject(error);
        }
      };
      socket.onclose = () => {
        this.connectPromise = null;
        this.socket = null;
        this.rejectPending(new Error("Puffer daemon WebSocket closed."));
        this.setState("closed");
      };
    });

    return this.connectPromise;
  }

  async request<T = unknown>(method: string, params: Record<string, unknown> = {}): Promise<T> {
    await this.connect();
    const socket = this.socket;
    if (!socket || socket.readyState !== WebSocket.OPEN) {
      throw new Error("Puffer daemon WebSocket is not open.");
    }

    const id = String(this.nextRequestId++);
    const request = {
      type: "request",
      id,
      method,
      params
    };

    return new Promise<T>((resolve, reject) => {
      const timeout = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`Puffer daemon request timed out: ${method}`));
      }, REQUEST_TIMEOUT_MS);
      this.pending.set(id, {
        resolve: (value) => resolve(value as T),
        reject,
        timeout
      });
      socket.send(JSON.stringify(request));
    });
  }

  on<T = unknown>(event: string, handler: (payload: T) => void): () => void {
    const wrapped = handler as (payload: unknown) => void;
    const listeners = this.eventListeners.get(event) ?? new Set();
    listeners.add(wrapped);
    this.eventListeners.set(event, listeners);
    void this.connect().catch(() => {});
    void this.subscribeEvent(event);
    return () => {
      listeners.delete(wrapped);
      if (listeners.size === 0) {
        this.eventListeners.delete(event);
        void this.unsubscribeEvent(event);
      }
    };
  }

  close(): void {
    this.socket?.close();
    this.socket = null;
    this.rejectPending(new Error("Puffer daemon client closed."));
    this.setState("closed");
  }

  private handleSocketMessage(raw: string): void {
    let message: WsResponseMessage | WsEventMessage;
    try {
      message = JSON.parse(raw) as WsResponseMessage | WsEventMessage;
    } catch {
      return;
    }

    if (message.type === "event" || (message as WsEventMessage).event) {
      const event = (message as WsEventMessage).event;
      if (!event) return;
      const listeners = this.eventListeners.get(event);
      if (!listeners) return;
      for (const listener of listeners) listener((message as WsEventMessage).payload);
      return;
    }

    if (message.type === "response" || "id" in message) {
      const response = message as WsResponseMessage;
      if (response.id == null) return;
      const id = String(response.id);
      const pending = this.pending.get(id);
      if (!pending) return;
      this.pending.delete(id);
      clearTimeout(pending.timeout);
      if (response.error) {
        pending.reject(new Error(responseErrorMessage(response.error)));
      } else if (response.ok !== false) {
        pending.resolve(response.result);
      } else {
        pending.reject(new Error("Puffer daemon request failed."));
      }
    }
  }

  private webSocketUrl(): string {
    if (!this.handshake.token) return this.handshake.url;
    try {
      const url = new URL(this.handshake.url);
      if (!url.searchParams.has("token")) {
        url.searchParams.set("token", this.handshake.token);
      }
      return url.toString();
    } catch (_error) {
      const separator = this.handshake.url.includes("?") ? "&" : "?";
      return `${this.handshake.url}${separator}token=${encodeURIComponent(this.handshake.token)}`;
    }
  }

  private async subscribeEvent(event: string): Promise<void> {
    try {
      await this.request("subscribe_event", { event });
    } catch {
      /* Older daemons broadcast all events; keep the local listener active. */
    }
  }

  private async unsubscribeEvent(event: string): Promise<void> {
    try {
      await this.request("unsubscribe_event", { event });
    } catch {
      /* Connection teardown already drops server-side subscriptions. */
    }
  }

  private async resubscribeActiveEvents(): Promise<void> {
    await Promise.all([...this.eventListeners.keys()].map((event) => this.subscribeEvent(event)));
  }

  private rejectPending(error: Error): void {
    for (const pending of this.pending.values()) {
      clearTimeout(pending.timeout);
      pending.reject(error);
    }
    this.pending.clear();
  }

  private setState(state: ConnectionState): void {
    if (this._state === state) return;
    this._state = state;
    for (const handler of this.connectionListeners) handler(state);
  }
}

function responseErrorMessage(error: string | { message?: string; code?: string }): string {
  if (typeof error === "string") return error;
  const message = error.message?.trim();
  if (message) return message;
  const code = error.code?.trim();
  return code ? `Puffer daemon error: ${code}` : "Puffer daemon request failed.";
}

let sharedClient: DaemonClient | null = null;

/** Lazily starts (via backend) and connects to the local puffer daemon. */
export async function ensureDaemonClient(): Promise<DaemonClient> {
  if (sharedClient) return sharedClient;
  const handshake = await backendRequest<DaemonHandshake>("daemon_handshake");
  sharedClient = new DaemonClient(handshake);
  await sharedClient.connect();
  return sharedClient;
}

export function currentDaemonClient(): DaemonClient | null {
  return sharedClient;
}

/**
 * Drop the memoized client (closing its socket) so the next
 * `ensureDaemonClient()` re-fetches the handshake and dials a fresh socket.
 *
 * `ensureDaemonClient` never re-dials a dead daemon on its own — once
 * `sharedClient` is set it's returned forever. Callers that observe a failed
 * request (the daemon may have restarted, invalidating the old ws URL/token)
 * call this so the connection can self-heal on the next attempt.
 */
export function resetDaemonClient(): void {
  sharedClient?.close();
  sharedClient = null;
}
