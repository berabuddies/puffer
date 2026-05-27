/**
 * Domain wrappers around `wsClient` for the connector-setup RPCs in
 * `src-tauri/src/connectors.rs`. Each Telegram step targets a long-lived
 * `puffer connect telegram` child process keyed by `sessionId`; Email is
 * a one-shot `puffer connect email configure`.
 *
 * Used by the Connected Apps page modals (Telegram + Email).
 */

import * as ws from "./wsClient";

export type ConnectorStatus =
  | "awaiting_code"
  | "awaiting_password"
  | "complete"
  | "failed"
  | "unknown";

export interface ConnectorEvent {
  topic: string;
  kind: string;
  control?: boolean;
  payload?: Record<string, unknown>;
}

export interface TelegramSessionResult {
  sessionId: string;
  connectionSlug: string;
  status: ConnectorStatus;
  event: ConnectorEvent;
}

export interface TelegramStartParams {
  phone: string;
  /** Override the default Telegram connection slug (`telegram-user`). */
  connectionSlug?: string;
}

export interface TelegramSubmitCodeParams {
  sessionId: string;
  code: string;
}

export interface TelegramSubmitPasswordParams {
  sessionId: string;
  password: string;
}

export interface EmailConfigureParams {
  imapHost: string;
  smtpHost: string;
  username: string;
  password: string;
  fromAddress: string;
  /** `0` (or omit) lets the subscriber default the port (993 / 587). */
  imapPort?: number;
  smtpPort?: number;
}

export interface EmailConfigureResult {
  status: "configured";
  output: string;
}

export function telegramStart(
  params: TelegramStartParams,
): Promise<TelegramSessionResult> {
  return ws.request<TelegramSessionResult>(
    "telegram_start",
    stripUndefined(params as unknown as Record<string, unknown>),
  );
}

export function telegramSubmitCode(
  params: TelegramSubmitCodeParams,
): Promise<TelegramSessionResult> {
  return ws.request<TelegramSessionResult>(
    "telegram_submit_code",
    stripUndefined(params as unknown as Record<string, unknown>),
  );
}

export function telegramSubmitPassword(
  params: TelegramSubmitPasswordParams,
): Promise<TelegramSessionResult> {
  return ws.request<TelegramSessionResult>(
    "telegram_submit_password",
    stripUndefined(params as unknown as Record<string, unknown>),
  );
}

export function telegramCancel(sessionId: string): Promise<unknown> {
  return ws.request<unknown>("telegram_cancel", { sessionId });
}

export function emailConfigure(params: EmailConfigureParams): Promise<EmailConfigureResult> {
  return ws.request<EmailConfigureResult>(
    "email_configure",
    stripUndefined(params as unknown as Record<string, unknown>),
  );
}

export interface ConnectorStatusSnapshot {
  telegram: boolean;
  email: boolean;
}

export function getConnectorStatus(
  connectionSlug?: string,
): Promise<ConnectorStatusSnapshot> {
  const params: Record<string, unknown> = {};
  if (connectionSlug) params.connectionSlug = connectionSlug;
  return ws.request<ConnectorStatusSnapshot>("connector_status", params);
}

export type ConnectorKind = "telegram" | "email";

export function connectorDisconnect(
  kind: ConnectorKind,
  connectionSlug?: string,
): Promise<unknown> {
  const params: Record<string, unknown> = { kind };
  if (connectionSlug) params.connectionSlug = connectionSlug;
  return ws.request<unknown>("connector_disconnect", params);
}

function stripUndefined(params: Record<string, unknown>): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(params)) {
    if (value === undefined || value === null) continue;
    out[key] = value;
  }
  return out;
}
