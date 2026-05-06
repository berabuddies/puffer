export type ScreenId = "workspace" | "pipelines" | "deployments" | "settings";
export type AgentState = "idle" | "thinking" | "running" | "awaiting";
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
  agentState: AgentState;
};

export const defaultTweaks: Tweaks = {
  screen: "workspace",
  theme: "light",
  accent: "violet",
  density: "comfortable",
  fontMix: "sans-mono",
  userName: "Otter",
  showSidebar: true,
  collapsedSidebar: false,
  agentState: "running"
};

const STORAGE_KEY = "puffer-desktop:tweaks";

export function loadTweaks(): Tweaks {
  if (typeof window === "undefined") return { ...defaultTweaks };
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return { ...defaultTweaks };
    return { ...defaultTweaks, ...JSON.parse(raw) };
  } catch {
    return { ...defaultTweaks };
  }
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
