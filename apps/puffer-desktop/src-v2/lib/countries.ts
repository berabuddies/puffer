/**
 * Country catalog used by the KYC form's country picker.
 *
 * We avoid bundling a 250-entry hand-written name table. The list is
 * derived at module init from two sources we already have:
 *   1. `libphonenumber-js` → enumerate every ISO Alpha-2 it supports
 *      and its numeric calling code (always in sync with the phone
 *      validator).
 *   2. `Intl.DisplayNames` (built into modern browsers / V8 / Hermes)
 *      → localize the ISO code to a human-readable country name with
 *      zero deps.
 *
 * Flags come from Unicode regional-indicator codepoints (no assets).
 *
 * Ported 1:1 from worldclaw-app/lib/countries.ts.
 */

import { getCountries, getCountryCallingCode } from 'libphonenumber-js';

export interface CountryEntry {
  iso: string; // ISO Alpha-2, uppercase (e.g. "CN")
  name: string; // Localized name (e.g. "China")
  callingCode: string; // Numeric calling code without "+" (e.g. "86")
  flag: string; // Two-glyph regional-indicator emoji (e.g. "🇨🇳")
}

function flagOf(iso: string): string {
  // Regional-Indicator Symbol Letter A is U+1F1E6. ISO 3166-1 alpha-2
  // codes map onto pairs of these glyphs by shifting from ASCII A (0x41).
  return iso
    .toUpperCase()
    .replace(/./g, (c) => String.fromCodePoint(127397 + c.charCodeAt(0)));
}

function buildCountries(): CountryEntry[] {
  // We pin the locale to 'en' so the picker stays predictable across
  // device languages — KYC paperwork is in English regardless.
  let names: Intl.DisplayNames | null = null;
  try {
    names = new Intl.DisplayNames(['en'], { type: 'region' });
  } catch {
    names = null;
  }

  const out: CountryEntry[] = [];
  for (const iso of getCountries()) {
    let callingCode: string;
    try {
      callingCode = String(getCountryCallingCode(iso));
    } catch {
      continue;
    }
    const name = (names && names.of(iso)) || iso;
    out.push({ iso, name, callingCode, flag: flagOf(iso) });
  }
  out.sort((a, b) => a.name.localeCompare(b.name));
  return out;
}

export const COUNTRIES: CountryEntry[] = buildCountries();

const BY_ISO: Map<string, CountryEntry> = new Map(
  COUNTRIES.map((c) => [c.iso, c]),
);

export function findCountryByIso(iso: string): CountryEntry | undefined {
  return BY_ISO.get(iso.toUpperCase());
}

/**
 * Filter for the picker's search box. Matches against:
 *   - country name (case-insensitive substring)
 *   - ISO code (prefix)
 *   - calling code (prefix, with optional leading "+")
 */
export function searchCountries(query: string): CountryEntry[] {
  const q = query.trim().toLowerCase();
  if (!q) return COUNTRIES;
  const ccQuery = q.replace(/^\+/, '');
  return COUNTRIES.filter((c) => {
    if (c.name.toLowerCase().includes(q)) return true;
    if (c.iso.toLowerCase().startsWith(q)) return true;
    if (c.callingCode.startsWith(ccQuery)) return true;
    return false;
  });
}
