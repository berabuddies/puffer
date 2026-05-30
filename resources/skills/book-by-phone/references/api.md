# Book-by-Phone API Reference (WorldRouter Edition)

## Base URL

```
{WORLDROUTER_BASE_URL}/v1/services/lifeclaw/skill
```

- test: `https://control-api-test-0f9c17.worldrouter.ai`
- prod: `https://control-api.worldrouter.ai`

All paths below are relative to that base (e.g. `/search` → `{WORLDROUTER_BASE_URL}/v1/services/lifeclaw/skill/search`).

## Authentication

All endpoints require: `Authorization: Bearer $WORLDROUTER_API_KEY`

This is the user's **own WorldRouter API key**, created in the WorldRouter dashboard. It authenticates against the user's WorldRouter account and is the account that gets billed. There is **no third-party (LifeClaw) token** on this path — WorldRouter holds the upstream provider credential server-side. Rotate / revoke from the WorldRouter dashboard.

## Endpoints

### POST /search

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| query | string | Yes | — | Search query (max 500 chars) |
| location | string | No | "" | Location hint (e.g. "Tokyo", "Singapore") |
| language | string | No | "en" | Result language |
| limit | int | No | 5 | Results to return (1-20) |

**Response:**
```json
{
  "results": [
    {
      "name": "Sushi Dai",
      "address": "5-2-1 Tsukiji, Chuo City",
      "rating": 4.6,
      "cid": "12345678901234567",
      "maps_url": "https://www.google.com/maps?cid=12345678901234567"
    }
  ]
}
```

### POST /detail

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| name | string | Yes | Business name (max 200 chars) |
| cid | string | Yes | Google Maps CID from search (max 200 chars) |
| language | string | No | Result language (default "en") |

**Response:**
```json
{
  "detail": {
    "phone": "+81312345678",
    "address": "5-2-1 Tsukiji, Chuo City",
    "website": "https://example.com",
    "hours": "Mon: 05:00-14:00, Tue: 05:00-14:00",
    "rating": 4.6,
    "rating_count": 1234,
    "price_level": "$$$$",
    "type": "Sushi restaurant",
    "menu": "https://example.com/menu",
    "booking_url": "https://...",
    "maps_url": "https://www.google.com/maps?cid=...",
    "thumbnail_url": "https://..."
  },
  "phone_ref": "eyJjaWQ...",
  "phone_ref_unavailable_reason": null
}
```

**Phone fields:**
- `detail.phone` — raw merchant phone number for **display only**
- `phone_ref` — signed token for `/book/phone`. Present when phone booking is available.
- `phone_ref_unavailable_reason` — why `phone_ref` is null (e.g. `"calling this region is not currently supported"`, `"phone number not in international format"`)

### POST /book/phone

One endpoint for all phone calls. The `action` field determines server behavior — it is the primary routing key. WorldRouter runs a **balance precheck** before dialing; insufficient balance returns `402` and no call is placed.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| action | string | No | **Routing key.** `book` (default), `cancel`, `reschedule`, `inquiry` |
| phone_ref | string | Conditional | Required for `book` and `inquiry` |
| booking_id | int | Conditional | Required for `cancel` and `reschedule` |
| call_plan | object | Yes | See below |

**`phone_ref` and `booking_id` are mutually exclusive.** Passing both returns 422.

**Preconditions for cancel/reschedule:**
- `booking_id` must exist and belong to the current account — 404 if not found.
- Booking must have status `confirmed` and a callable merchant phone — 422 otherwise.

**call_plan fields (new booking):**

| Field | Level | Description |
|-------|-------|-------------|
| purpose | Required | Natural-language prompt for the AI caller (does not affect routing). e.g. "Book a table at …" |
| merchant_name | Required | Business name |
| date, time, party_size, name | Required | Booking details |
| contact_phone | Recommended | E.164 format (e.g. "+6591234567") — auto-formatted for voice readability |
| special_requests | Optional | String array — passed to AI caller as-is |
| predicted_qa | Optional | Array of {question, answer} — helps AI caller handle merchant questions |
| fallback_instructions | Optional | Free-text fallback guidance for the AI caller |
| language | Ignored | Do not set — auto-inferred from merchant phone country |

**call_plan fields (cancel/reschedule):**

| Field | Required | Description |
|-------|----------|-------------|
| purpose | Yes | e.g. "Cancel reservation at Sushi Dai" |
| new_date | For reschedule | New date (provide if date is changing) |
| new_time | For reschedule | New time (provide if time is changing) |

At least one of `new_date` or `new_time` is required for reschedule. Only include the fields that are changing. Server auto-fills: `language`, merchant phone, original booking details.

**call_plan fields (inquiry):**

| Field | Required | Description |
|-------|----------|-------------|
| purpose | Yes | e.g. "Ask about opening hours and dress code" |

Result contains the merchant's answer in `result.summary`. No booking record is created.

**Response (202):**
```json
{
  "task_id": 42,
  "status": "pending",
  "poll_url": "/v1/services/lifeclaw/skill/task/42"
}
```

### PATCH /bookings/{booking_id}

Client-side write-back: update your booking record after confirming the call result. This is the caller's own record — the server does not auto-update bookings based on call outcomes.

| Field | Type | Description |
|-------|------|-------------|
| status | string | New status, e.g. "cancelled" |
| booking_time | string | New date+time for reschedule. Recommended format: `"YYYY-MM-DD HH:MM"` (merchant's local time) |
| party_size | int | New party size |

At least **one** field required; empty body returns 422.

### GET /task/{task_id}

**Pending:**
```json
{ "task_id": 42, "status": "pending", "created_at": "2026-04-04T10:00:00+00:00" }
```

**Completed:**
```json
{
  "task_id": 42,
  "status": "completed",
  "result": {
    "status": "confirmed",
    "summary": "Table booked for 2 at 12pm on April 10th",
    "details": "Confirmed under name Alex",
    "conditions": ["Smart casual dress code"],
    "duration_seconds": 45
  },
  "completed_at": "2026-04-04T10:02:15+00:00"
}
```

### GET /bookings

| Param | Type | Default | Description |
|-------|------|---------|-------------|
| status | string | "" | Filter: "confirmed", "cancelled", or empty for both |
| limit | int | 20 | Max results (capped at 50) |

```json
{
  "bookings": [
    {
      "id": 3,
      "merchant_name": "Sushi Dai",
      "method": "phone",
      "status": "confirmed",
      "booking_time": "2026-04-07 18:00",
      "party_size": 4,
      "merchant_phone": "+81312345678",
      "created_at": "2026-04-06T10:00:00+00:00"
    }
  ]
}
```

## Billing & Settlement

Billing is on the **user's WorldRouter account** (not a separate LifeClaw bill):

1. **Precheck** before dialing — insufficient balance → `402`, no call placed.
2. **Reserve** an authorization hold for the estimated max cost on dial.
3. **Settle** on completion:
   - Connected + booked → **capture** the actual cost (balance `spend` increases).
   - No answer / zero-cost → **release** the hold, balance unchanged.
   - Rejected → release the hold, balance unchanged.

A real completed call is charged once; a retried provider webhook for the same call must not double-charge (idempotent on the provider request id).

## Error Codes

| Code | Meaning |
|------|---------|
| 401 | Missing or invalid WorldRouter API key |
| 402 | Insufficient WorldRouter balance (precheck denied) — surface top-up |
| 403 | WorldRouter account inactive / suspended |
| 404 | Resource not found |
| 422 | Invalid request body |
| 429 | Rate / concurrency cap hit — `in_flight` and `cap` echoed in body |
| 502 | Voice service unavailable |

**Retry guidance**
- **429 concurrency** (`partner_concurrent_call_cap_exceeded`): retry after a few seconds. **429 spend cap**: do not retry within the period — surface to user.
- **502**: retry once after 5s; else surface.
- **other 5xx**: retry once with backoff. **other 4xx**: do not retry — fix the request shape.
