import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import type { Page } from "@playwright/test";

export type PufferCoverageEnv = {
  role?: string;
  viewport: "narrow" | "desktop" | "wide" | string;
  theme?: string;
  locale?: string;
  browserOrShell: string;
  terminalSize?: string;
  fakeDaemon?: boolean;
};

export type PufferInteractiveElement = {
  elementId: string;
  groupId: string;
  roleOrKind: string;
  name: string;
  panel: string;
  component: string;
  locatorHint: string;
  seen: boolean;
  activated?: boolean;
  riskWeight?: number;
};

export type PufferUiState = {
  stateHash: string;
  observedAt: string;
  appArea: string;
  routePattern: string;
  modalStack: string[];
  activePanel: string;
  activeTab: string;
  focusRegion: string;
  daemonState: string;
  normalizedTextSignature: string;
  normalizedTreeSignature: string;
  env: PufferCoverageEnv;
  interactiveElements: PufferInteractiveElement[];
  artifacts?: Record<string, string>;
};

export type PufferTraceEvent = {
  type: "state" | "action" | "console" | "pageerror" | "daemon" | "finding";
  traceId: string;
  step?: number;
  timestamp: string;
  [key: string]: unknown;
};

export function createTraceId(prefix = "puffer-uiux"): string {
  return `${prefix}-${Date.now().toString(36)}-${crypto.randomBytes(4).toString("hex")}`;
}

export function installRuntimeOracle(page: Page, trace: PufferTraceEvent[], traceId: string): void {
  page.on("console", (message) => {
    if (message.type() !== "error") return;
    trace.push({
      type: "console",
      traceId,
      timestamp: new Date().toISOString(),
      level: message.type(),
      text: message.text()
    });
  });
  page.on("pageerror", (error) => {
    trace.push({
      type: "pageerror",
      traceId,
      timestamp: new Date().toISOString(),
      text: error.message,
      stack: error.stack
    });
  });
}

export async function collectPufferUiState(page: Page, env: PufferCoverageEnv): Promise<PufferUiState> {
  const observed = await page.evaluate(() => {
    function visibleText(element: Element): string {
      const html = element as HTMLElement;
      return (
        element.getAttribute("aria-label") ||
        element.getAttribute("title") ||
        element.getAttribute("placeholder") ||
        html.innerText ||
        element.textContent ||
        ""
      ).trim().replace(/\s+/g, " ").slice(0, 140);
    }

    function panelName(element: Element): string {
      const panel = element.closest("[class*='pf-']");
      if (!panel) return "unknown";
      const className = String((panel as HTMLElement).className || "");
      const match = className.match(/\bpf-[a-z0-9-]+/);
      return match?.[0] ?? "unknown";
    }

    function cssPath(element: Element): string {
      const parts: string[] = [];
      let current: Element | null = element;
      while (current && current.nodeType === Node.ELEMENT_NODE && parts.length < 6) {
        let part = current.tagName.toLowerCase();
        const testId = current.getAttribute("data-testid") || current.getAttribute("data-test") || current.id;
        if (testId) part += `[id="${testId}"]`;
        parts.unshift(part);
        current = current.parentElement;
      }
      return parts.join(">");
    }

    const selector = [
      "a[href]",
      "button",
      "input",
      "select",
      "textarea",
      "[role]",
      "[tabindex]:not([tabindex='-1'])",
      "[onclick]",
      "[data-testid]",
      ".pf-agent-tabs button",
      ".pf-browser-tab",
      ".pf-sidebar-agents-list button"
    ].join(",");

    const elements = Array.from(document.querySelectorAll(selector))
      .filter((element) => {
        const html = element as HTMLElement;
        const style = window.getComputedStyle(html);
        const rect = html.getBoundingClientRect();
        return style.visibility !== "hidden" && style.display !== "none" && rect.width > 0 && rect.height > 0;
      })
      .map((element) => {
        const role = element.getAttribute("role") || element.tagName.toLowerCase();
        const name = visibleText(element);
        const panel = panelName(element);
        const testId = element.getAttribute("data-testid") || element.getAttribute("data-test") || "";
        const locatorHint = testId ? `[data-testid="${testId}"]` : cssPath(element);
        const groupBase = testId || `${panel}:${role}:${name}`;
        return {
          roleOrKind: role,
          name,
          panel,
          component: panel,
          locatorHint,
          elementId: `${location.pathname}::${groupBase}::${cssPath(element)}`,
          groupId: `${location.pathname}::${groupBase}`,
          seen: true
        };
      });

    const activeElement = document.activeElement;
    const activePanel = activeElement ? panelName(activeElement) : "unknown";
    const dialogs = Array.from(document.querySelectorAll("[role='dialog'], dialog, .pf-modal, .modal"))
      .filter((element) => {
        const rect = (element as HTMLElement).getBoundingClientRect();
        return rect.width > 0 && rect.height > 0;
      })
      .map((element) => visibleText(element).slice(0, 80) || panelName(element));
    const bodyText = (document.body?.innerText || "").replace(/\s+/g, " ").trim().slice(0, 4000);
    const activeTab = Array.from(document.querySelectorAll("[aria-selected='true'], [data-active='true'], .active"))
      .map((element) => visibleText(element))
      .find(Boolean) || "";
    const daemonText = bodyText.includes("not connected") || bodyText.includes("Disconnected")
      ? "disconnected"
      : bodyText.includes("Connecting")
        ? "reconnecting"
        : "connected-or-unknown";

    return {
      routePattern: location.pathname || "/",
      appArea: activePanel,
      activePanel,
      activeTab,
      focusRegion: activePanel,
      modalStack: dialogs,
      daemonState: daemonText,
      bodyText,
      elements
    };
  });

  const normalizedTextSignature = sha256(normalizeText(observed.bodyText));
  const normalizedTreeSignature = sha256(JSON.stringify(
    observed.elements.map((element) => [element.groupId, element.roleOrKind, normalizeText(element.name)]).sort()
  ));
  const stateWithoutHash = {
    observedAt: new Date().toISOString(),
    appArea: observed.appArea,
    routePattern: normalizeRoute(observed.routePattern),
    modalStack: observed.modalStack,
    activePanel: observed.activePanel,
    activeTab: normalizeText(observed.activeTab),
    focusRegion: observed.focusRegion,
    daemonState: observed.daemonState,
    normalizedTextSignature,
    normalizedTreeSignature,
    env,
    interactiveElements: observed.elements
  };
  return {
    ...stateWithoutHash,
    stateHash: `state:${sha256(JSON.stringify(stateWithoutHash)).slice(0, 24)}`
  };
}

export function appendTraceEvent(trace: PufferTraceEvent[], event: Omit<PufferTraceEvent, "timestamp">): void {
  trace.push({ ...event, timestamp: new Date().toISOString() });
}

export function writeTraceJsonl(filePath: string, trace: PufferTraceEvent[]): void {
  fs.mkdirSync(path.dirname(filePath), { recursive: true });
  fs.writeFileSync(filePath, `${trace.map((event) => JSON.stringify(event)).join("\n")}\n`);
}

function sha256(value: string): string {
  return crypto.createHash("sha256").update(value).digest("hex");
}

function normalizeText(value: string): string {
  return String(value ?? "")
    .replace(/[0-9a-fA-F]{8}-[0-9a-fA-F-]{27,}/g, "<uuid>")
    .replace(/\b\d{4}-\d{2}-\d{2}\b/g, "<date>")
    .replace(/\b\d{13}\b/g, "<timestamp>")
    .replace(/\b\d+\b/g, "<num>")
    .replace(/\s+/g, " ")
    .trim()
    .slice(0, 2000);
}

function normalizeRoute(value: string): string {
  return String(value || "/")
    .replace(/[0-9a-fA-F]{8}-[0-9a-fA-F-]{27,}/g, ":uuid")
    .replace(/\/\d+(?=\/|$)/g, "/:num");
}
