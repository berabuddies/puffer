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
import WalletKycForm from "./pages/WalletKycForm.svelte";
import WalletKycVerify from "./pages/WalletKycVerify.svelte";
import ConnectedApps from "./pages/ConnectedApps.svelte";
import Agent from "./pages/Agent.svelte";
import NewChat from "./pages/NewChat.svelte";
import Login from "./pages/Login.svelte";
import AuthCallback from "./pages/AuthCallback.svelte";
import OnboardingWhere from "./pages/onboarding/Where.svelte";
import OnboardingRole from "./pages/onboarding/Role.svelte";
import OnboardingApps from "./pages/onboarding/Apps.svelte";
import OnboardingDone from "./pages/onboarding/Done.svelte";
import ChatGallery from "./pages/dev/ChatGallery.svelte";

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
  // Auth routes live at the top — they need to resolve even when the
  // user is signed out, before any of the gated app routes.
  { pattern: "/login", component: Login as Component<Record<string, unknown>>, hasShell: false, displayName: "Login" },
  { pattern: "/auth/callback", component: AuthCallback as Component<Record<string, unknown>>, hasShell: false, displayName: "Auth callback" },

  { pattern: "/home/empty", component: HomeEmpty as Component<Record<string, unknown>>, hasShell: true, displayName: "Home (empty)" },
  { pattern: "/home", component: Home as Component<Record<string, unknown>>, hasShell: true, displayName: "Home" },

  { pattern: "/contact/:id", component: ContactDetail as Component<Record<string, unknown>>, hasShell: true, displayName: "Contact detail" },
  { pattern: "/contact", component: Contact as Component<Record<string, unknown>>, hasShell: true, displayName: "Contact" },

  { pattern: "/wallet/kyc/form", component: WalletKycForm as Component<Record<string, unknown>>, hasShell: true, displayName: "Wallet · KYC form" },
  { pattern: "/wallet/kyc/verify", component: WalletKycVerify as Component<Record<string, unknown>>, hasShell: true, displayName: "Wallet · KYC verify" },
  { pattern: "/wallet/kyc", component: WalletKyc as Component<Record<string, unknown>>, hasShell: true, displayName: "Wallet · KYC" },
  { pattern: "/wallet", component: Wallet as Component<Record<string, unknown>>, hasShell: true, fullbleed: true, displayName: "Wallet" },

  { pattern: "/apps", component: ConnectedApps as Component<Record<string, unknown>>, hasShell: true, displayName: "Connected Apps" },

  { pattern: "/new", component: NewChat as Component<Record<string, unknown>>, hasShell: true, fullbleed: true, displayName: "New chat" },
  { pattern: "/agent/:taskId", component: Agent as Component<Record<string, unknown>>, hasShell: true, fullbleed: true, displayName: "Agent" },

  { pattern: "/onboarding/where", component: OnboardingWhere as Component<Record<string, unknown>>, hasShell: false, displayName: "Onboarding · Where" },
  { pattern: "/onboarding/role", component: OnboardingRole as Component<Record<string, unknown>>, hasShell: false, displayName: "Onboarding · Role" },
  { pattern: "/onboarding/apps", component: OnboardingApps as Component<Record<string, unknown>>, hasShell: false, displayName: "Onboarding · Apps" },
  { pattern: "/onboarding/done", component: OnboardingDone as Component<Record<string, unknown>>, hasShell: false, displayName: "Onboarding · Done" },

  // Dev-only chat UI gallery (`#/dev/chat-gallery`). Registered only in dev
  // builds: Vite replaces import.meta.env.DEV with false in prod and drops both
  // this entry and the ChatGallery import via dead-code elimination, so it
  // never ships to production. Pure fixture data — no controller/daemon.
  ...(import.meta.env.DEV
    ? [
        {
          pattern: "/dev/chat-gallery",
          component: ChatGallery as Component<Record<string, unknown>>,
          hasShell: false,
          fullbleed: true,
          displayName: "Chat gallery (dev)"
        }
      ]
    : [])
];

/**
 * '/' redirects depending on auth + onboarding state.
 *
 * Decision tree:
 *   authState.status === "signedOut" → "/login" (gate fires regardless
 *     of onboarded flag — we never want to show in-app chrome to an
 *     unauthenticated user).
 *   authState.status === "signedIn"  →
 *       isOnboarded() === false → "/onboarding/where" (first-run flow)
 *       isOnboarded() === true  → "/home"
 *   authState.status === "unknown"   → "/home" (App.svelte shows a
 *     loading splash while status is unknown, so the only consumer of
 *     this branch is the very first paint between mount and
 *     loadAuthFromStorage running).
 *
 * `ROOT_REDIRECT` stays exported for tests / callers that just want a
 * stable default. New callers should prefer `getRootRedirect()`.
 */
import { authState, isOnboarded } from "./lib/auth.svelte";

export const ROOT_REDIRECT = "/home";

export function getRootRedirect(): string {
  if (authState.status === "signedOut") return "/login";
  if (authState.status === "signedIn" && !isOnboarded()) return "/onboarding/where";
  return "/home";
}
