/**
 * Agent conversations — strings match the design artboards verbatim
 * (1UH-0 "Agent" and 1YY-0 "Agent · Restaurant Booking"), captured via
 * mcp__paper__get_screenshot.
 */

import type { AgentConversation } from "./types";

const calendar: AgentConversation = {
  slug: "calendar",
  title: "Create a calendar event with Alice",
  composerPlaceholder: "Hi, Tomo. How's my luck today?",
  steps: [
    {
      kind: "user",
      text: "Schedule a 30-minute product sync with Mei and Daniel next week. Prefer mornings, and add a short agenda."
    },
    {
      kind: "agent",
      text: "I'll check everyone's Google Calendar and look for morning slots next week."
    },
    {
      kind: "tool",
      icon: "calendar",
      label: "Reading availability from Google Calendar…"
    },
    {
      kind: "options",
      intro: "I found 3 good morning slots.",
      options: [
        {
          primary: "Tue, May 26 · 10:00–10:30",
          trailing: "Best fit",
          highlighted: true,
          badge: "Best fit"
        },
        {
          primary: "Wed, May 27 · 09:30–10:00",
          trailing: "Mei has a hard stop",
          highlighted: false
        },
        {
          primary: "Thu, May 28 · 11:00–11:30",
          trailing: "Daniel tentative",
          highlighted: false
        }
      ],
      footnote:
        "Suggested agenda: review launch blockers, confirm owner for metrics, agree next milestone."
    },
    {
      kind: "user",
      text: "Use Tuesday at 10. Invite Mei and Daniel, and add the agenda."
    },
    {
      kind: "result",
      title: "Event created in Google Calendar",
      subtitle: "Invites sent · Conferencing link added",
      detail: {
        title: "Product sync",
        facts: ["Tue, May 26", "10:00–10:30", "Mei, Daniel, Yuna"],
        notes: "Agenda: launch blockers, metrics owner, next milestone."
      },
      actions: [
        { label: "Open event", tone: "cream" },
        { label: "Edit details", tone: "neutral" }
      ]
    }
  ]
};

const restaurant: AgentConversation = {
  slug: "restaurant",
  title: "Book a restaurant by phone",
  composerPlaceholder: "Hi, Tomo. How's my luck today?",
  steps: [
    {
      kind: "user",
      text: "Call Narisawa and book a table for two this Friday night. Prefer 7:30, quiet seating if available."
    },
    {
      kind: "agent",
      text: "I'll call the restaurant, check availability, and confirm the reservation details."
    },
    {
      kind: "tool",
      icon: "phone",
      label: "Calling Narisawa · Tokyo · +81 3-5785-0799…"
    },
    {
      kind: "options",
      intro: "The restaurant has 3 available options.",
      options: [
        {
          primary: "Fri, May 29 · 19:30 · Counter seats",
          trailing: "Best fit",
          highlighted: true,
          badge: "Best fit"
        },
        {
          primary: "Fri, May 29 · 20:00 · Table for two",
          trailing: "Window side unavailable",
          highlighted: false
        },
        {
          primary: "Sat, May 30 · 18:30 · Table for two",
          trailing: "Earlier opening",
          highlighted: false
        }
      ],
      footnote:
        "They can note a quiet seating preference. A credit card is not required for this booking."
    },
    {
      kind: "user",
      text: "Use Friday at 7:30. Confirm under Hanzhi, phone ending 0821."
    },
    {
      kind: "result",
      title: "Reservation confirmed",
      subtitle: "Phone booking completed · Confirmation note saved",
      detail: {
        title: "Narisawa dinner",
        facts: ["Fri, May 29", "19:30", "2 guests · Hanzhi"],
        notes: "Notes: quiet seating preferred, arrive 10 minutes early."
      },
      actions: [
        { label: "View details", tone: "cream" },
        { label: "Call again", tone: "neutral" }
      ]
    }
  ]
};

export const conversations: Record<string, AgentConversation> = {
  calendar,
  restaurant
};

export function findConversation(slug: string): AgentConversation | undefined {
  return conversations[slug];
}
