import type { ProxyTestResult } from "../../types";

export type ProxyStatusState = "checking" | "connected" | "failed" | "unknown";

export function proxyStatusLabel(
  proxyId: string,
  testingId: string | null,
  lastTest: ProxyTestResult | null
): string | null {
  if (testingId === proxyId) return "checking...";
  if (lastTest?.proxyId !== proxyId) return null;
  if (lastTest.ok) {
    return lastTest.latencyMs === null
      ? "connected"
      : `connected (ping: ${lastTest.latencyMs} ms)`;
  }
  return failureLabel(lastTest.message);
}

export function proxyStatusState(
  proxyId: string,
  testingId: string | null,
  lastTest: ProxyTestResult | null
): ProxyStatusState {
  if (testingId === proxyId) return "checking";
  if (lastTest?.proxyId !== proxyId) return "unknown";
  return lastTest.ok ? "connected" : "failed";
}

export function proxyStatusTitle(
  proxyId: string,
  lastTest: ProxyTestResult | null
): string {
  return lastTest?.proxyId === proxyId ? lastTest.message : "";
}

function failureLabel(message: string): string {
  const normalized = message.toLowerCase();
  if (normalized.includes("timed out") || normalized.includes("timeout")) {
    return "failed (timeout)";
  }
  if (normalized.includes("connection refused")) {
    return "failed (connection refused)";
  }
  if (normalized.includes("socks")) {
    return "failed (SOCKS handshake failed)";
  }
  if (normalized.includes("tls") || normalized.includes("certificate")) {
    return "failed (TLS error)";
  }
  return "failed";
}
