---
name: book-by-phone
description: Search for local businesses (restaurants, salons, clinics, hotels) and place real AI phone-call bookings on the user's behalf, plus cancel, reschedule, and ask merchants questions by phone. Use when a user says "find", "book", "reserve", "cancel", "reschedule", or asks about a local business. Calls go through the WorldRouter API and are authenticated and billed against the user's own WorldRouter account — no third-party token is needed. Supports any language; the AI caller automatically matches the merchant's language.
metadata:
  author: worldrouter
  version: "1.0.0"
  homepage: https://worldrouter.ai
  clawdbot:
    emoji: "📞"
    requires:
      env:
        - WORLDROUTER_API_KEY
        - WORLDROUTER_BASE_URL
    primaryEnv: WORLDROUTER_API_KEY
---

# Book by Phone (via WorldRouter)

Search for any business, get full details, and book via an AI-powered phone call — all through the **WorldRouter** API, billed against the user's own WorldRouter account.

## For Integrators

This skill is **agent instructions + a REST API contract**, not a library. To embed it in an application (e.g. a desktop assistant that solves problems for customers, including by phone):

1. **Load** this `SKILL.md` (and `references/api.md` when needed) into your agent's context. The agent decides when to trigger based on the `description` above.
2. **Authenticate with a WorldRouter API key** — the user's *own* account credential, created in the WorldRouter dashboard. There is **no LifeClaw token** in this skill; the phone-call provider is wrapped behind WorldRouter and its credentials live server-side, never in the client.
3. **Set the base URL** via `WORLDROUTER_BASE_URL` so the same skill works across environments (test / prod).
4. **Never ship the API key in the desktop binary.** Proxy the calls through your own backend so the key is not extractable from the client.
5. **Billing is on the user's WorldRouter balance.** Each completed call is captured from their balance; failed/zero-cost calls are not charged. See [Billing & Settlement](#billing--settlement).

## Configuration

| Env var | Description | Example |
|---------|-------------|---------|
| `WORLDROUTER_API_KEY` | The user's WorldRouter API key (`Authorization: Bearer ...`) | `wr_...` |
| `WORLDROUTER_BASE_URL` | WorldRouter control API base | test: `https://control-api-test-0f9c17.worldrouter.ai` · prod: `https://control-api.worldrouter.ai` |

All endpoints live under `{WORLDROUTER_BASE_URL}/v1/services/lifeclaw/skill/...`. (`lifeclaw` here is just WorldRouter's internal service name for the phone-booking route — it is your own API, not a call to a third party.)

## Quick Start

```bash
# Load the user's WorldRouter credentials (written by the host app at login).
source ~/.wr/.creds

BASE="$WORLDROUTER_BASE_URL/v1/services/lifeclaw/skill"

# 1. Search
curl -X POST "$BASE/search" \
  -H "Authorization: Bearer $WORLDROUTER_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"query": "sushi near Shibuya", "location": "Tokyo"}'

# 2. Get details + phone_ref
curl -X POST "$BASE/detail" \
  -H "Authorization: Bearer $WORLDROUTER_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"name": "Sushi Dai", "cid": "12345678901234567"}'

# 3. Book by phone (only if phone_ref is not null)
curl -X POST "$BASE/book/phone" \
  -H "Authorization: Bearer $WORLDROUTER_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"phone_ref": "<phone_ref from step 2>", "call_plan": {"purpose": "Book a table", "merchant_name": "Sushi Dai", "date": "2026-04-10", "time": "12:00", "party_size": 2, "name": "Alex"}}'

# 4. Poll until status != "pending" (use task_id from step 3 response)
curl "$BASE/task/{task_id}" \
  -H "Authorization: Bearer $WORLDROUTER_API_KEY"
```

## When to Use

- User asks to **find a business** (restaurant, salon, clinic, hotel, etc.)
- User wants to **book / reserve** something by phone
- User wants to **cancel or reschedule** an existing reservation
- User wants to **ask a merchant** a question (hours, menu, dress code, etc.)
- User needs **business details** (phone number, hours, address, reviews)

## Authentication

All requests require `Authorization: Bearer $WORLDROUTER_API_KEY`.

**Loading credentials:** Before any call, load the user's credentials:

```bash
source ~/.wr/.creds
```

This exports `WORLDROUTER_API_KEY` and `WORLDROUTER_BASE_URL` for the curl calls. The file is written by the host app at login (mode 0600). If it is missing, tell the user to sign in first — never invent or hardcode a key.

**Token source:** the user's own WorldRouter API key (the same one the host app uses for inference), authenticated against their WorldRouter account and billed there.

There is **no LifeClaw / third-party token** in this flow. WorldRouter holds the upstream phone-provider credential server-side.

---

## Workflow: New Booking

Follow steps 1–4 in order. Do NOT skip steps.

### Step 1 — Search

```
POST {base}/search
{"query": "sushi near Shibuya", "location": "Tokyo", "language": "en", "limit": 5}
```

Response: list of places with `cid` and `maps_url`.

### Step 2 — Get Details

```
POST {base}/detail
{"name": "Sushi Dai", "cid": "12345678901234567", "language": "en"}
```

Key response fields:
- `phone_ref` — signed token for booking. **If `null`, phone booking is NOT available.** Check `phone_ref_unavailable_reason`.
- `detail.phone` — merchant phone for **display only**
- `booking_url` — online booking link (if available)

**When `phone_ref` is `null`:** Do NOT call `/book/phone`. Show `detail.phone` and `booking_url` to the user instead.

### Step 3 — Book by Phone

**Only call this if `phone_ref` is not null.**

```
POST {base}/book/phone
{
  "phone_ref": "<phone_ref from Step 2>",
  "call_plan": {
    "purpose": "Book a table at Sushi Dai",
    "merchant_name": "Sushi Dai",
    "date": "2026-04-10",
    "time": "12:00",
    "party_size": 2,
    "name": "Alex",
    "contact_phone": "+6591234567"
  }
}
```

The `action` field determines what the server does (`book`, `cancel`, `reschedule`, `inquiry`). `purpose` inside `call_plan` is a natural-language prompt for the AI caller — it does NOT affect routing.

**call_plan fields (new booking):**
- **Required:** `purpose`, `merchant_name`, `date`, `time`, `party_size`, `name`
- **Recommended:** `contact_phone` (E.164) — auto-formatted for voice readability
- **Optional:** `special_requests` (string array), `predicted_qa` (array of {question, answer}), `fallback_instructions`
- **Ignored:** `language` — do not set, auto-inferred from merchant phone

Returns `{"task_id": {id}, "status": "pending", "poll_url": ".../task/{id}"}`.

Before the call is placed, WorldRouter runs a **balance precheck**. If the account can't cover the call, you get `402` and **no call is placed** (see [Billing & Settlement](#billing--settlement)).

### Step 4 — Poll Result

```
GET {base}/task/{task_id}
```

Poll every 10 seconds until top-level `status` is no longer `"pending"`. Do NOT use increasing delays — use a fixed 10-second interval. Timeout after 5 minutes.

**Two-layer status:** Top-level `status` (`pending` / `completed` / `failed`) indicates whether the task is still running. Inner `result.status` indicates the booking outcome. Only stop polling when top-level `status != "pending"`; only treat the operation as successful when `result.status == "confirmed"`.

**Result interpretation:**

| `result.status` | Meaning | Action |
|-----------------|---------|--------|
| `confirmed` | Booking succeeded | Show confirmation to user |
| `pending` | Merchant hasn't confirmed yet | Tell user to wait for callback |
| `rejected` | Merchant refused | Suggest alternatives |
| `failed` | Call failed (no answer, etc.) | Show merchant phone for manual action |

---

## Workflow: Cancel / Reschedule

Uses the same `/book/phone` endpoint with `booking_id` instead of `phone_ref`. Server auto-fills merchant phone and original booking details.

### Step 1 — Get Booking History

```
GET {base}/bookings?status=confirmed
```

### Step 2 — Make the Call

Use the `id` from Step 1 as `booking_id`.

**Cancel:**
```
POST {base}/book/phone
{"action": "cancel", "booking_id": 3, "call_plan": {"purpose": "Cancel reservation at Sushi Dai"}}
```

**Reschedule:**
```
POST {base}/book/phone
{"action": "reschedule", "booking_id": 3, "call_plan": {"purpose": "Reschedule reservation at Sushi Dai", "new_date": "2026-04-12", "new_time": "20:00"}}
```

Provide `new_date` and/or `new_time` — only include the fields that are changing.

Poll with Step 4 from the new booking workflow.

### Step 3 — Update Booking Record

After polling completes and `result.status == "confirmed"`, update the record:

```
PATCH {base}/bookings/{booking_id}
{"status": "cancelled"}                 // for cancel
{"booking_time": "2026-04-12 20:00"}    // for reschedule
```

**Important:** Booking records are client-managed. The server does NOT auto-update them based on call outcomes — you must call PATCH to write back the confirmed result.

---

## Workflow: Inquiry

For general questions (hours, menu, dress code, parking) — no booking involved.

```
POST {base}/book/phone
{"action": "inquiry", "phone_ref": "<phone_ref from /detail>", "call_plan": {"purpose": "Ask about opening hours and dress code"}}
```

Poll with Step 4. Answer is in `result.summary`. No booking record is created.

---

## Billing & Settlement

Calls are billed against the **user's WorldRouter account** — there is no separate LifeClaw billing on this path. The lifecycle:

1. **Precheck** (before dialing): WorldRouter checks the account has enough available balance. If not → `402 credit_unavailable`, **no call placed, nothing charged**.
2. **Reserve** (on dial): an authorization hold is placed for the estimated max cost.
3. **Settle** (on completion), based on the actual outcome reported back:
   - **Connected to a person + booked** → the real cost is **captured** from the balance (`spend` increases).
   - **No answer / zero-cost outcome** → the hold is **released**, balance unchanged.
   - **Merchant/voice service rejected** → hold released, balance unchanged.

So "正常结算" means: a real completed call charges the actual cost once (idempotent — a retried provider webhook must not double-charge); everything else releases the hold and charges nothing.

> Verifying settlement end-to-end (capture / cancel / precheck-deny / webhook idempotency) is exactly what the billing QA plan's TC-001…TC-007 cover. That validation runs against the WorldRouter test environment, not the client app.

## Edge Cases

- **`phone_ref` is `null`:** Do NOT call `/book/phone`. Show phone + booking_url instead.
- **402 (insufficient balance):** The user's WorldRouter balance can't cover the call. Surface the WorldRouter top-up flow. No call was placed.
- **403 (account inactive):** The WorldRouter account is suspended. Route the user to WorldRouter account support.
- **429 (rate / concurrency cap):** Too many in-flight calls. Retry after a few seconds if it's a concurrency cap; back off if it's a spend cap.
- **Phone call fails:** Show merchant phone for manual calling. Suggest `booking_url` if available.
- **No search results:** Suggest broadening query or different location.
- **Polling > 5 min:** Task likely timed out.

## Privacy & Data Handling

This skill sends user-provided data (name, phone number, party size, special requests) through WorldRouter to place phone bookings on the user's behalf. This data is:

- Used solely to complete the requested booking via an AI phone call
- Not shared with third parties beyond the merchant being called
- Not used for advertising or profiling
- Retained only as booking records accessible to the account owner

WorldRouter is the data controller for this flow; the upstream voice provider acts as a sub-processor. Data access/deletion requests are handled through WorldRouter.

## API Reference

See [references/api.md](references/api.md) for complete request/response schemas and error codes. All paths there are relative to `{WORLDROUTER_BASE_URL}/v1/services/lifeclaw/skill`.
