import { expect, test } from "@playwright/test";
import {
  proxyStatusLabel,
  proxyStatusState,
  proxyStatusTitle
} from "../src/lib/screens/settings/proxyStatus";

test("proxy status label shows connected latency", () => {
  const result = {
    proxyId: "local",
    ok: true,
    message: "Connected to https://www.gstatic.com/generate_204 with HTTP 204",
    latencyMs: 848,
    statusCode: 204
  };

  expect(proxyStatusLabel("local", null, result)).toBe("connected (ping: 848 ms)");
  expect(proxyStatusState("local", null, result)).toBe("connected");
  expect(proxyStatusTitle("local", result)).toBe(result.message);
});

test("proxy status label includes actionable failure reasons", () => {
  expect(proxyStatusLabel("local", null, {
    proxyId: "local",
    ok: false,
    message: "operation timed out",
    latencyMs: null,
    statusCode: null
  })).toBe("failed (timeout)");
  expect(proxyStatusLabel("local", null, {
    proxyId: "local",
    ok: false,
    message: "client error: connection refused",
    latencyMs: null,
    statusCode: null
  })).toBe("failed (connection refused)");
});
