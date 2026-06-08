export type ScreenId =
  | "workspace"
  | "workflows"
  | "tasks"
  | "settings"
  | "telegram-relationships";
export type AgentState = "idle" | "thinking" | "running" | "awaiting" | "review";
export type AccentKey = "violet" | "cyan" | "amber" | "rose" | "lime" | "mono";
export type DensityKey = "compact" | "comfortable" | "airy";
export type ThemeKey = "light" | "dark";
export type FontMixKey = "sans-mono" | "all-mono";

export type Tweaks = {
  screen: ScreenId;
  theme: ThemeKey;
  accent: AccentKey;
  density: DensityKey;
  fontMix: FontMixKey;
  userName: string;
  showSidebar: boolean;
  collapsedSidebar: boolean;
  sidebarWidth: number;
  agentState: AgentState;
};

export const SIDEBAR_MIN_WIDTH = 220;
export const SIDEBAR_DEFAULT_WIDTH = 248;
export const SIDEBAR_MAX_WIDTH = 420;

export const defaultTweaks: Tweaks = {
  screen: "workspace",
  theme: "light",
  accent: "violet",
  density: "comfortable",
  fontMix: "sans-mono",
  userName: "Otter",
  showSidebar: true,
  collapsedSidebar: false,
  sidebarWidth: SIDEBAR_DEFAULT_WIDTH,
  agentState: "running"
};

const STORAGE_KEY = "puffer-desktop:tweaks";

export function loadTweaks(): Tweaks {
  if (typeof window === "undefined") return { ...defaultTweaks };
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return { ...defaultTweaks };
    const loaded = { ...defaultTweaks, ...JSON.parse(raw) };
    return {
      ...loaded,
      screen: normalizeScreen(loaded.screen),
      sidebarWidth: clampSidebarWidth(loaded.sidebarWidth)
    };
  } catch {
    return { ...defaultTweaks };
  }
}

function normalizeScreen(value: unknown): ScreenId {
  if (value === "pipelines") return "workflows";
  return value === "workspace" ||
    value === "workflows" ||
    value === "tasks" ||
    value === "settings" ||
    value === "telegram-relationships"
    ? value
    : defaultTweaks.screen;
}

export function clampSidebarWidth(value: unknown): number {
  const numeric = typeof value === "number" && Number.isFinite(value) ? value : SIDEBAR_DEFAULT_WIDTH;
  return Math.min(SIDEBAR_MAX_WIDTH, Math.max(SIDEBAR_MIN_WIDTH, Math.round(numeric)));
}

export function persistTweaks(tweaks: Tweaks) {
  if (typeof window === "undefined") return;
  window.localStorage.setItem(STORAGE_KEY, JSON.stringify(tweaks));
}

export function applyTweaksToDocument(tweaks: Tweaks) {
  if (typeof document === "undefined") return;
  const root = document.documentElement;
  root.classList.toggle("dark", tweaks.theme === "dark");
  root.dataset.accent = tweaks.accent;
  root.dataset.density = tweaks.density;
  root.dataset.fontmix = tweaks.fontMix;
}
