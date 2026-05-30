# Momo Onboarding Profile → puffer 全局 memory Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** onboarding 走到 Done 时把用户选的「国家+职业」写进 puffer 用户级全局 memory（`~/.puffer/AGENTS.md` + `~/.puffer/CLAUDE.md`），并对 onboarding gate 做显式加固 + 回归测试。

**Architecture:** 前端新增模块级 `$state` 跨页收集选择；Done 经现有 1431 `wsClient` 调 momo backend 新 RPC `write_user_profile`；Rust backend 解析 `$HOME/.puffer`、sanitize 输入、用「托管块」幂等合并写两个 md 文件（daemon 每 turn 从磁盘 fresh 读，无需重启）。Gate 加固：`OnboardingShell.onSkip` 显式 `markOnboarded()`（行为本由 `App.svelte` 的 `/home` `$effect` 兜底，此为显式化 + 防回归）。

**Tech Stack:** Rust (Tauri backend, anyhow, serde_json, tempfile for tests)；Svelte 5 runes (`.svelte.ts` 模块级 `$state`)；Playwright (`test:desktop-ui`) + FakeDaemon harness。

**Spec:** `docs/superpowers/specs/2026-05-30-momo-onboarding-profile-design.md`

---

## File Structure

- **Create** `apps/momo/src-tauri/src/user_profile.rs` — 纯逻辑：`sanitize` / `build_block` / `upsert_managed_block` / `write_profile_files`（接受目录参数，便于 tempdir 单测）。
- **Modify** `apps/momo/src-tauri/src/lib.rs` — 注册 `mod user_profile;`。
- **Modify** `apps/momo/src-tauri/src/backend.rs` — `handle` 加 `write_user_profile` 分支 + 薄方法（解析 `$HOME/.puffer` → 调 `user_profile::write_profile_files`）。
- **Create** `apps/momo/src/lib/onboarding.svelte.ts` — 模块级 `$state` 收集器 + `setCountry`/`setRole`/`commitProfile`。
- **Modify** `apps/momo/src/pages/onboarding/Where.svelte` — `pick` 里 `setCountry`。
- **Modify** `apps/momo/src/pages/onboarding/Role.svelte` — `pickPreset`/`onCustomInput`/`onCustomSubmit` 里 `setRole`。
- **Modify** `apps/momo/src/pages/onboarding/Done.svelte` — `onMount` 调 `commitProfile()`。
- **Modify** `apps/momo/src/components/onboarding/OnboardingShell.svelte` — `onSkip` 调 `markOnboarded()`。
- **Modify** `apps/momo/tests/support/fakeDaemon.ts` — `dispatch` 加 `case "write_user_profile"`。
- **Create** `apps/momo/tests/onboarding-profile.spec.ts` — e2e：画像写入 + gate 回归。
- **Modify** `apps/momo/CLAUDE.md` — 订正「没有用户级全局 memory」。

---

## Task 1: `user_profile.rs` — 纯逻辑 + 文件写入（Rust, TDD）

**Files:**
- Create: `apps/momo/src-tauri/src/user_profile.rs`
- Modify: `apps/momo/src-tauri/src/lib.rs:1` (add `mod user_profile;`)

- [ ] **Step 1: 注册模块**

在 `apps/momo/src-tauri/src/lib.rs` 顶部 `mod backend;` 之后加一行：

```rust
mod backend;
mod user_profile;
```

- [ ] **Step 2: 写实现（含 markers / sanitize / upsert / 写文件）**

Create `apps/momo/src-tauri/src/user_profile.rs`:

```rust
//! User-profile memory: writes the onboarding profile (country + role) into a
//! delimited "managed block" inside puffer's user-level global memory files
//! (`~/.puffer/AGENTS.md` + `~/.puffer/CLAUDE.md`). The block is upserted
//! idempotently so re-running onboarding replaces it without clobbering any
//! other content in those files. puffer-core reads these files fresh each turn
//! (see crates/puffer-core/runtime/system_prompt.rs), so no daemon restart is
//! needed.

use std::path::Path;

const BEGIN: &str = "<!-- BEGIN momo-user-profile (managed by onboarding) -->";
const END: &str = "<!-- END momo-user-profile -->";

/// Collapse a user-supplied string to a single safe line: strip the HTML-comment
/// delimiters (so a malicious/accidental marker string can't break block
/// detection) and fold all whitespace (incl. newlines) into single spaces.
fn sanitize(s: &str) -> String {
    s.replace("<!--", " ")
        .replace("-->", " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Build the managed block from optional country/role. Returns `None` when both
/// are empty (after sanitize), signalling "nothing to write".
pub fn build_block(country: Option<&str>, role: Option<&str>) -> Option<String> {
    let country = country.map(sanitize).filter(|s| !s.is_empty());
    let role = role.map(sanitize).filter(|s| !s.is_empty());
    if country.is_none() && role.is_none() {
        return None;
    }
    let mut lines = vec![BEGIN.to_string(), "## About the user".to_string()];
    if let Some(c) = &country {
        lines.push(format!("- Lives in: {c}"));
    }
    if let Some(r) = &role {
        lines.push(format!("- Role / occupation: {r}"));
    }
    lines.push(END.to_string());
    Some(lines.join("\n"))
}

fn ensure_trailing_newline(mut s: String) -> String {
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// Insert or replace the managed block in `existing`. If a well-formed block
/// (BEGIN before END) is present, its inclusive span is replaced; otherwise the
/// block is appended (preserving all existing content). Idempotent.
pub fn upsert_managed_block(existing: &str, block: &str) -> String {
    match (existing.find(BEGIN), existing.find(END)) {
        (Some(b), Some(e)) if e > b => {
            let end_idx = e + END.len();
            let mut out = String::with_capacity(existing.len() + block.len());
            out.push_str(&existing[..b]);
            out.push_str(block);
            out.push_str(&existing[end_idx..]);
            ensure_trailing_newline(out)
        }
        _ => {
            let mut out = existing.trim_end().to_string();
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str(block);
            ensure_trailing_newline(out)
        }
    }
}

/// Write the profile into `<puffer_dir>/AGENTS.md` and `<puffer_dir>/CLAUDE.md`.
/// Creates `puffer_dir` if missing. Returns `Ok(false)` (no files touched) when
/// both fields are empty. Reads each file (empty if absent), upserts the block,
/// writes it back.
pub fn write_profile_files(
    puffer_dir: &Path,
    country: Option<&str>,
    role: Option<&str>,
) -> std::io::Result<bool> {
    let Some(block) = build_block(country, role) else {
        return Ok(false);
    };
    std::fs::create_dir_all(puffer_dir)?;
    for name in ["AGENTS.md", "CLAUDE.md"] {
        let path = puffer_dir.join(name);
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        let next = upsert_managed_block(&existing, &block);
        std::fs::write(&path, next)?;
    }
    Ok(true)
}
```

- [ ] **Step 3: 写失败测试**

在同文件 `user_profile.rs` 末尾追加：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_block_emits_both_bullets() {
        let block = build_block(Some("United States"), Some("Founder")).unwrap();
        assert!(block.starts_with(BEGIN));
        assert!(block.ends_with(END));
        assert!(block.contains("- Lives in: United States"));
        assert!(block.contains("- Role / occupation: Founder"));
    }

    #[test]
    fn build_block_omits_empty_field_and_returns_none_when_both_empty() {
        let only_role = build_block(Some("   "), Some("Engineer")).unwrap();
        assert!(!only_role.contains("Lives in"));
        assert!(only_role.contains("- Role / occupation: Engineer"));
        assert!(build_block(Some(""), None).is_none());
    }

    #[test]
    fn sanitize_strips_markers_and_newlines() {
        let block = build_block(None, Some("Founder\n<!-- END momo-user-profile -->")).unwrap();
        // Exactly one BEGIN and one END marker survive.
        assert_eq!(block.matches(BEGIN).count(), 1);
        assert_eq!(block.matches(END).count(), 1);
        // split_whitespace().join(" ") leaves exactly one space between tokens.
        assert!(block.contains("- Role / occupation: Founder END momo-user-profile"));
    }

    #[test]
    fn upsert_into_empty_creates_block_with_trailing_newline() {
        let block = build_block(Some("Japan"), None).unwrap();
        let out = upsert_managed_block("", &block);
        assert!(out.contains("- Lives in: Japan"));
        assert!(out.ends_with('\n'));
    }

    #[test]
    fn upsert_appends_without_clobbering_existing() {
        let block = build_block(Some("Korea"), None).unwrap();
        let out = upsert_managed_block("# My notes\n\nkeep me\n", &block);
        assert!(out.contains("# My notes"));
        assert!(out.contains("keep me"));
        assert!(out.contains("- Lives in: Korea"));
    }

    #[test]
    fn upsert_replaces_existing_block_and_is_idempotent() {
        let first = build_block(Some("Japan"), Some("Designer")).unwrap();
        let with_other = format!("preamble\n\n{first}\n\ntrailer\n");
        let second = build_block(Some("Singapore"), Some("Investor")).unwrap();
        let once = upsert_managed_block(&with_other, &second);
        assert!(once.contains("- Lives in: Singapore"));
        assert!(!once.contains("Japan"));
        assert!(once.contains("preamble"));
        assert!(once.contains("trailer"));
        assert_eq!(once.matches(BEGIN).count(), 1);
        // Writing the same block again changes nothing.
        let twice = upsert_managed_block(&once, &second);
        assert_eq!(once, twice);
    }

    #[test]
    fn write_profile_files_writes_both_and_skips_when_empty() {
        let dir = tempfile::tempdir().unwrap();
        let puffer = dir.path().join(".puffer");

        let wrote = write_profile_files(&puffer, Some("China"), Some("Student")).unwrap();
        assert!(wrote);
        for name in ["AGENTS.md", "CLAUDE.md"] {
            let body = std::fs::read_to_string(puffer.join(name)).unwrap();
            assert!(body.contains("- Lives in: China"));
            assert!(body.contains("- Role / occupation: Student"));
        }

        let wrote_empty = write_profile_files(&puffer, None, Some("  ")).unwrap();
        assert!(!wrote_empty);
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --manifest-path apps/momo/src-tauri/Cargo.toml user_profile`
Expected: 7 tests pass.

- [ ] **Step 5: Commit**

```bash
git add apps/momo/src-tauri/src/user_profile.rs apps/momo/src-tauri/src/lib.rs
git commit -m "feat(momo): user_profile managed-block writer for global memory"
```

---

## Task 2: backend `write_user_profile` RPC（wiring）

**Files:**
- Modify: `apps/momo/src-tauri/src/backend.rs:139` (handle 分支) + 新增私有方法

- [ ] **Step 1: 加 handle 分支**

在 `apps/momo/src-tauri/src/backend.rs` 的 `match method` 里，`"resolve_user_question"` 分支之后、`other =>` 之前插入：

```rust
            "resolve_user_question" => self.resolve_user_question(params),
            "write_user_profile" => {
                let country = optional_trimmed_string_param(&params, &["country"]);
                let role = optional_trimmed_string_param(&params, &["role"]);
                self.write_user_profile(country, role)
            }
            other => bail!("unknown method: {other}"),
```

- [ ] **Step 2: 加方法实现**

在 `impl` 块内（紧邻 `daemon_handshake` 方法处，见 `backend.rs:144` 附近）加：

```rust
    /// Persist the onboarding profile to puffer's user-level global memory
    /// (`~/.puffer/AGENTS.md` + `~/.puffer/CLAUDE.md`). Returns `{ written }`.
    fn write_user_profile(&self, country: Option<String>, role: Option<String>) -> Result<Value> {
        let dir = home_dir().join(".puffer");
        let written =
            crate::user_profile::write_profile_files(&dir, country.as_deref(), role.as_deref())
                .with_context(|| format!("writing user profile under {}", dir.display()))?;
        Ok(json!({ "written": written }))
    }
```

- [ ] **Step 3: 编译确认**

Run: `cargo check --manifest-path apps/momo/src-tauri/Cargo.toml`
Expected: 通过（`home_dir` / `optional_trimmed_string_param` / `Context` / `json!` 均已在 backend.rs 作用域内）。

- [ ] **Step 4: Commit**

```bash
git add apps/momo/src-tauri/src/backend.rs
git commit -m "feat(momo): add write_user_profile 1431 RPC"
```

---

## Task 3: onboarding 收集器 store

**Files:**
- Create: `apps/momo/src/lib/onboarding.svelte.ts`

- [ ] **Step 1: 写 store**

Create `apps/momo/src/lib/onboarding.svelte.ts`:

```ts
/**
 * Cross-page collector for onboarding selections.
 *
 * Onboarding pages (Where / Role) are separate routes, so per-page local
 * `$state` can't carry the choices forward to Done. This module-level `$state`
 * does — mirroring `sessionStore.svelte.ts` / `projectStore.svelte.ts`.
 *
 * `commitProfile()` persists the collected profile to puffer's user-level
 * global memory via the 1431 backend (`write_user_profile`). It is
 * fire-and-forget: a failure toasts but never blocks the Done -> /home hop.
 */
import { request } from "./wsClient";
import { pushToast } from "./toast.svelte";

const profile = $state<{ country: string | null; role: string | null }>({
  country: null,
  role: null
});

export function setCountry(country: string): void {
  profile.country = country;
}

export function setRole(role: string): void {
  profile.role = role;
}

export async function commitProfile(): Promise<void> {
  try {
    await request("write_user_profile", {
      country: profile.country,
      role: profile.role
    });
  } catch {
    pushToast("Couldn't save your profile — you can set it later.", "error");
  }
}
```

- [ ] **Step 2: 类型检查**

Run: `cd apps/momo && npm run check`
Expected: 无新增错误（`request` / `pushToast` 均为现有导出）。

- [ ] **Step 3: Commit**

```bash
git add apps/momo/src/lib/onboarding.svelte.ts
git commit -m "feat(momo): onboarding profile collector store"
```

---

## Task 4: Where / Role / Done 接线

**Files:**
- Modify: `apps/momo/src/pages/onboarding/Where.svelte`
- Modify: `apps/momo/src/pages/onboarding/Role.svelte`
- Modify: `apps/momo/src/pages/onboarding/Done.svelte`

- [ ] **Step 1: Where 采集国家**

在 `Where.svelte` 的 `<script>` 顶部 import 区加：

```ts
  import { setCountry } from "../../lib/onboarding.svelte";
```

在 `pick` 函数里、`selected = country;` 之后加 `setCountry(country);`：

```ts
  function pick(country: string): void {
    if (advancing) return;
    selected = country;
    setCountry(country);
    advancing = true;
    pushToast(`Where: ${country}`, "info");
    window.setTimeout(() => navigate("/onboarding/role"), 300);
  }
```

- [ ] **Step 2: Role 采集职业（三处）**

在 `Role.svelte` 的 import 区加：

```ts
  import { setRole } from "../../lib/onboarding.svelte";
```

`pickPreset` 里 `selected = role;` 之后加 `setRole(role);`：

```ts
  function pickPreset(role: string): void {
    if (advancing) return;
    selected = role;
    setRole(role);
    customRole = "";
```

`onCustomInput` 里 `selected = value;` 之后加 `setRole(value.trim());`：

```ts
    customRole = value;
    selected = value;
    setRole(value.trim());
    if (customTimer) clearTimeout(customTimer);
```

`onCustomSubmit` 里（冗余防御，Enter 提交路径）在 `if (customRole.trim().length === 0) return;` 之后加 `setRole(customRole.trim());`：

```ts
    if (customRole.trim().length === 0) return;
    setRole(customRole.trim());
    if (customTimer) clearTimeout(customTimer);
```

- [ ] **Step 3: Done 触发提交**

在 `Done.svelte` 的 import 区，把 `markOnboarded` 那行改为同时导入 commitProfile：

```ts
  import { markOnboarded } from "../../lib/auth.svelte";
  import { commitProfile } from "../../lib/onboarding.svelte";
```

`onMount` 里 `markOnboarded();` 之后加 `void commitProfile();`：

```ts
  onMount(() => {
    markOnboarded();
    void commitProfile();
    timer = setTimeout(() => navigate("/home"), 3000);
  });
```

- [ ] **Step 4: 类型检查**

Run: `cd apps/momo && npm run check`
Expected: 通过。

- [ ] **Step 5: Commit**

```bash
git add apps/momo/src/pages/onboarding/Where.svelte apps/momo/src/pages/onboarding/Role.svelte apps/momo/src/pages/onboarding/Done.svelte
git commit -m "feat(momo): collect onboarding country/role and commit on Done"
```

---

## Task 5: OnboardingShell skip 显式加固

**Files:**
- Modify: `apps/momo/src/components/onboarding/OnboardingShell.svelte:48-51`

- [ ] **Step 1: onSkip 落 flag**

在 `OnboardingShell.svelte` 的 import 区加：

```ts
  import { markOnboarded } from "../../lib/auth.svelte";
```

把 `onSkip` 改为：

```ts
  function onSkip(event: MouseEvent): void {
    event.preventDefault();
    // Skipping = the machine has seen onboarding; persist the gate flag so it
    // never reappears (App.svelte's /home $effect also backstops this; this is
    // the explicit, local guard + regression anchor — see the design spec §11).
    markOnboarded();
    if (skipTo) navigate(skipTo);
  }
```

- [ ] **Step 2: 类型检查**

Run: `cd apps/momo && npm run check`
Expected: 通过。

- [ ] **Step 3: Commit**

```bash
git add apps/momo/src/components/onboarding/OnboardingShell.svelte
git commit -m "feat(momo): mark onboarded on skip (explicit gate hardening)"
```

---

## Task 6: FakeDaemon dispatch case

**Files:**
- Modify: `apps/momo/tests/support/fakeDaemon.ts:1340` (dispatch switch)

- [ ] **Step 1: 加 case**

在 `fakeDaemon.ts` 的 `dispatch` switch 里（`case "logout_provider":` 之后，任意位置即可）加：

```ts
      case "logout_provider":
        return this.logoutProvider(request.params);
      case "write_user_profile":
        return { written: true };
```

（`record()` 在 dispatch 之前跑，所以 `waitForRequest("write_user_profile")` 不依赖此 case；加它只为消除 `default` 抛错的噪声。）

- [ ] **Step 2: Commit**

```bash
git add apps/momo/tests/support/fakeDaemon.ts
git commit -m "test(momo): FakeDaemon handles write_user_profile"
```

---

## Task 7: e2e — 画像写入 + gate 回归

**Files:**
- Create: `apps/momo/tests/onboarding-profile.spec.ts`

- [ ] **Step 1: 写测试**

Create `apps/momo/tests/onboarding-profile.spec.ts`:

```ts
import { expect, test } from "@playwright/test";
import { FakeDaemon } from "./support/fakeDaemon";

/**
 * Seed a signed-in machine that has NOT completed onboarding: a valid JWT
 * (so auth gate passes) but no `puffer.onboarded` flag (so getRootRedirect
 * sends "/" -> "/onboarding/where"). Mirrors bootHelpers.bootOnboarded minus
 * the onboarded seed. forceOnboarding query params are inert — do not use them.
 */
async function seedSignedInNotOnboarded(page: import("@playwright/test").Page) {
  await page.addInitScript(() => {
    const b64 = (o: unknown) =>
      btoa(JSON.stringify(o)).replace(/=+$/, "").replace(/\+/g, "-").replace(/\//g, "_");
    const header = b64({ alg: "RS256", typ: "JWT" });
    const payload = b64({
      sub: "test-user",
      email: "test@example.com",
      name: "Test User",
      exp: Math.floor(Date.now() / 1000) + 60 * 60 * 24
    });
    window.localStorage.setItem("puffer.authToken", `${header}.${payload}.test-sig`);
    window.localStorage.removeItem("puffer.onboarded");
  });
  await page.route("https://control-api.worldrouter.ai/**", (route) => route.abort());
}

test("completing onboarding writes the profile and sets the onboarded flag", async ({ page }) => {
  // FakeDaemon serves over a page.route ws mock — no start()/stop(); construct
  // + install() is the whole lifecycle (mirrors chat-smoke / sessions specs).
  const daemon = new FakeDaemon({ sessions: [] });
  await seedSignedInNotOnboarded(page);
  await daemon.install(page);

  await page.goto("/");
  await expect(page).toHaveURL(/#\/onboarding\/where$/);

  await page.getByRole("button", { name: "Japan" }).click();
  await expect(page).toHaveURL(/#\/onboarding\/role$/);
  await page.getByRole("button", { name: "Engineer" }).click();
  await expect(page).toHaveURL(/#\/onboarding\/apps$/);

  // Apps -> Done (skip link routes to /onboarding/done).
  await page.getByText("Skip for now and explore the app").click();

  const req = await daemon.waitForRequest("write_user_profile");
  expect(req.params).toMatchObject({ country: "Japan", role: "Engineer" });

  await expect
    .poll(() => page.evaluate(() => localStorage.getItem("puffer.onboarded")))
    .toBe("true");
});

test("skipping from Where marks the machine onboarded (gate regression)", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await seedSignedInNotOnboarded(page);
  await daemon.install(page);

  await page.goto("/");
  await expect(page).toHaveURL(/#\/onboarding\/where$/);
  await page.getByText("Skip for now and explore the app").click();

  await expect
    .poll(() => page.evaluate(() => localStorage.getItem("puffer.onboarded")))
    .toBe("true");
  // Re-resolving root must now land on /home, never back in onboarding.
  await page.goto("/");
  await expect(page).toHaveURL(/#\/home$/);
});
```

- [ ] **Step 2: 跑测试**

Run: `cd apps/momo && npm run test:desktop-ui -- onboarding-profile`
Expected: 2 tests pass.

> 若 button accessible-name 不匹配（Chip 渲染方式不同），用 `page.getByText("Japan", { exact: true })` 等定位；先跑一次按实际 DOM 调整 selector。skip 链接文案见 `OnboardingShell.svelte` 默认 `skipLabel`。`FakeDaemon` 用法 = `new FakeDaemon({ sessions: [] })` + `await daemon.install(page)`（无 `start()/stop()`，走 `page.route` ws mock；参考 `chat-smoke.spec.ts` / `sessions.spec.ts`）。

- [ ] **Step 3: Commit**

```bash
git add apps/momo/tests/onboarding-profile.spec.ts
git commit -m "test(momo): e2e onboarding profile write + skip gate regression"
```

---

## Task 8: 订正 `apps/momo/CLAUDE.md`

**Files:**
- Modify: `apps/momo/CLAUDE.md`（「memory 是 project 级，没有"用户级全局 memory"」一段）

- [ ] **Step 1: 改文档**

把该段（§「两个跨概念的坑」第二条）改为如实描述：`~/.puffer/{AGENTS,CLAUDE}.md` 是用户级全局注入（puffer-core 每 turn fresh 读，`system_prompt.rs`），provider 决定文件名（openai 优先 AGENTS.md 否则 CLAUDE.md），momo 经 1431 backend `write_user_profile` 把 onboarding 国家/职业写到这两个文件。保留 project `MEMORY.md` 的说明。

- [ ] **Step 2: Commit**

```bash
git add apps/momo/CLAUDE.md
git commit -m "docs(momo): correct global-memory note (user-level ~/.puffer AGENTS/CLAUDE.md)"
```

---

## 最终验证

- [ ] Run: `cargo test --manifest-path apps/momo/src-tauri/Cargo.toml user_profile` → 7 pass
- [ ] Run: `cargo check --manifest-path apps/momo/src-tauri/Cargo.toml` → ok
- [ ] Run: `cd apps/momo && npm run check` → ok
- [ ] Run: `cd apps/momo && npm run test:desktop-ui -- onboarding-profile` → 2 pass
- [ ] 手动验收：跑 onboarding → `cat ~/.puffer/AGENTS.md ~/.puffer/CLAUDE.md` 含 `momo-user-profile` 块 → 起 chat 问 agent「我在哪/我的职业」确认它知道。
