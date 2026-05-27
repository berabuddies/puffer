/**
 * Connected Apps catalogue. Only the apps Momo can actually authenticate
 * end-to-end live here — Telegram (personal-account login) and Email
 * (IMAP/SMTP). Both flows are driven by `puffer internal-tool …` via the
 * Tauri RPCs in `src-tauri/src/connectors.rs`.
 *
 * The original Paper artboard 1B6-0 had ten brands. We intentionally hide
 * the eight puffer doesn't support yet (Google Calendar, Meet, Lark,
 * Twitter, Facebook, Instagram, GitHub, Notion, iMessage) instead of
 * showing them as placeholders the user can't click through.
 */

import type { AppEntry } from "./types";

export const apps: AppEntry[] = [
  {
    id: "telegram",
    name: "Telegram",
    status: "not_connected",
    logo: "/service-icons/telegram.svg",
    connectorSlug: "telegram-login",
  },
  {
    id: "email",
    name: "Email",
    status: "not_connected",
    logo: "/service-icons/email.svg",
    connectorSlug: "email",
  },
];
