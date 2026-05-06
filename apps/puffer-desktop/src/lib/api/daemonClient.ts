//! Compatibility client for the copied Puffer UI.
//!
//! Files, terminals, and Browser panes all round-trip through this daemon
//! client. In the Tauri shell it can still fall back to invoke, but the
//! default path is the local WebSocket bridge so the Vite browser preview has
//! the same backend surface.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type DaemonHandshake = {
  url: string;
  token: string;
  protocolVersion: string;
  workspaceRoot: string;
};

export type ConnectionState = "idle" | "connecting" | "open" | "reconnecting" | "closed";

type BackendEventEnvelope = {
  event: string;
  payload: unknown;
};

type PendingRequest = {
  resolve: (value: unknown) => void;
  reject: (error: Error) => void;
  timeout: ReturnType<typeof setTimeout>;
};

type WsResponseMessage = {
  type?: string;
  id?: number;
  ok?: boolean;
  result?: unknown;
  error?: string;
};

type WsEventMessage = {
  type?: string;
  event?: string;
  payload?: unknown;
};

const DEFAULT_WS_URL = "ws://127.0.0.1:1421/ws";
const REQUEST_TIMEOUT_MS = 30000;

export class DaemonClient {
  private connectionListeners = new Set<(state: ConnectionState) => void>();
  private eventListeners = new Map<string, Set<(payload: unknown) => void>>();
  private pending = new Map<number, PendingRequest>();
  private socket: WebSocket | null = null;
  private connectPromise: Promise<void> | null = null;
  private nextRequestId = 1;
  private _state: ConnectionState = "idle";
  private readonly useWebSocket: boolean;

  constructor(
    public readonly handshake: DaemonHandshake = {
      url: "tauri://corbina",
      token: "",
      protocolVersion: "1",
      workspaceRoot: ""
    }
  ) {
    this.useWebSocket = handshake.url.startsWith("ws://") || handshake.url.startsWith("wss://");
    this._state = this.useWebSocket ? "idle" : "open";
  }

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
    if (!this.useWebSocket) {
      this.setState("open");
      return;
    }
    if (this.socket?.readyState === WebSocket.OPEN) return;
    if (this.connectPromise) return this.connectPromise;

    this.setState(this._state === "closed" ? "reconnecting" : "connecting");
    this.connectPromise = new Promise((resolve, reject) => {
      const socket = new WebSocket(this.handshake.url);
      this.socket = socket;

      socket.onopen = () => {
        this.connectPromise = null;
        this.setState("open");
        resolve();
      };
      socket.onmessage = (event) => {
        this.handleSocketMessage(String(event.data));
      };
      socket.onerror = () => {
        const error = new Error(`Unable to connect to Corbina backend at ${this.handshake.url}`);
        if (this._state !== "open") {
          this.connectPromise = null;
          this.setState("closed");
          reject(error);
        }
      };
      socket.onclose = () => {
        this.connectPromise = null;
        this.socket = null;
        this.rejectPending(new Error("Corbina backend WebSocket closed."));
        this.setState("closed");
      };
    });

    return this.connectPromise;
  }

  async request<T = unknown>(method: string, params: Record<string, unknown> = {}): Promise<T> {
    if (!this.useWebSocket) {
      return invoke<T>("backend_request", { method, params });
    }

    await this.connect();
    const socket = this.socket;
    if (!socket || socket.readyState !== WebSocket.OPEN) {
      throw new Error("Corbina backend WebSocket is not open.");
    }

    const id = this.nextRequestId++;
    const request = {
      type: "request",
      id,
      method,
      params,
      token: this.handshake.token
    };

    return new Promise<T>((resolve, reject) => {
      const timeout = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`Corbina backend request timed out: ${method}`));
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
    if (this.useWebSocket) {
      const wrapped = handler as (payload: unknown) => void;
      const listeners = this.eventListeners.get(event) ?? new Set();
      listeners.add(wrapped);
      this.eventListeners.set(event, listeners);
      void this.connect().catch(() => {});
      return () => {
        listeners.delete(wrapped);
        if (listeners.size === 0) this.eventListeners.delete(event);
      };
    }

    let active = true;
    let unlisten: UnlistenFn | null = null;
    const pending = listen<BackendEventEnvelope>("corbina:event", (nativeEvent) => {
      if (!active) return;
      const payload = nativeEvent.payload;
      if (payload?.event === event) {
        handler(payload.payload as T);
      }
    });
    void pending.then((next) => {
      unlisten = next;
      if (!active) unlisten();
    });

    return () => {
      active = false;
      if (unlisten) {
        unlisten();
      } else {
        void pending.then((next) => next());
      }
    };
  }

  off(): void {
    // Per-listener disposers are returned from on().
  }

  close(): void {
    this.socket?.close();
    this.socket = null;
    this.rejectPending(new Error("Corbina backend client closed."));
    this.setState("closed");
  }

  private handleSocketMessage(raw: string): void {
    let message: WsResponseMessage | WsEventMessage;
    try {
      message = JSON.parse(raw) as WsResponseMessage | WsEventMessage;
    } catch {
      return;
    }

    if (message.type === "event") {
      const event = (message as WsEventMessage).event;
      if (!event) return;
      const listeners = this.eventListeners.get(event);
      if (!listeners) return;
      for (const listener of listeners) listener((message as WsEventMessage).payload);
      return;
    }

    if (message.type === "response") {
      const response = message as WsResponseMessage;
      if (response.id == null) return;
      const pending = this.pending.get(response.id);
      if (!pending) return;
      this.pending.delete(response.id);
      clearTimeout(pending.timeout);
      if (response.ok) {
        pending.resolve(response.result);
      } else {
        pending.reject(new Error(response.error || "Corbina backend request failed."));
      }
    }
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

let sharedClient: DaemonClient | null = null;

export function canInvokeTauri(): boolean {
  if (typeof window === "undefined") return false;
  const tauriWindow = window as unknown as {
    __TAURI_INTERNALS__?: unknown;
    __TAURI__?: unknown;
  };
  return Boolean(tauriWindow.__TAURI_INTERNALS__) || Boolean(tauriWindow.__TAURI__);
}

export function configuredBrowserDaemonHandshake(): DaemonHandshake | null {
  if (typeof window === "undefined") return null;

  const params = new URLSearchParams(window.location.search);
  const viteEnv = (import.meta as unknown as { env?: Record<string, string | undefined> }).env;
  const url =
    params.get("corbinaBackend") ||
    params.get("backendUrl") ||
    params.get("backend") ||
    window.localStorage.getItem("corbina.backendUrl") ||
    viteEnv?.VITE_CORBINA_DAEMON_URL ||
    DEFAULT_WS_URL;

  if (!url.startsWith("ws://") && !url.startsWith("wss://")) return null;

  return {
    url,
    token:
      params.get("corbinaToken") ||
      params.get("token") ||
      window.localStorage.getItem("corbina.backendToken") ||
      viteEnv?.VITE_CORBINA_DAEMON_TOKEN ||
      "dev",
    protocolVersion: "1",
    workspaceRoot:
      params.get("workspaceRoot") ||
      window.localStorage.getItem("corbina.workspaceRoot") ||
      ""
  };
}

export function canReachDaemon(): boolean {
  return configuredBrowserDaemonHandshake() !== null || canInvokeTauri() || sharedClient !== null;
}

export async function ensureLocalDaemonClient(): Promise<DaemonClient> {
  if (sharedClient) return sharedClient;
  const handshake = configuredBrowserDaemonHandshake();
  if (handshake) {
    sharedClient = new DaemonClient(handshake);
    await sharedClient.connect();
    return sharedClient;
  }
  if (!canInvokeTauri()) {
    throw new Error("Corbina's Rust backend is only available through the backend WebSocket or inside the Tauri desktop app.");
  }
  sharedClient = new DaemonClient();
  await sharedClient.connect();
  return sharedClient;
}

export async function ensureRemoteDaemonClient(
  url: string,
  token: string
): Promise<DaemonClient> {
  const client = new DaemonClient({
    url,
    token,
    protocolVersion: "1",
    workspaceRoot: ""
  });
  await client.connect();
  return client;
}

export async function switchDaemonClient(handshake: DaemonHandshake): Promise<DaemonClient> {
  sharedClient?.close();
  sharedClient = new DaemonClient(handshake);
  await sharedClient.connect();
  return sharedClient;
}

export function currentDaemonClient(): DaemonClient | null {
  return sharedClient;
}
