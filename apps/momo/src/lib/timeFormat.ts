/**
 * Tiny clock-format helper for chat bubble meta rows.
 *
 * Returns a 24-hour, zero-padded "HH:MM" string for a millisecond
 * epoch. Falsy / zero / undefined inputs return an empty string so
 * callers can pass `message.createdAt` blindly without a guard —
 * `{#if createdAt}` in the template then suppresses the meta row.
 *
 * No locale / timezone arg: chat bubbles render in the user's local
 * time (`Date.prototype.getHours/getMinutes`), matching v1.
 */
export function formatTime(ms: number | null | undefined): string {
  if (!ms) return "";
  const d = new Date(ms);
  const hh = String(d.getHours()).padStart(2, "0");
  const mm = String(d.getMinutes()).padStart(2, "0");
  return `${hh}:${mm}`;
}
