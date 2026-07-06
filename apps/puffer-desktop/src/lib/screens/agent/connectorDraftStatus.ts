// Pure mapping helpers for the connector-draft (unified outbound gate) card.
// Kept out of ToolCard.svelte so the status/error routing can be unit tested.

export type ConnectorDraftSendState =
  | "idle"
  | "sending"
  | "cancelling"
  | "sent"
  | "cancelled"
  | "expired"
  | "uncertain"
  | "error";

/** Warning shown when an action is in duplicate-risk / crash limbo. */
export const UNCERTAIN_SEND_MESSAGE =
  "Send status is uncertain. Check Telegram before retrying.";

export type ConnectorDraftStatusResult = {
  state: ConnectorDraftSendState;
  error: string;
};

function errorText(value: unknown): string {
  if (value === null || value === undefined) return "";
  if (typeof value === "string") return value;
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}

// Persisted outbound-action statuses (returned verbatim from the store):
// draft_ready, sending, sent, failed, cancelled, expired, uncertain.
/** Map a persisted outbound-action status string to a card send state. */
export function connectorDraftStateForStatus(
  status: string,
  error: unknown = null
): ConnectorDraftStatusResult {
  switch (status) {
    case "sent":
      return { state: "sent", error: "" };
    case "cancelled":
      return { state: "cancelled", error: "" };
    case "expired":
      return { state: "expired", error: "" };
    case "sending":
      return { state: "sending", error: "" };
    case "uncertain":
      return { state: "uncertain", error: UNCERTAIN_SEND_MESSAGE };
    case "failed":
      return { state: "error", error: errorText(error) || "Send failed." };
    default:
      return { state: "idle", error: "" };
  }
}

export type OutboundSendErrorClass = ConnectorDraftStatusResult & {
  /** Whether the caller should re-fetch status once and re-render from truth. */
  refresh: boolean;
};

// Rejection sentinels thrown by outbound_action_execute.
/** Route a thrown execute-rejection message to a card send state. */
export function classifyOutboundSendError(message: string): OutboundSendErrorClass {
  if (message.includes("outbound_action_expired")) {
    return { state: "expired", error: "", refresh: false };
  }
  if (message.includes("terminal_outbound_action")) {
    return { state: "cancelled", error: "", refresh: false };
  }
  if (message.includes("duplicate_risk_ack_required")) {
    return { state: "uncertain", error: UNCERTAIN_SEND_MESSAGE, refresh: false };
  }
  if (message.includes("outbound_action_version_mismatch")) {
    return { state: "error", error: "Draft changed since it was shown. Refreshing…", refresh: true };
  }
  return { state: "error", error: message, refresh: false };
}
