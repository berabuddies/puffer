---
name: book-anything
description: Search for businesses and book, cancel, reschedule, or inquire about appointments via AI phone calls. Use whenever a user says "find", "book", "reserve", "cancel", "reschedule", or asks about a local business (restaurant, salon, clinic, hotel, etc.), including its hours, phone, address, menu, or availability. The AI caller automatically matches the merchant's language, so use this for non-English bookings too.
metadata:
  author: lifeclaw
  version: 1.4.2
  homepage: https://lifeclaw.agentese.ai
license: Proprietary
---

# Book Anything

Searches for businesses and books appointments via AI phone calls. Also cancels, reschedules, and makes merchant inquiries by phone.

> **Requires** the `LIFECLAW_API_TOKEN` environment variable. Get a token by messaging `@lifeclaw_ai_bot` on Telegram with `/token_create <name>`. Tokens are shown once and cannot be recovered if lost; revoke with `/token_revoke <name>`.

## Quick start

Base URL: `https://api-v2.lifeclaw.agentese.ai`

All requests require the header `Authorization: Bearer $LIFECLAW_API_TOKEN`.

The core workflow is four steps:

1. **Search** — `POST /skill/search` with `query` and `location`.
2. **Get details** — `POST /skill/detail` with `name` and `cid` to retrieve a `phone_ref`.
3. **Book by phone** — `POST /skill/book/phone`, only if `phone_ref` is not null.
4. **Poll for the result** — `GET /skill/task/{task_id}` every 10 seconds until `status` is no longer `pending`.

## When to use this skill

- The user asks to find a business (restaurant, salon, clinic, hotel, etc.).
- The user wants to book or reserve something by phone.
- The user wants to cancel or reschedule an existing reservation.
- The user wants to ask a merchant a question (hours, menu, dress code, etc.).
- The user needs business details (phone number, hours, address, reviews).

## Step details

### Step 2 — Detail response

- `phone_ref` — signed token used for booking. If null, phone booking is unavailable; fall back to `detail.phone` and `booking_url`.
- `detail.phone` — merchant phone number, for display only.
- `booking_url` — online booking link, if available.

### Step 3 — Book by phone

The `call_plan` object accepts:

- **Required:** `purpose`, `merchant_name`, `date`, `time`, `party_size`, `name`
- **Recommended:** `contact_phone` (E.164 format)
- **Optional:** `special_requests`, `predicted_qa`, `fallback_instructions`

### Step 4 — Interpreting the result

- `confirmed` — the booking succeeded.
- `pending` — the merchant has not confirmed yet; keep polling.
- `rejected` — the merchant refused.
- `failed` — the call failed (no answer, etc.).

## Cancel and reschedule

1. `GET /skill/bookings` to retrieve booking history.
2. `POST /skill/book/phone` with `action: "cancel"` or `action: "reschedule"` and the `booking_id`.
3. After confirmation, `PATCH /skill/bookings/{booking_id}` to update the record.

## Inquiry workflow

For general questions without a booking, call `POST /skill/book/phone` with `action: "inquiry"`, then poll for `result.summary`.

## Other endpoints

- Balance check: `GET /skill/balance`
- Booking history: `GET /skill/bookings?status=confirmed&limit=10`

## Edge cases

- **`phone_ref` is null:** show the merchant phone and `booking_url` instead of booking.
- **Insufficient points (HTTP 402):** the response includes a `topup_url`; surface it to the user.
- **No search results:** suggest broadening the query.
- **Polling exceeds 5 minutes:** the task has likely timed out.

## Privacy and data handling

User data (name, phone, party size, special requests) is sent to the LifeClaw API solely to complete bookings. It is not shared with third parties beyond the merchant, not used for advertising, and retained only as booking records. Tokens are scoped and revocable.

For data-handling questions: https://t.me/agenteseAI
