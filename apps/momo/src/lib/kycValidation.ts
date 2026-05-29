/**
 * Frontend pre-validation for the KYC form. Mirrors the rules Strada
 * enforces server-side (verified 2026-04-29 against api2.stradacarte.cloud)
 * so users get inline feedback instead of a generic backend rejection.
 *
 * Email rules follow the TC37 one-pager (KYC Didit): max 36 characters and
 * no "+" in the address. These are surfaced to the user verbatim by the
 * KycHelpModal, so keep the two in sync if either changes.
 *
 * The wire format requires `firstName`, `lastName`, `dob`, `email`,
 * `callingCode`, `countryCallingCode`, `phoneNum`, `cellNum`, `address`,
 * `city`, `country`, `zipCode` for every country, plus `state` when
 * `country === "US"`. See ./walletTypes.ts → CreateCardHolderParams.
 *
 * Ported 1:1 from worldclaw-app/lib/kyc-validation.ts.
 */

import { parsePhoneNumberFromString } from 'libphonenumber-js';

const EMAIL_RE = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;
// Per the TC37 one-pager, the cardholder email must not contain a "+"
// (Strada rejects it). Kept as its own regex so we can give a specific
// message instead of a generic "invalid format".
const EMAIL_PLUS_RE = /\+/;
const ISO_ALPHA2_RE = /^[A-Z]{2}$/;
const CALLING_CODE_RE = /^\d{1,4}$/;
const DIGITS_ONLY_RE = /^\d+$/;
const DOB_RE = /^\d{4}-\d{2}-\d{2}$/;

// US states + DC + territories Strada will accept (Strada itself doesn't
// validate the value beyond non-empty, but we want to prevent silly typos
// like "California" or "ca" from sneaking through).
export const US_STATE_CODES = [
  'AL', 'AK', 'AZ', 'AR', 'CA', 'CO', 'CT', 'DE', 'DC', 'FL',
  'GA', 'HI', 'ID', 'IL', 'IN', 'IA', 'KS', 'KY', 'LA', 'ME',
  'MD', 'MA', 'MI', 'MN', 'MS', 'MO', 'MT', 'NE', 'NV', 'NH',
  'NJ', 'NM', 'NY', 'NC', 'ND', 'OH', 'OK', 'OR', 'PA', 'RI',
  'SC', 'SD', 'TN', 'TX', 'UT', 'VT', 'VA', 'WA', 'WV', 'WI',
  'WY', 'PR', 'VI', 'GU', 'AS', 'MP',
] as const;

const US_STATE_SET = new Set<string>(US_STATE_CODES);

// `countryCallingCode` is on the wire (CreateCardHolderParams) but not
// in the form — it's always the same ISO Alpha-2 as `country`, so we
// derive it at submit time rather than asking the user twice.
export interface KycFormShape {
  firstName: string;
  lastName: string;
  dob: string;
  email: string;
  country: string;
  callingCode: string;
  phoneNum: string;
  address: string;
  city: string;
  state: string;
  zipCode: string;
}

export type KycFieldKey = keyof KycFormShape;

/**
 * Validate one field. Returns `null` when valid, otherwise a short
 * user-facing message. Some validators look at sibling fields
 * (phone needs callingCode + country; state needs country), so we
 * accept the whole form.
 */
export function validateField(
  key: KycFieldKey,
  value: string,
  form: KycFormShape,
): string | null {
  const v = (value ?? '').trim();
  switch (key) {
    case 'firstName':
    case 'lastName':
      if (!v) return 'Required.';
      if (v.length > 50) return 'Maximum 50 characters.';
      if (!/^[A-Za-z][A-Za-z\s'-]*$/.test(v)) {
        return 'Letters, spaces, hyphen and apostrophe only.';
      }
      return null;

    case 'dob':
      if (!v) return 'Required.';
      if (!DOB_RE.test(v)) return 'Use YYYY-MM-DD (e.g. 1990-05-21).';
      if (!isRealDate(v)) return 'That date does not exist.';
      {
        const age = ageInYears(v);
        if (age < 18) return 'You must be at least 18 years old.';
        if (age > 100) return 'Please double-check the year.';
      }
      return null;

    case 'email':
      if (!v) return 'Required.';
      // TC37 one-pager: max 36 chars, no "+" in the address.
      if (v.length > 36) return 'Maximum 36 characters.';
      if (EMAIL_PLUS_RE.test(v)) return 'Email cannot contain a "+".';
      if (!EMAIL_RE.test(v)) return 'Invalid email format.';
      return null;

    case 'country':
      if (!v) return 'Required.';
      if (!ISO_ALPHA2_RE.test(v.toUpperCase())) {
        return 'ISO Alpha-2 (2 uppercase letters), e.g. CN, US, GB.';
      }
      return null;

    case 'callingCode':
      if (!v) return 'Required.';
      if (!CALLING_CODE_RE.test(v)) return 'Digits only, e.g. 86, 1, 44.';
      return null;

    case 'phoneNum': {
      if (!v) return 'Required.';
      if (!DIGITS_ONLY_RE.test(v)) return 'Digits only, no spaces or dashes.';
      const callingCode = form.callingCode?.trim();
      // Phone validation depends on the country (libphonenumber wants
      // the ISO Alpha-2 to interpret the local format). `country` is
      // the single source of truth.
      const isoCountry = form.country?.trim().toUpperCase();
      if (!callingCode || !ISO_ALPHA2_RE.test(isoCountry || '')) {
        // Can't validate cross-field yet — wait for the other fields
        // to be filled rather than show a misleading "invalid phone".
        return null;
      }
      const e164 = `+${callingCode}${v.replace(/^0+/, '')}`;
      const parsed = parsePhoneNumberFromString(
        e164,
        isoCountry as Parameters<typeof parsePhoneNumberFromString>[1],
      );
      if (!parsed || !parsed.isValid()) {
        return 'Invalid phone number for the selected country.';
      }
      return null;
    }

    case 'address':
      if (!v) return 'Required.';
      if (v.length < 4) return 'At least 4 characters.';
      if (v.length > 50) return 'Maximum 50 characters.';
      return null;

    case 'city':
      if (!v) return 'Required.';
      if (v.length > 50) return 'Maximum 50 characters.';
      return null;

    case 'state': {
      if (!v) return 'Required.';
      if (v.length > 50) return 'Maximum 50 characters.';
      const isUS = form.country?.trim().toUpperCase() === 'US';
      if (isUS && !US_STATE_SET.has(v.toUpperCase())) {
        return 'Use a valid US state code (e.g. CA, NY).';
      }
      return null;
    }

    case 'zipCode':
      if (!v) return 'Required.';
      if (v.length > 12) return 'Maximum 12 characters.';
      return null;

    default:
      return null;
  }
}

/**
 * Validate the whole form. Returns a map of field → error for invalid
 * fields, plus a flat list of failing keys in form order (handy for
 * scrolling to the first invalid input).
 */
export function validateForm(form: KycFormShape): {
  errors: Partial<Record<KycFieldKey, string>>;
  invalidKeys: KycFieldKey[];
} {
  const order: KycFieldKey[] = [
    'firstName', 'lastName', 'dob', 'email',
    'country', 'callingCode', 'phoneNum',
    'address', 'city', 'state', 'zipCode',
  ];
  const errors: Partial<Record<KycFieldKey, string>> = {};
  const invalidKeys: KycFieldKey[] = [];
  for (const key of order) {
    const err = validateField(key, form[key] ?? '', form);
    if (err) {
      errors[key] = err;
      invalidKeys.push(key);
    }
  }
  return { errors, invalidKeys };
}

function isRealDate(s: string): boolean {
  // s already matches YYYY-MM-DD
  const [y, m, d] = s.split('-').map((p) => Number(p));
  if (!y || !m || !d) return false;
  const dt = new Date(Date.UTC(y, m - 1, d));
  return (
    dt.getUTCFullYear() === y &&
    dt.getUTCMonth() === m - 1 &&
    dt.getUTCDate() === d &&
    dt.getTime() <= Date.now()
  );
}

function ageInYears(dob: string): number {
  const [y, m, d] = dob.split('-').map((p) => Number(p));
  const now = new Date();
  let age = now.getFullYear() - y;
  const beforeBirthday =
    now.getMonth() + 1 < m ||
    (now.getMonth() + 1 === m && now.getDate() < d);
  if (beforeBirthday) age -= 1;
  return age;
}
