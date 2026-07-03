# TG Subscriber Runtime Hardening Design (plan-02 / agentenv/monorepo#764)

Date: 2026-07-03
Branch: `refactor/tg-subscriber-runtime-hardening`
Issues covered: #610 #639 #717 (hydration has no readiness semantics), #604 (slow degradation detection + reconnect without backoff)
Constraints: backward compatibility is out of scope; optimize only for long-term benefit, stability, and performance; avoid over-engineering; changes limited to the puffer repo.

## 0. Root-cause summary (inputs to this design)

1. **Synchronous blocking hydration**: on the contacts RPC path the daemon spawns a thread that dials Telegram; after `recv_timeout(15s)` expires the thread is abandoned but keeps running (`daemon_contacts_telegram_peer_cache.rs:230-350`), with no in-flight deduplication.
2. **Missing readiness semantics**: `telegram_peer_cache_needs_hydration` treats "peers non-empty" as "complete" (:213-228); `contacts_search` and query-bearing `list` hardcode `ready: true` (`daemon_contacts.rs:177,245`); whether contact-book hydration finished is recorded nowhere.
3. **Session write race**: the daemon hydrate client and the subscriber live client share the same `telegram.session` and each writes it back (`daemon_contacts_telegram_peer_cache.rs:311-314` vs `persist_live_session_state`).
4. **Degradation detected only by 60s polling**: `health_from_control_event` (`puffer-subscriptions/manager.rs:1001-1052`) does not consume `resume_failed` / `update_loop_error` events; auth-class failures wait for the 60×1s tick of `spawn_auth_monitor`.
5. **Fragile runtime reconnect**: the update loop's recovery branch = fixed 1s delay + a single resume; on failure the process exits (`client.rs:486-531`), forcing a fresh login on network jitter or sleep/wake.

## 1. Core decisions

| Decision | Choice | Rationale |
|---|---|---|
| Telegram connection ownership | **Entirely owned by the subscriber**; the daemon never dials | the session race is eliminated architecturally; single connection, single writer |
| contacts RPC semantics | **Fully non-blocking**: read cache and return immediately + status field, push an event on completion | RPC latency stays stable at milliseconds; no timeout parameter to tune |
| Runtime reconnect | **Reuse the login-phase offline-docking state machine** instead of adding a new retry loop | one backoff mechanism serves both scenarios; net code reduction |
| UI scope | minimal consumption in puffer-desktop (banner + event re-fetch), no changes to pagination/layout | the new contract has a real consumer inside this repo |

## 2. Hydration ownership refactor

### 2.1 Subscriber side (`crates/puffer-subscriber-telegram-user`)

- **New command** `TelegramHydrateContacts { target: usize }` (defined in `puffer-subscriber-runtime/src/command.rs`).
- Runtime handling: `tokio::spawn` contact-book hydration on the existing live client (`contacts.GetContacts{hash:0}` + `contacts.GetSaved`) + a recent-dialog scan up to `target`.
- **Single-flight**: at most one in-flight hydration task per process (holding a `JoinHandle`); while a task is running, a repeated command is immediately acked with `contacts_hydrated { ok: false, state: "hydrating" }` — no queuing, no stacking.
- **Write `contact_book.state = "hydrating"` as soon as the task starts** (otherwise, until completion the daemon can only read stale not-ready); on completion/failure write the terminal state + emit the control event `contacts_hydrated { ok, error?, peer_count }`.
- **The writer of the recent-dialogs marker file also moves to the subscriber**: the hydrate command performs the dialog scan and writes the marker; the daemon's readiness computation reads both the peer-cache v2 and the marker files, both read-only.
- Receiving this command during the login phase: reply `contacts_hydrated { ok: false, state: "auth_required" }`.

### 2.2 peer-cache.json v2

New top-level field:

```json
"contact_book": {
  "state": "ready | hydrating | failed",
  "hydrated_at_ms": 1730000000000,
  "last_error": null
}
```

- Bump `CACHE_VERSION`. Old caches (without the `contact_book` field) are all treated as not-ready and self-heal after one triggered hydration; **no migration code is written**.
- From now on this file has **exactly one writer: the subscriber**.

### 2.3 Daemon side (`crates/puffer-cli`)

**Remove**: `hydrate_telegram_peer_cache_from_session_blocking`, `hydrate_telegram_recent_peer_cache_from_session_blocking`, the `TEST_HYDRATOR` stub, and the daemon-internal `Client::connect` dial path. `daemon_contacts_telegram_peer_cache.rs` shrinks to: cache reading + readiness-state computation + hydrate-command dispatch.

**Behavior**:

- `contacts_list` / `contacts_search`: pure cache read, return immediately; when an account is found not-ready, fire-and-forget a single hydrate command.
- `contacts_refresh`: unconditionally send a force-hydrate command, then immediately return the current snapshot.
- **Bring up the subscriber on demand**: reuse `start_connection_subscriber` (relaxing the `has_consumer` precondition), and only for accounts that **have a session file** — when the subscriber is not running, bring it up first, then send the command.
- **Obtaining the manager**: the contacts handler accesses it via the `puffer_core::subscription_manager()` global `OnceLock` (there is precedent at `daemon_workflows.rs:112`); **the RPC signature is not changed**; when the manager is unavailable it degrades to pure cache reads (status is still surfaced, only hydration is not triggered).
- **Account directory ↔ connection mapping**: the `telegram-accounts/<dir>` directory name *is* the connection slug (an existing convention in connect.rs); the first implementation step pins this convention with an assertion/test.
- **Response contract (breaking change)**: `ready: bool` is replaced by

  ```json
  "sync": { "state": "ready | hydrating | failed | auth_required", "updated_at_ms": ..., "error": null }
  ```

  Aggregation rule across multiple accounts: any account `hydrating` → `hydrating`; else any `failed` → `failed`; else any `auth_required` → `auth_required`; all ready → `ready`. `has_more` / `next_cursor` are unchanged.
- The daemon consumes the `contacts_hydrated` control event → publishes a `contacts_updated` event to the frontend event bus (`DaemonState::events` broadcast).

## 3. Event-driven degradation (`crates/puffer-subscriptions/manager.rs`)

`health_from_control_event` gains two new mappings (the subscriber already emits these events; only the daemon has not been consuming them):

| Event | class | ConnectionHealthStatus |
|---|---|---|
| `resume_failed` | `auth` | `AuthRequired` (→ immediately `Degraded`) |
| `resume_failed` | `network` | `Retrying` (→ immediately `Degraded`) |
| `update_loop_error` | `auth` | `AuthRequired` (→ immediately `Degraded`) |
| `update_loop_error` | `network` / `other` | `Retrying` (→ immediately `Degraded`) |

- **`resume_failed` is mapped only for class ∈ {auth, network}**: benign first-login paths such as `not_signed_in` (class `none`/`config`) are not mapped, avoiding marking a normal first login as Degraded — that scenario is already covered by the `login_required` event.
- Zero changes on the subscriber side.
- The 60s poll (`spawn_auth_monitor`) is kept as a fallback (covering silent process death) and is no longer the primary detection path.
- Degradation awareness drops from up to 60s to seconds.

## 4. Runtime bounded-backoff reconnect (`crates/puffer-subscriber-telegram-user/src/client.rs`)

**Unify onto the single offline-docking state machine** (the existing `OfflineResumeState`: starts at 5s, ×2, capped at 60s, commands remain responsive while docked, emits a `resume_offline` event):

- `UpdateLoopExit` gains a new variant `WentOffline(String)`.
- When the update loop hits a **network-class** stream error: remove the existing "fixed 1s + single resume" branch and instead return `WentOffline(detail)` → `run()` re-enters the login loop's offline-docking branch with `OfflineResumeState::new(detail)`.
- **`run()` control-flow restructure (the largest structural change in this design)**: today "login loop → main loop" is a linear two-phase sequence, and `WentOffline` cannot return to the docking state. Extract offline-docking + the login loop into a re-enterable function (or turn `run()` into an explicit phase outer loop), so that startup and runtime-offline share the same code. This restructure is constrained to be behavior-preserving (all existing login/docking tests keep passing).
- **auth-class** errors: go through the existing `ReauthStarted` path back to the login phase (`login_required` is already mapped to `Degraded`).
- **The process no longer exits on stream errors**; fatal exit is reserved for genuine surprises like stdin disconnect.
- `next_offline_retry_delay` gains **full jitter**: `delay/2 + rand(delay/2)`, seeded from `SystemTime` nanoseconds, introducing no new dependency; the 60s cap is unchanged.

### Explicitly out of scope (to prevent over-engineering)

- No new `ConnectionState` variant (reuse `Degraded`).
- No MTProto heartbeat/keepalive probing (the update stream itself is the liveness signal).
- The poll interval is not made configurable.
- No hydration task queue/priority (single-flight is sufficient).

## 5. puffer-desktop minimal consumption

- `src/lib/api/desktop.ts`: `ContactsSnapshot` gains a `sync` field; subscribe to the `contacts_updated` event.
- `src/lib/screens/Contacts.svelte`:
  - `sync.state === "hydrating"` → a non-blocking "syncing" hint next to Refresh, current candidates keep showing;
  - `sync.state === "failed"` → show the error, with Refresh as the retry entry point (no new button);
  - on `contacts_updated` → auto re-fetch.
- No changes to pagination, layout, or other interactions.

## 6. Test matrix

| Issue scenario | Test landing point |
|---|---|
| 30s cold-start hydration delay | daemon contacts test: inject a `state:"hydrating"` cache file, assert the RPC returns immediately and `sync.state` is correct (pure file injection, no thread stub) |
| request list while hydration in progress | as above + assert partial candidates still return |
| hydration completes | subscriber unit test: `contacts_hydrated` emitted + v2 metadata set; manager test: → `contacts_updated` forwarded |
| second-level awareness of network disconnect | manager test: `update_loop_error{class=network}` envelope → record becomes `Degraded/Retrying` |
| successful backoff reconnect | subscriber unit test: `WentOffline` → offline docking → recovery (extends the existing offline-resume test) |
| repeated disconnects, no storm | jitter unit test: interval `[delay/2, delay]`, capped at 60s |

Additional cases: hydrate-command single-flight (repeated commands do not stack); hydrate command during the login phase returns `auth_required`.

## 7. Deliverables and deletion checklist

**Changes**:

- `crates/puffer-subscriber-runtime/src/command.rs`: +1 command.
- `crates/puffer-subscriber-telegram-user`: hydrate task + single-flight, peer-cache v2, `WentOffline`, jitter.
- `crates/puffer-subscriptions/src/manager.rs`: +2 event mappings, `contacts_hydrated → contacts_updated` forwarding.
- `crates/puffer-cli`: make contacts pure-read, bring up the subscriber on demand, the `sync` contract.
- `apps/puffer-desktop`: types + banner + event listener.

**Deletions**: the daemon's two blocking hydration paths, `TEST_HYDRATOR`, the update loop's single-recovery branch, the old `ready: bool` contract. Expected net decrease in code.

**Risk point**: bringing up the subscriber on demand introduces a new dependency direction "contacts RPC → manager starts a process"; `start_subscriber` is already invoked concurrently by the auth monitor thread, so the infrastructure is mature — the implementation must verify there is no lock-ordering issue.

**External impact (accepted)**: the `ready: bool` contract consumed by the bobo repo is replaced by the `sync` object; bobo needs to adapt as follow-up. This design keeps no compatibility shim for it.
