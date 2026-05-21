import { createHash } from "node:crypto";

export function sha256(value) {
  return createHash("sha256").update(String(value)).digest("hex");
}

export function stableJson(value) {
  return JSON.stringify(sortValue(value));
}

export function normalizeText(value, options = {}) {
  const limit = Number(options.limit ?? 2000);
  return String(value ?? "")
    .replace(/[0-9a-fA-F]{8}-[0-9a-fA-F-]{27,}/g, "<uuid>")
    .replace(/\b\d{4}-\d{2}-\d{2}\b/g, "<date>")
    .replace(/\b\d{13}\b/g, "<timestamp>")
    .replace(/\b\d+\b/g, "<num>")
    .replace(/\s+/g, " ")
    .trim()
    .slice(0, limit);
}

export function normalizeRoute(value) {
  return String(value ?? "/")
    .replace(/[0-9a-fA-F]{8}-[0-9a-fA-F-]{27,}/g, ":uuid")
    .replace(/\/\d+(?=\/|$)/g, "/:num")
    .replace(/\/session-[^/]+/g, "/:session")
    .replace(/\/turn-[^/]+/g, "/:turn");
}

export function actionFingerprint(action) {
  const core = {
    id: action?.id ?? action?.action ?? "",
    kind: action?.kind ?? "",
    target: action?.target ?? "",
    params: normalizeParams(action?.params ?? {})
  };
  return `act:${sha256(stableJson(core)).slice(0, 16)}`;
}

export function edgeId(beforeStateHash, action, afterStateHash) {
  return `edge:${sha256(`${beforeStateHash}|${actionFingerprint(action)}|${afterStateHash}`).slice(0, 24)}`;
}

export function stateHash(state) {
  const core = {
    appArea: state?.appArea ?? "unknown",
    routePattern: normalizeRoute(state?.routePattern ?? ""),
    modalStack: state?.modalStack ?? [],
    activePanel: state?.activePanel ?? "",
    activeTab: state?.activeTab ?? "",
    focusRegion: state?.focusRegion ?? "",
    daemonState: state?.daemonState ?? "",
    env: state?.env ?? {},
    tree: state?.normalizedTreeSignature ?? "",
    text: state?.normalizedTextSignature ?? "",
    elements: (state?.interactiveElements ?? []).map((item) => [
      item.groupId ?? "",
      item.roleOrKind ?? "",
      normalizeText(item.name ?? "", { limit: 120 })
    ]).sort()
  };
  return `state:${sha256(stableJson(core)).slice(0, 24)}`;
}

export function bugSignature(finding) {
  const evidence = finding?.evidence ?? {};
  const environment = finding?.environment ?? {};
  const core = {
    category: finding?.category ?? "",
    area: finding?.area ?? evidence.appArea ?? "",
    component: evidence.component ?? finding?.component ?? "",
    elementId: evidence.elementId ?? "",
    stateHash: evidence.stateHash ?? "",
    action: finding?.action ?? evidence.actionFingerprint ?? "",
    asyncEvent: finding?.asyncEvent ?? "",
    invariant: finding?.invariant ?? finding?.oracleId ?? "",
    actual: normalizeText(finding?.actual ?? "", { limit: 500 }),
    logs: normalizeText((evidence.logs ?? []).join("\n"), { limit: 500 }),
    viewport: environment.viewport ?? "",
    browser: environment.browser ?? environment.browserOrShell ?? "",
    fakeDaemon: environment.fakeDaemon === true
  };
  return `bug:${sha256(stableJson(core)).slice(0, 24)}`;
}

export function findDuplicateSignatures(candidate, knownSignatures = []) {
  const signature = typeof candidate === "string" ? candidate : bugSignature(candidate);
  return knownSignatures.filter((item) => item === signature);
}

function normalizeParams(value) {
  if (value === null || typeof value !== "object") return normalizeText(value, { limit: 200 });
  if (Array.isArray(value)) return value.map(normalizeParams);
  const result = {};
  for (const [key, item] of Object.entries(value)) {
    result[key] = normalizeParams(item);
  }
  return result;
}

function sortValue(value) {
  if (value === null || typeof value !== "object") return value;
  if (Array.isArray(value)) return value.map(sortValue);
  return Object.fromEntries(
    Object.entries(value)
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([key, item]) => [key, sortValue(item)])
  );
}
