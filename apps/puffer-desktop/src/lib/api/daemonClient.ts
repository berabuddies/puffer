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
const DEV_BROWSER_BACKEND_URL = "ws://127.0.0.1:1421/ws";

export class DaemonClient {
  private connectionListeners = new Set<(state: ConnectionState) => void>();
  private eventListeners = new Map<string, Set<(payload: unknown) => void>>();
  private pending = new Map<string, PendingRequest>();
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
    if (!this.useWebSocket) {
      return invoke<T>("backend_request", { method, params });
    }

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
    if (this.useWebSocket) {
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
  const viteEnv = (import.meta as unknown as { env?: Record<string, boolean | string | undefined> }).env;
  const url =
    params.get("pufferBackend") ||
    params.get("corbinaBackend") ||
    params.get("backendUrl") ||
    params.get("backend") ||
    window.localStorage.getItem("puffer.backendUrl") ||
    window.localStorage.getItem("corbina.backendUrl") ||
    stringEnv(viteEnv?.VITE_PUFFER_DAEMON_URL) ||
    stringEnv(viteEnv?.VITE_CORBINA_DAEMON_URL) ||
    devBrowserBackendUrl(viteEnv);

  if (!url || (!url.startsWith("ws://") && !url.startsWith("wss://"))) return null;

  return {
    url,
    token:
      params.get("pufferToken") ||
      params.get("corbinaToken") ||
      params.get("token") ||
      window.localStorage.getItem("puffer.backendToken") ||
      window.localStorage.getItem("corbina.backendToken") ||
      stringEnv(viteEnv?.VITE_PUFFER_DAEMON_TOKEN) ||
      stringEnv(viteEnv?.VITE_CORBINA_DAEMON_TOKEN) ||
      "dev",
    protocolVersion: "1",
    workspaceRoot:
      params.get("workspaceRoot") ||
      window.localStorage.getItem("puffer.workspaceRoot") ||
      window.localStorage.getItem("corbina.workspaceRoot") ||
      ""
  };
}

function stringEnv(value: boolean | string | undefined): string | undefined {
  return typeof value === "string" && value.trim() ? value : undefined;
}

function devBrowserBackendUrl(viteEnv?: Record<string, boolean | string | undefined>): string | null {
  if (canInvokeTauri() || !isViteDev(viteEnv) || !isLocalBrowserPreview()) return null;
  return DEV_BROWSER_BACKEND_URL;
}

function isViteDev(viteEnv?: Record<string, boolean | string | undefined>): boolean {
  return viteEnv?.DEV === true || viteEnv?.MODE === "development";
}

function isLocalBrowserPreview(): boolean {
  const hostname = window.location.hostname;
  return hostname === "localhost" || hostname === "127.0.0.1" || hostname === "::1";
}

export function configuredBrowserRemoteDaemonHandshake(): DaemonHandshake | null {
  if (typeof window === "undefined") return null;

  const params = new URLSearchParams(window.location.search);
  const viteEnv = (import.meta as unknown as { env?: Record<string, string | undefined> }).env;
  const url =
    params.get("pufferRemoteBackend") ||
    params.get("corbinaRemoteBackend") ||
    params.get("remoteBackendUrl") ||
    window.localStorage.getItem("puffer.remoteBackendUrl") ||
    window.localStorage.getItem("corbina.remoteBackendUrl") ||
    viteEnv?.VITE_PUFFER_REMOTE_DAEMON_URL ||
    viteEnv?.VITE_CORBINA_REMOTE_DAEMON_URL;

  if (!url || (!url.startsWith("ws://") && !url.startsWith("wss://"))) return null;

  return {
    url,
    token:
      params.get("pufferRemoteToken") ||
      params.get("corbinaRemoteToken") ||
      params.get("remoteToken") ||
      window.localStorage.getItem("puffer.remoteBackendToken") ||
      window.localStorage.getItem("corbina.remoteBackendToken") ||
      viteEnv?.VITE_PUFFER_REMOTE_DAEMON_TOKEN ||
      viteEnv?.VITE_CORBINA_REMOTE_DAEMON_TOKEN ||
      "dev",
    protocolVersion:
      params.get("pufferRemoteProtocolVersion") ||
      params.get("remoteProtocolVersion") ||
      "1",
    workspaceRoot:
      params.get("pufferRemoteWorkspaceRoot") ||
      params.get("corbinaRemoteWorkspaceRoot") ||
      params.get("remoteWorkspaceRoot") ||
      window.localStorage.getItem("puffer.remoteWorkspaceRoot") ||
      window.localStorage.getItem("corbina.remoteWorkspaceRoot") ||
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
    throw new Error("Puffer's Rust daemon is only available through a configured WebSocket or inside the Tauri desktop app.");
  }
  sharedClient = new DaemonClient(await invoke<DaemonHandshake>("ensure_local_daemon"));
  await sharedClient.connect();
  return sharedClient;
}

function responseErrorMessage(error: string | { message?: string; code?: string }): string {
  if (typeof error === "string") return error;
  const message = error.message?.trim();
  if (message) return message;
  const code = error.code?.trim();
  return code ? `Puffer daemon error: ${code}` : "Puffer daemon request failed.";
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
