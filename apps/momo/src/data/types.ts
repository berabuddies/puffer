/**
 * Shared TypeScript types for v2 mock data. Page agents should extend these
 * types in this file rather than introducing one-off shapes in their pages.
 */

export interface User {
  name: string;
  credits: number;
  avatarLabel: string;
  email: string;
}

/** Lucide icon name (kebab-case). Pages resolve to the actual component. */
export type IconName =
  | "calendar"
  | "message-circle"
  | "cake"
  | "shopping-bag"
  | "mail"
  | "phone"
  | "wallet"
  | "credit-card"
  | "user"
  | "users"
  | "briefcase"
  | "bot"
  | "check"
  | "circle-alert";

export interface TaskAction {
  /** Visible label. */
  label: string;
  /** Visual tone — `cream` is the primary, `neutral` the secondary. */
  tone: "cream" | "neutral";
  /** Optional route to navigate to (e.g. `/agent/calendar`). */
  navigateTo?: string;
}

export interface Task {
  id: string;
  /** Lucide icon name shown in the 40px round icon block. */
  icon: IconName;
  /** Card title (15px medium). */
  title: string;
  /** Secondary body (13px regular). May include people / time / context. */
  meta: string;
  /** Optional source app glyph asset path (rendered as a small chip). */
  app?: string;
  primaryAction: TaskAction;
  /** Secondary action — usually a neutral 'Ignore' style. */
  secondaryAction?: TaskAction;
}

export interface TaskGroup {
  label: "Work" | "Life";
  count: number;
  tasks: Task[];
}

export interface Contact {
  id: string;
  name: string;
  role: string;
  email: string;
  /** Single uppercase letter for the avatar placeholder. */
  avatarLabel: string;
  /** Hint for avatar tint — page agent decides how to render. */
  colorHint: "warm" | "cool" | "neutral";
  /** Organization shown on the detail view next to role ("Product Lead · WorldAgent"). */
  org?: string;
  /** Activity recency string used on the detail view ("Active 12 min ago"). */
  lastActive?: string;
  /** Detail view: next scheduled meeting summary ("Product sync · Tue 10:00"). */
  nextMeeting?: string;
  /** Detail view: most recent touch point summary ("Lark message · Yesterday"). */
  lastTouch?: string;
}

export interface AppEntry {
  id: string;
  name: string;
  /** "Connected" or "Not connected". */
  status: "connected" | "not_connected";
  /** Asset path for the logo (page agent may swap with a vector). */
  logo: string;
}

export interface WalletActivity {
  id: string;
  /** ISO date or display string. */
  date: string;
  merchant: string;
  amount: string;
  category: string;
  /** Direction — "in" for top-ups / deposits, "out" for outgoing spend. */
  direction?: "in" | "out";
  /** Optional second-line description shown beneath the merchant. */
  description?: string;
  /** Optional display-friendly time/day stamp shown beneath the amount. */
  time?: string;
}

export interface WalletSnapshot {
  balance: string;
  monthlySpend: string;
  pendingHolds: string;
  kycStatus: "Verified" | "Pending" | "Not started";
  recentActivity: WalletActivity[];
}

/* ───── Agent conversation timeline ───────────────────────────── */

/** A single bubble or block in an agent conversation. */
export type ConversationStep =
  | UserMessageStep
  | AgentMessageStep
  | ToolStep
  | OptionsStep
  | ResultStep;

export interface UserMessageStep {
  kind: "user";
  text: string;
}

export interface AgentMessageStep {
  kind: "agent";
  text: string;
}

export interface ToolStep {
  kind: "tool";
  /** Lucide icon for the tool pill. */
  icon: IconName;
  /** Short status string shown inside the pill. */
  label: string;
}

export interface OptionsStep {
  kind: "options";
  /** Lead-in line ("I found 3 good morning slots."). */
  intro: string;
  options: ConversationOption[];
  /** Optional trailing footnote ("Suggested agenda: ..."). */
  footnote?: string;
}

export interface ConversationOption {
  primary: string;
  trailing: string;
  /** Whether this row is the highlighted "best fit" recommendation. */
  highlighted: boolean;
  /** Optional badge label shown on the right (e.g. "Best fit"). */
  badge?: string;
}

export interface ResultStep {
  kind: "result";
  /** Header line like "Event created in Google Calendar". */
  title: string;
  /** Sub-line like "Invites sent · Conferencing link added". */
  subtitle: string;
  /** Inner card body — a small detail block. */
  detail: ResultDetail;
  actions: TaskAction[];
}

export interface ResultDetail {
  title: string;
  facts: string[];
  notes?: string;
}

export interface AgentConversation {
  /** Slug used in the URL (`/agent/<slug>`). */
  slug: string;
  /** Display title at the top of the page. */
  title: string;
  /** Composer placeholder for this conversation. */
  composerPlaceholder: string;
  steps: ConversationStep[];
}
