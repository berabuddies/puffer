import { expect, test } from "vitest";
import {
  classifyOutboundSendError,
  connectorDraftStateForStatus,
  UNCERTAIN_SEND_MESSAGE
} from "./connectorDraftStatus";

test("maps terminal and in-flight statuses to their send states", () => {
  expect(connectorDraftStateForStatus("sent")).toEqual({ state: "sent", error: "" });
  expect(connectorDraftStateForStatus("cancelled")).toEqual({ state: "cancelled", error: "" });
  expect(connectorDraftStateForStatus("expired")).toEqual({ state: "expired", error: "" });
  expect(connectorDraftStateForStatus("sending")).toEqual({ state: "sending", error: "" });
});

test("maps draft_ready (and unknown statuses) to idle", () => {
  expect(connectorDraftStateForStatus("draft_ready")).toEqual({ state: "idle", error: "" });
  expect(connectorDraftStateForStatus("whatever-else")).toEqual({ state: "idle", error: "" });
});

test("maps the uncertain status to the warning state (not an idle enabled button)", () => {
  expect(connectorDraftStateForStatus("uncertain")).toEqual({
    state: "uncertain",
    error: UNCERTAIN_SEND_MESSAGE
  });
});

test("maps failed with an error payload, falling back to a default message", () => {
  expect(connectorDraftStateForStatus("failed", "boom")).toEqual({
    state: "error",
    error: "boom"
  });
  expect(connectorDraftStateForStatus("failed")).toEqual({
    state: "error",
    error: "Send failed."
  });
});

test("routes expired execute rejection to the expired terminal state", () => {
  expect(classifyOutboundSendError("rejected: outbound_action_expired")).toEqual({
    state: "expired",
    error: "",
    refresh: false
  });
});

test("routes terminal execute rejection to the cancelled terminal state", () => {
  expect(classifyOutboundSendError("terminal_outbound_action: already sent")).toEqual({
    state: "cancelled",
    error: "",
    refresh: false
  });
});

test("routes duplicate-risk rejection to the uncertain warning state", () => {
  expect(classifyOutboundSendError("duplicate_risk_ack_required")).toEqual({
    state: "uncertain",
    error: UNCERTAIN_SEND_MESSAGE,
    refresh: false
  });
});

test("routes version mismatch to a refresh-from-truth state", () => {
  const routed = classifyOutboundSendError("outbound_action_version_mismatch");
  expect(routed.state).toBe("error");
  expect(routed.refresh).toBe(true);
});

test("keeps unrecognized rejections generic", () => {
  expect(classifyOutboundSendError("some network blip")).toEqual({
    state: "error",
    error: "some network blip",
    refresh: false
  });
});
