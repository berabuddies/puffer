import type { AgentState } from "../shell/tweaks.ts";

export type AgentStatus = "running" | "awaiting" | "review" | "idle";

export type MockProject = {
  id: string;
  name: string;
  path: string;
  branch: string;
  remote: string;
  color: string;
  remoteHost?: boolean;
};

export type MockAgent = {
  id: string;
  project: string;
  name: string;
  title: string;
  worktree: string;
  branch: string;
  status: AgentStatus;
  progress: number;
  step: string;
  tools: number;
  elapsed: string;
  model: string;
};

export type MockTask = {
  id: string;
  project: string;
  title: string;
  status: "queued" | "done" | "archived";
  priority?: "normal" | "high";
  author?: string;
  elapsed?: string;
  when?: string;
};

export type RemoteHost = {
  id: string;
  label: string;
  host: string;
  user: string;
  status: "online" | "offline";
  latency: string;
};

export const PROJECTS: MockProject[] = [
  {
    id: "api",  name: "stripe-api",  path: "~/src/stripe-api",   branch: "main",
    remote: "github.com/acme/stripe-api",
    color: "oklch(0.68 0.17 20)"
  },
  {
    id: "web",  name: "puffer-web",  path: "~/src/puffer-web",   branch: "feat/onboard",
    remote: "github.com/acme/puffer-web",
    color: "oklch(0.68 0.16 260)"
  },
  {
    id: "infra", name: "infra-tf",   path: "~/infra",             branch: "main",
    remote: "github.com/acme/infra-tf",
    color: "oklch(0.68 0.15 150)"
  },
  {
    id: "docs", name: "docs-site",   path: "remote: ssh://build-01", branch: "main",
    remote: "github.com/acme/docs-site",
    color: "oklch(0.7 0.13 60)",
    remoteHost: true
  }
];

export const AGENTS: MockAgent[] = [
  { id: "a-sage-1", project: "api", name: "Sage", title: "Fix proration in webhook", worktree: "wt/proration", branch: "fix/webhook-proration", status: "running",  progress: 62, step: "Writing tests for unused_time branch", tools: 14, elapsed: "2m 14s", model: "sonnet" },
  { id: "a-mink-1", project: "api", name: "Mink", title: "Rewrite billing tests",    worktree: "wt/bill-tests", branch: "test/billing-suite",   status: "review",   progress: 100, step: "12 files changed · 4 commits", tools: 21, elapsed: "18m", model: "haiku" },
  { id: "a-flint-1", project: "web", name: "Flint",  title: "Add dark-mode toggle", worktree: "wt/dark-mode", branch: "feat/dark-mode", status: "running",  progress: 48, step: "Patching settings/theme.tsx", tools: 8, elapsed: "1m 05s", model: "sonnet" },
  { id: "a-juno-1",  project: "web", name: "Juno",   title: "Generate API types",  worktree: "wt/gen-types", branch: "chore/gen-types", status: "running",  progress: 82, step: "Validating against OpenAPI spec", tools: 9, elapsed: "41s", model: "haiku" },
  { id: "a-harbor-1", project: "web", name: "Harbor", title: "Migrate to passkeys", worktree: "wt/passkeys", branch: "feat/passkeys",  status: "awaiting", progress: 55, step: "Waiting on approval: enable webauthn origin", tools: 6, elapsed: "4m", model: "sonnet" },
  { id: "a-willow-1", project: "infra", name: "Willow", title: "Bump terraform aws", worktree: "wt/tf-bump", branch: "chore/tf-bump", status: "review", progress: 100, step: "Ready for review · 3 resources drifted", tools: 12, elapsed: "6m", model: "sonnet" }
];

export const EXTRA_TASKS: MockTask[] = [
  { project: "api", id: "eq-1", title: "Migrate billing tests from jest to vitest", status: "queued",   priority: "normal", author: "@harvey" },
  { project: "api", id: "eq-2", title: "Add idempotency keys to invoice.create",    status: "queued",   priority: "high",   author: "@lin" },
  { project: "api", id: "ed-1", title: "Add retry on Stripe webhooks",               status: "done",     author: "Sage",    elapsed: "3m 41s",  when: "yesterday" },
  { project: "api", id: "ed-2", title: "Fix 500 on expired idem-key replay",         status: "done",     author: "Atlas",   elapsed: "58s",     when: "yesterday" },
  { project: "api", id: "ed-3", title: "Add coupon stacking support",                 status: "done",     author: "Lumen",   elapsed: "4m 31s",  when: "2h ago" },
  { project: "web", id: "eqw-1", title: "Investigate flaky checkout e2e spec",      status: "queued",   priority: "normal", author: "@harvey" },
  { project: "web", id: "edw-1", title: "Wire telemetry for pricing page",           status: "done",     author: "Lumen",    elapsed: "12m",   when: "yesterday" },
  { project: "web", id: "edw-2", title: "Add skeleton loaders to dashboard",         status: "done",     author: "Flint",    elapsed: "8m",    when: "yesterday" },
  { project: "infra", id: "edi-1", title: "Add CloudWatch alarm for 4xx spike",     status: "done",     author: "Sage",     elapsed: "5m 30s", when: "yesterday" },
  { project: "infra", id: "edi-2", title: "Rotate shared VPN certs",                status: "done",     author: "Willow",   elapsed: "2m",    when: "3d ago" },
  { project: "docs", id: "eqd-1", title: "Update API reference for v3",             status: "queued",   priority: "normal", author: "@harvey" }
];

export const REMOTE_HOSTS: RemoteHost[] = [
  { id: "h1", label: "build-01",   host: "ssh://build-01.internal",  user: "puffer", status: "online",  latency: "12ms" },
  { id: "h2", label: "sandbox-eu", host: "ssh://sbx-eu.puffer.dev",  user: "agent",  status: "online",  latency: "74ms" },
  { id: "h3", label: "gpu-03",     host: "ssh://gpu-03.cluster",     user: "ml",     status: "offline", latency: "—" }
];

export const AGENT_STATE_LABELS: Record<string, string> = {
  running: "Running",
  awaiting: "Awaiting approval",
  review: "Ready to review",
  idle: "Idle"
};

export function agentPufferState(status: AgentStatus): AgentState {
  switch (status) {
    case "running": return "running";
    case "awaiting": return "awaiting";
    case "review": return "review";
    default: return "idle";
  }
}
