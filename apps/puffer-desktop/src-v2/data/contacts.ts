import type { Contact } from "./types";

/**
 * The Paper design uses email-derived avatar initials (M for mei@…, D for
 * daniel@…, etc.) rather than the contact's display-name initial. We mirror
 * that exactly so the list + detail visuals match the artboard.
 */
export const contacts: Contact[] = [
  {
    id: "hanzhi",
    name: "Hanzhi",
    role: "Product Lead",
    email: "mei@worldagent.ai",
    avatarLabel: "M",
    colorHint: "warm",
    org: "WorldAgent",
    lastActive: "Active 12 min ago",
    nextMeeting: "Product sync · Tue 10:00",
    lastTouch: "Lark message · Yesterday"
  },
  {
    id: "chaofan",
    name: "Chaofan",
    role: "Engineering Manager",
    email: "daniel@worldagent.ai",
    avatarLabel: "D",
    colorHint: "cool",
    org: "WorldAgent",
    lastActive: "Active 3 hr ago",
    nextMeeting: "Eng review · Thu 14:00",
    lastTouch: "Email reply · Today"
  },
  {
    id: "helen",
    name: "Helen",
    role: "Design Partner",
    email: "sara@studio.kr",
    avatarLabel: "S",
    colorHint: "neutral",
    org: "Studio KR",
    lastActive: "Active yesterday",
    nextMeeting: "Design crit · Fri 11:00",
    lastTouch: "Figma comment · 2 days ago"
  },
  {
    id: "sean",
    name: "Sean",
    role: "Advisor",
    email: "austin@northstar.vc",
    avatarLabel: "A",
    colorHint: "warm",
    org: "Northstar",
    lastActive: "Active last week",
    nextMeeting: "Quarterly check-in · Mon 09:30",
    lastTouch: "Lark message · 5 days ago"
  }
];

export function findContact(id: string): Contact | undefined {
  return contacts.find((c) => c.id === id);
}
