/**
 * Declarative route table. Order matters for matchRoute — more specific
 * patterns must come before generic ones (e.g. /home/empty before /home).
 *
 * Each entry:
 *   - pattern: app path with optional ':name' params
 *   - component: the Svelte component to render
 *   - hasShell: true for in-app pages (Sidebar + Page column),
 *               false for full-bleed flows (onboarding)
 *   - displayName: short label, useful for debugging / docTitle
 */

import type { Component } from "svelte";

import Home from "./pages/Home.svelte";
import HomeEmpty from "./pages/HomeEmpty.svelte";
import Contact from "./pages/Contact.svelte";
import ContactDetail from "./pages/ContactDetail.svelte";
import Wallet from "./pages/Wallet.svelte";
import WalletKyc from "./pages/WalletKyc.svelte";
import ConnectedApps from "./pages/ConnectedApps.svelte";
import Agent from "./pages/Agent.svelte";
import OnboardingWhere from "./pages/onboarding/Where.svelte";
import OnboardingRole from "./pages/onboarding/Role.svelte";
import OnboardingApps from "./pages/onboarding/Apps.svelte";
import OnboardingDone from "./pages/onboarding/Done.svelte";

export interface RouteDef {
  pattern: string;
  component: Component<Record<string, unknown>>;
  hasShell: boolean;
  /**
   * When true, the page takes over the Shell's content column entirely:
   * no top padding (the page draws its own header beneath the traffic
   * lights) and the column is locked to the viewport height with no
   * scrolling — the page is expected to flex-layout its own scroll
   * region (e.g. Agent's thread between a sticky header and composer).
   *
   * Default false: the column gets the standard top padding for traffic
   * lights and is allowed to scroll when content overflows.
   */
  fullbleed?: boolean;
  displayName: string;
}

export const routes: readonly RouteDef[] = [
  // Order: most specific first (longer paths before shorter prefixes).
  { pattern: "/home/empty", component: HomeEmpty as Component<Record<string, unknown>>, hasShell: true, displayName: "Home (empty)" },
  { pattern: "/home", component: Home as Component<Record<string, unknown>>, hasShell: true, displayName: "Home" },

  { pattern: "/contact/:id", component: ContactDetail as Component<Record<string, unknown>>, hasShell: true, displayName: "Contact detail" },
  { pattern: "/contact", component: Contact as Component<Record<string, unknown>>, hasShell: true, displayName: "Contact" },

  { pattern: "/wallet/kyc", component: WalletKyc as Component<Record<string, unknown>>, hasShell: true, displayName: "Wallet · KYC" },
  { pattern: "/wallet", component: Wallet as Component<Record<string, unknown>>, hasShell: true, displayName: "Wallet" },

  { pattern: "/apps", component: ConnectedApps as Component<Record<string, unknown>>, hasShell: true, displayName: "Connected Apps" },

  { pattern: "/agent/:taskId", component: Agent as Component<Record<string, unknown>>, hasShell: true, fullbleed: true, displayName: "Agent" },

  { pattern: "/onboarding/where", component: OnboardingWhere as Component<Record<string, unknown>>, hasShell: false, displayName: "Onboarding · Where" },
  { pattern: "/onboarding/role", component: OnboardingRole as Component<Record<string, unknown>>, hasShell: false, displayName: "Onboarding · Role" },
  { pattern: "/onboarding/apps", component: OnboardingApps as Component<Record<string, unknown>>, hasShell: false, displayName: "Onboarding · Apps" },
  { pattern: "/onboarding/done", component: OnboardingDone as Component<Record<string, unknown>>, hasShell: false, displayName: "Onboarding · Done" }
];

/**
 * '/' redirects depending on whether the user has finished onboarding.
 *
 * App.svelte calls this on mount and again whenever it lands on '/'. We keep
 * the static `ROOT_REDIRECT` export so existing callers (and any unit test
 * that imports it for a fixed default) keep compiling — it always points to
 * the main app entry. New callers should prefer `getRootRedirect()` so the
 * decision honours the `puffer.onboarded` localStorage flag.
 */
import { isOnboarded } from "./lib/auth.svelte";

export const ROOT_REDIRECT = "/home";

export function getRootRedirect(): string {
  return isOnboarded() ? "/home" : "/onboarding/where";
}
