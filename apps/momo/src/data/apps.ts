/**
 * Connected Apps catalogue. Logos use the existing /service-icons paths
 * where available, otherwise an empty string — page agents will swap to
 * the real asset (svg/png) as they implement the page.
 */

import type { AppEntry } from "./types";

export const apps: AppEntry[] = [
  { id: "google-calendar", name: "Google Calendar", status: "connected", logo: "/service-icons/google-calendar.svg" },
  { id: "gmail", name: "Gmail", status: "not_connected", logo: "/service-icons/gmail.svg" },
  { id: "google-meet", name: "Google Meet", status: "not_connected", logo: "/service-icons/google-meet.svg" },
  { id: "lark", name: "Lark", status: "not_connected", logo: "/service-icons/lark.svg" },
  { id: "twitter", name: "Twitter / X", status: "not_connected", logo: "/service-icons/twitter.svg" },
  { id: "facebook", name: "Facebook", status: "not_connected", logo: "/service-icons/facebook.svg" },
  { id: "instagram", name: "Instagram", status: "not_connected", logo: "/service-icons/instagram.svg" },
  { id: "github", name: "GitHub", status: "not_connected", logo: "/service-icons/github.svg" },
  { id: "notion", name: "Notion", status: "not_connected", logo: "/service-icons/notion.svg" },
  { id: "imessage", name: "iMessage", status: "not_connected", logo: "/service-icons/imessage.svg" }
];
