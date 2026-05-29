# momo KYC form — field-help modal

**Date:** 2026-05-29
**Author:** sean
**Status:** implemented (2026-05-29)

## Problem

The KYC form (`/wallet/kyc/form`, `WalletKycForm.svelte`) collects an
11-field cardholder payload, but gives the user no guidance on how to fill
each field. Several fields have non-obvious rules that the Strada backend
(TC37 / Didit) enforces — e.g. email must be ≤36 chars and contain no `+`,
phone must be ≤10 digits with the calling code in a separate field. Users
who don't know these rules get a generic backend rejection.

Source of truth for the rules: `KYC Didit - one pager.pdf` (TC37 — KYC
verification Requirements), "Stradacarte Portal – Card holder Creation"
section.

## Scope

In scope:
- A single help trigger on the form page that opens a modal listing
  per-field filling guidance.
- Guidance limited to the fields that actually exist on the momo form
  (name, DOB, email, country, code, phone, address, city, state, zip).
  Fields the PDF mentions but the momo form does not have (middle name,
  nationality, place of birth, occupation, "Address 2") are **excluded**.
- Aligning the email validator in `kycValidation.ts` to the PDF rules so
  the on-screen tip and the actual enforcement agree.

Out of scope (per decisions below):
- The PDF's non-form content: Didit verification steps, ID requirements,
  Proof-of-Address rules, KYC status, TC37 card info.
- Changing phone validation (libphonenumber stays — see Decisions).

## Decisions

1. **Content scope** — only field-filling guidance for fields that exist on
   the momo form. (Not a verbatim PDF dump, not a full KYC help center.)
2. **Rule precedence** — when the PDF conflicts with the current momo
   validator, the PDF wins. This applies to the modal text **and** the
   validator (see item 4).
3. **Single trigger** — one entry point ("a tip"), not per-field icons.
4. **Email validator alignment** — update `kycValidation.ts` email rule to
   match the PDF: max length **36** (was 50) and **reject any `+`** via
   regex. Keeps tip text == enforcement.
5. **Phone validation unchanged** — momo validates phone via
   `libphonenumber-js` against the selected country, which is more accurate
   than a flat "max 10 digits" and avoids false rejections for non-US
   numbers. The modal still tells the user "no country code, use the Code
   field," which does not conflict.

## Design

### Trigger
In `WalletKycForm.svelte`, add an always-visible help trigger in the
existing `.kyc-form__title-row` (right of the "Identity verification"
title, alongside the mock-only dev link, which is unchanged). It is an
icon + text link: a `lucide-svelte` `Info` (or `HelpCircle`) icon plus
"How to fill this in". Clicking it sets a local `helpOpen` state to `true`.

### Modal
New component `src/components/wallet/KycHelpModal.svelte`, reusing the
existing `Modal.svelte` primitive (props `open` / `onClose` / `title`;
it already provides ESC-to-close, backdrop-click-to-close, and body-scroll
lock). Patterned after `TopUpModal.svelte`. `WalletKycForm.svelte` owns
`let helpOpen = $state(false)` and renders `<KycHelpModal open={helpOpen}
onClose={() => (helpOpen = false)} />`.

Title: "How to fill in your details". Body: a definition-style list of
field → tip rows.

### Content (per-field tips, PDF wording)
| Field | Tip |
|---|---|
| First / Last name | Enter your legal name exactly as it appears on your government ID. |
| Date of birth | Use the date of birth shown on your ID. You must be at least 18. |
| Email | Max 36 characters. Do not include a `+` in the email. |
| Country | Select your country from the dropdown. |
| Code | Your country calling code — auto-filled when you pick a country. |
| Phone number | Max 10 digits. Don't add the country calling code — use the Code field for that. |
| Address | Enter your complete address (max 50 characters). |
| City | The city on your proof of address. |
| State / Region | For the US, pick your state; otherwise enter your region / province. |
| Zip / Postal | Your postal / ZIP code. |

### Validator change (`kycValidation.ts`, `case 'email'`)
- Change `if (v.length > 50)` → `if (v.length > 36) return 'Maximum 36 characters.'`
- Add a regex-based `+` rejection (per request, handle via regex): e.g.
  `const EMAIL_NO_PLUS_RE = /^[^\s@+]+@[^\s@]+\.[^\s@]+$/;` used in place of
  the current `EMAIL_RE` (the `+` is excluded from the local part / whole
  string by the character class), or a dedicated `if (/\+/.test(v)) return
  'Email cannot contain a "+".'` guard for a clearer message. Prefer the
  dedicated guard so the user sees *why* it failed rather than a generic
  "Invalid email format."
- Update the file's header comment that documents the email rule.

## Components & data flow

```
WalletKycForm.svelte
  ├─ helpOpen: $state(false)
  ├─ header title-row → [Info] "How to fill this in"  (onclick → helpOpen = true)
  └─ <KycHelpModal {open=helpOpen} onClose=…>
        └─ <Modal title="How to fill in your details">
              └─ static per-field tip list
```
The modal is presentational and self-contained: no props beyond
`open` / `onClose`, no data fetching, content is static markup.

## Testing
- `npm run check` — type-check passes.
- `npm run test:desktop-ui` — add a test that the trigger opens the modal
  and that ESC / close button / backdrop dismiss it.
- Validator: a unit-level assertion (or extend an existing kyc test) that a
  37-char email and an email containing `+` are now rejected.

## Risks
- Tip/validator divergence is the main risk; item 4 removes it for email.
  Phone keeps a documented, intentional gap (tip says "max 10 digits",
  validator defers to libphonenumber) — acceptable because libphonenumber
  is stricter/more correct per country.
