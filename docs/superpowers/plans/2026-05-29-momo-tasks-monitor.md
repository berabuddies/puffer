# Momo Tasks (Telegram monitor) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 puffer-desktop 的 "Telegram monitor → task" 闭环移植到 momo desktop:momo 接入常驻 `puffer daemon`(前端直连 daemon ws),连 tg 成功后自动建 monitor,新增 momo 风格 Tasks 页面展示 monitor task 并支持 Ignore。

**Architecture:** momo Tauri backend 启动一个常驻 `puffer daemon` 子进程(user-level workspace `$HOME`)并通过新增的 ws 方法 `daemon_handshake` 把 `{url, token}` 暴露给前端;momo 前端新建一个 `daemonClient`(抄 desktop 的 `daemonClient.ts`,唯一改动是 handshake 来源从 tauri `invoke` 改为 `wsClient.request("daemon_handshake")`)直连 daemon,task RPC(`workflow_list` / `task_monitor_create` / `task_monitor_ignore`)直接发 daemon。chat 仍走现有 `puffer non-interactive`(过渡,后续单独迁)。

**Tech Stack:** Rust (Tauri backend, `anyhow`/`serde`/`serde_json`/std——零新增 crate);Svelte 5 runes + TypeScript (前端);Playwright (`npm run test:desktop-ui`) + `cargo test` / `cargo check`。

**Spec:** `docs/superpowers/specs/2026-05-29-momo-tasks-monitor-design.md`

**关键约定(贯穿全程):**
- connection slug = **`telegram-user`**(connection 实例),不是 `telegram-login`(connector 类型)。`createMonitor` / `task_monitor_*` 一律传 `telegram-user`。
- daemon ws 协议:momo 的 daemonClient 连的是 puffer daemon,**沿用 desktop daemonClient 的 envelope**——请求 `{ type:"request", id, method, params }`,响应 `{ type:"response", id, ok, result|error }`。(这跟 momo 自己的 1431 wsClient 协议不同,后者无 `type`——别混。)
- daemon workspace = `$HOME`(user-level),让 task 落 `~/.puffer/runtime/claude_workflow/monitor_tasks.json`、monitor memory 落 `~/.puffer/runtime/monitors/`,与 momo 已连的 `~/.puffer/connections.json` 一致。
- 每个 task 结束 **commit**;commit message 用英文,结尾加 `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`。
- 风险(已接受,不处理):常驻 daemon 与 chat 的 `non-interactive` 可能同连同一 tg account,过渡期可能互踢;chat 迁 daemon 后消解。

---

## File Structure

**Backend (`apps/momo/src-tauri/src/`):**
- `daemon_launcher.rs`(新建) — spawn `puffer daemon` + 解析 handshake + 随 app 退出 kill。抄自 `apps/puffer-desktop/src-tauri/src/daemon_launcher.rs`,裁掉 remote/SSH。
- `backend.rs`(改) — `BackendState` 加 `launcher` 字段;`handle` match 加 `"daemon_handshake"` 臂 + `daemon_handshake()` 方法。
- `lib.rs`(改) — `mod daemon_launcher;` + 启动时预热 daemon。

**前端 (`apps/momo/src/`):**
- `lib/daemonClient.ts`(新建) — 连 daemon ws 的 client + `ensureDaemonClient()`。抄 desktop `daemonClient.ts`,handshake 改走 `wsClient.request("daemon_handshake")`。
- `lib/taskApi.ts`(新建) — `loadWorkflowSnapshot` / `createMonitor` / `ignoreMonitorTask` + 类型。
- `lib/taskStore.svelte.ts`(新建) — `monitorTasks` `$state` + `loadTasks` / `ignoreTask` / 就绪降级。
- `pages/Tasks.svelte`(新建) — momo 风格列表 + Refresh + Ignore。
- `routes.ts`(改) — 加 `/tasks`。
- `components/shell/Sidebar.svelte`(改) — navEntries 加 Tasks。
- `pages/ConnectedApps.svelte`(改) — telegram 刚连上时自动 `createMonitor("telegram-user")`。

---

## Task 1: Backend — daemon_launcher 模块

**Files:**
- Create: `apps/momo/src-tauri/src/daemon_launcher.rs`
- Modify: `apps/momo/src-tauri/src/lib.rs`(加 `mod daemon_launcher;`)
- Verify: `cargo check --manifest-path apps/momo/src-tauri/Cargo.toml`

- [ ] **Step 1: 抄并裁剪 daemon_launcher.rs**

从 `apps/puffer-desktop/src-tauri/src/daemon_launcher.rs` 复制到 `apps/momo/src-tauri/src/daemon_launcher.rs`,**只保留 local 部分**:

保留并抄入(verbatim,行号是 desktop 源):
- `DaemonHandshake`(:18-25)、`DaemonChild` + `Drop`(:27-42)、`spawn_daemon`(:239-279)、`ensure_started`(:88-107)、`default_workspace_cwd` + `dirs_home`(:289-318)、`resolve_puffer_binary`(:344-389)、`resolve_builtin_resources_dir`(:398-411)、`ChildExt`/`try_wait_unchecked`(:218-234)。
- `DaemonLauncher` struct(:57-79)但**删掉 `remotes` 字段**,只留 `child: Mutex<Option<DaemonChild>>`;保留 `new()`(:82-84)。

删除(remote/SSH,momo 不需要):
- `AuxChildren` + Drop(:46-55)、`RemoteSession` + Drop(:65-78)、`DaemonLauncher::start_ssh`(:131-220)、`restart_local`(:113-126)、`spawn_remote_daemon`(:325-405)、`StderrTail`(:411-432)、`parse_ws_port`(:434-440)、`shell_quote`(:442-454)。

文件顶部需要的 use(按裁剪后实际用到的保留):
```rust
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
```

裁剪后 `DaemonLauncher` 应长这样(核对):
```rust
pub(crate) struct DaemonLauncher {
    child: Mutex<Option<DaemonChild>>,
}

impl DaemonLauncher {
    pub(crate) fn new() -> Self {
        Self { child: Mutex::new(None) }
    }

    pub(crate) fn ensure_started(&self) -> Result<DaemonHandshake> {
        // ... 抄 desktop :88-107
    }
}
```

- [ ] **Step 2: 注册模块**

在 `apps/momo/src-tauri/src/lib.rs` 顶部模块声明区(现有 `mod backend;` 等那组)加一行:
```rust
mod daemon_launcher;
```

- [ ] **Step 3: 编译验证(裁剪无悬挂引用)**

Run: `cargo check --manifest-path apps/momo/src-tauri/Cargo.toml`
Expected: 编译通过(可能有 `dead_code` warning,因为 `ensure_started` 还没被调——可接受;若报"unused import"则按提示删对应 use)。

- [ ] **Step 4: 写 handshake 解析单测**

在 `daemon_launcher.rs` 末尾加:
```rust
#[cfg(test)]
mod tests {
    use super::DaemonHandshake;

    #[test]
    fn parses_handshake_line() {
        let line = r#"{"url":"ws://127.0.0.1:51234/ws","token":"abc","protocolVersion":"1","workspaceRoot":"/Users/x"}"#;
        let hs: DaemonHandshake = serde_json::from_str(line).unwrap();
        assert_eq!(hs.url, "ws://127.0.0.1:51234/ws");
        assert_eq!(hs.token, "abc");
    }
}
```

- [ ] **Step 5: 跑测试**

Run: `cargo test --manifest-path apps/momo/src-tauri/Cargo.toml daemon_launcher`
Expected: PASS（`parses_handshake_line` 通过）。

- [ ] **Step 6: Commit**

```bash
git add apps/momo/src-tauri/src/daemon_launcher.rs apps/momo/src-tauri/src/lib.rs
git commit -m "feat(momo): add daemon_launcher (spawn puffer daemon + handshake)

Port puffer-desktop's local daemon launcher into momo, dropping the
remote/SSH paths. Spawns \`puffer daemon --bind 127.0.0.1:0
--print-handshake\` from \$HOME and parses the first stdout ndjson line
into a DaemonHandshake.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Backend — 接入 BackendState + `daemon_handshake` RPC

**Files:**
- Modify: `apps/momo/src-tauri/src/backend.rs`(`BackendState` 字段 + `new()` + `handle` match + 新方法)
- Modify: `apps/momo/src-tauri/src/lib.rs`(启动预热)
- Test: 同文件 `#[cfg(test)]`

- [ ] **Step 1: `BackendState` 加 launcher 字段**

`backend.rs` `BackendState` struct(现有 `turns` / `pending_questions` 两个 Mutex map)加字段:
```rust
launcher: crate::daemon_launcher::DaemonLauncher,
```
`BackendState::new()` 里初始化:
```rust
launcher: crate::daemon_launcher::DaemonLauncher::new(),
```

- [ ] **Step 2: 加 `daemon_handshake()` 方法**

在 `impl BackendState` 内加:
```rust
/// Ensures the local `puffer daemon` is running and returns its
/// WebSocket handshake so the frontend can dial the daemon directly.
fn daemon_handshake(&self) -> Result<crate::daemon_launcher::DaemonHandshake> {
    self.launcher.ensure_started()
}
```
(注:`DaemonHandshake` derives `Serialize`,`serde_value` 可直接序列化。)

- [ ] **Step 3: `handle` match 加一臂**

在 `backend.rs` `BackendState::handle` 的 `match method { ... }` 里(例如 `"run_agent_turn"` 那臂附近)加:
```rust
"daemon_handshake" => serde_value(self.daemon_handshake()?),
```

- [ ] **Step 4: 启动预热(lib.rs)**

`lib.rs` `run()` 里,在 `websocket::start_backend_ws(backend.clone());` 之后加(best-effort 预热,失败不阻断启动):
```rust
{
    let backend = backend.clone();
    std::thread::spawn(move || {
        if let Err(error) = backend.launcher.ensure_started() {
            eprintln!("momo: failed to pre-start puffer daemon: {error:#}");
        }
    });
}
```
(需要 `BackendState.launcher` 为 `pub(crate)` 或加一个 `pub(crate) fn ensure_daemon(&self)` 包装;选后者更干净:在 backend.rs 加 `pub(crate) fn ensure_daemon(&self) -> Result<()> { self.launcher.ensure_started().map(|_| ()) }`,lib.rs 调 `backend.ensure_daemon()`。)

- [ ] **Step 5: 写 RPC 测试**

⚠️ `ensure_started` 会真去 spawn `puffer`,单测里不可靠。改测"method 分发存在 + 错误不 panic":
```rust
#[cfg(test)]
mod daemon_handshake_tests {
    use super::*;
    use crate::events::EventEmitter;

    #[test]
    fn daemon_handshake_method_is_dispatched() {
        let state = BackendState::new();
        // 没有 puffer 二进制时应返回 Err(而非 "unknown method")
        let result = state.handle(EventEmitter::websocket_only(), "daemon_handshake", serde_json::json!({}));
        match result {
            Ok(v) => assert!(v.get("url").is_some(), "handshake should carry url"),
            Err(e) => assert!(!e.to_string().contains("unknown method"), "method must be dispatched, got: {e}"),
        }
    }
}
```

- [ ] **Step 6: 跑测试 + 编译**

Run: `cargo test --manifest-path apps/momo/src-tauri/Cargo.toml daemon_handshake`
Expected: PASS。
Run: `cargo check --manifest-path apps/momo/src-tauri/Cargo.toml`
Expected: 编译通过。

- [ ] **Step 7: Commit**

```bash
git add apps/momo/src-tauri/src/backend.rs apps/momo/src-tauri/src/lib.rs
git commit -m "feat(momo): expose daemon_handshake RPC + pre-start daemon

BackendState owns a DaemonLauncher; the \`daemon_handshake\` ws method
returns {url, token} so the frontend can dial the daemon. App startup
pre-warms the daemon on a background thread (best-effort).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: 前端 — daemonClient.ts(连 daemon ws)

**Files:**
- Create: `apps/momo/src/lib/daemonClient.ts`
- Test: `apps/momo/tests/...`(见 Task 5/6 的集成测,本 task 先编译 + 类型)

- [ ] **Step 1: 抄 desktop daemonClient,改 handshake 来源**

从 `apps/puffer-desktop/src/lib/api/daemonClient.ts` 复制核心到 `apps/momo/src/lib/daemonClient.ts`:抄 `DaemonClient` 类的 `connect()`(desktop :80-119)、`request()`(:122-159)、`close()`、ws url 拼 token 的 `webSocketUrl()`(:235-247)、`DaemonHandshake` 类型。

**唯一适配改动** —— 替换 `ensureLocalDaemonClient`:desktop 用 `invoke("ensure_local_daemon")` 拿 handshake;momo 改为走 1431 wsClient:
```typescript
import { request as backendRequest } from "./wsClient";

export interface DaemonHandshake {
  url: string;
  token: string;
  protocolVersion?: string;
  workspaceRoot?: string;
}

let sharedClient: DaemonClient | null = null;

/** Lazily starts (via backend) and connects to the local puffer daemon. */
export async function ensureDaemonClient(): Promise<DaemonClient> {
  if (sharedClient) return sharedClient;
  const handshake = await backendRequest<DaemonHandshake>("daemon_handshake");
  sharedClient = new DaemonClient(handshake);
  await sharedClient.connect();
  return sharedClient;
}
```
`DaemonClient.request` 沿用 desktop 的 envelope `{ type:"request", id, method, params }` 与响应解析 `{ type:"response", id, ok, result|error }`(**不要**改成 momo wsClient 那种无 type 的格式——daemon 端按 desktop 协议)。删掉 desktop 里 `useWebSocket=false → invoke("backend_request")` 的分支(momo 总是 ws)和 `configuredBrowserDaemonHandshake`/`switchDaemonClient`(momo 单 workspace 不需要)。

- [ ] **Step 2: 类型检查**

Run: `cd apps/momo && npm run check`
Expected: 无类型错误(若报 `wsClient` 的 `request` 签名不匹配,核对 `wsClient.ts:147` 的 `request<T>(method, params)` 签名)。

- [ ] **Step 3: Commit**

```bash
git add apps/momo/src/lib/daemonClient.ts
git commit -m "feat(momo): add daemonClient (direct ws to puffer daemon)

Ports puffer-desktop's DaemonClient; the only adaptation is the handshake
source — momo fetches {url, token} via the 1431 backend ws method
\`daemon_handshake\` instead of a tauri invoke. Envelope/protocol matches
the daemon (type:request / type:response).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: 前端 — 类型 + task API

**Files:**
- Create: `apps/momo/src/lib/taskApi.ts`

- [ ] **Step 1: 写类型 + 三个 API 函数**

`apps/momo/src/lib/taskApi.ts`(类型用 monitor_tasks 精确、其余放宽为 `unknown[]`,符合 momo zero-coupling 风格):
```typescript
import { ensureDaemonClient } from "./daemonClient";

export interface WorkflowMonitorTaskAction {
  name: string;
  prompt: string;
}

export interface WorkflowMonitorTask {
  task_id: string;
  subject: string;
  description: string;
  status: string;
  monitor_connection?: string | null;
  monitor_connector?: string | null;
  monitor_memory_path?: string | null;
  ignored?: boolean;
  actions?: WorkflowMonitorTaskAction[];
  possible_ignore_reasons?: string[];
  started_at_ms?: number | null;
  updated_at_ms?: number | null;
}

/** Subset of puffer's workflow snapshot — only monitor_tasks is typed precisely. */
export interface WorkflowSnapshot {
  monitor_tasks?: WorkflowMonitorTask[];
  monitor_task_error?: string | null;
  [key: string]: unknown;
}

export async function loadWorkflowSnapshot(): Promise<WorkflowSnapshot> {
  const client = await ensureDaemonClient();
  return client.request<WorkflowSnapshot>("workflow_list");
}

export async function createMonitor(connectionSlug: string): Promise<WorkflowSnapshot> {
  const client = await ensureDaemonClient();
  return client.request<WorkflowSnapshot>("task_monitor_create", { connection_slug: connectionSlug });
}

export async function ignoreMonitorTask(taskId: string, reason?: string): Promise<WorkflowSnapshot> {
  const client = await ensureDaemonClient();
  return client.request<WorkflowSnapshot>("task_monitor_ignore", {
    task_id: taskId,
    reason: reason?.trim() || undefined,
  });
}
```

- [ ] **Step 2: 类型检查**

Run: `cd apps/momo && npm run check`
Expected: 无类型错误。

- [ ] **Step 3: Commit**

```bash
git add apps/momo/src/lib/taskApi.ts
git commit -m "feat(momo): add task API (workflow_list / monitor create+ignore)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: 前端 — taskStore.svelte.ts

**Files:**
- Create: `apps/momo/src/lib/taskStore.svelte.ts`
- Test: `apps/momo/tests/tasks.spec.ts`(用 fakeDaemon)

- [ ] **Step 1: 写失败测试(fakeDaemon 注入 monitor_tasks)**

先看 `apps/momo/tests/support/fakeDaemon.ts` 现有 helper(它已 mock 1431 backend ws)。新增 `apps/momo/tests/tasks.spec.ts`,让 fakeDaemon 对 `daemon_handshake` 返回一个指向 fake 的 ws、对 `workflow_list` 返回带 2 条 `monitor_tasks` 的 snapshot,断言 Tasks 页面渲染 2 行。
```typescript
import { test, expect } from "@playwright/test";
import { startFakeDaemon } from "./support/fakeDaemon";

test("tasks page lists monitor tasks", async ({ page }) => {
  const daemon = await startFakeDaemon({
    daemon_handshake: () => ({ url: daemon.daemonWsUrl, token: "t" }),
    workflow_list: () => ({
      monitor_tasks: [
        { task_id: "monitor-1", subject: "Review KYC", description: "...", status: "pending" },
        { task_id: "monitor-2", subject: "Telecom notice", description: "...", status: "pending" },
      ],
    }),
  });
  await page.goto(daemon.appUrl + "#/tasks");
  await expect(page.getByText("Review KYC")).toBeVisible();
  await expect(page.getByText("Telecom notice")).toBeVisible();
});
```
⚠️ 若 `fakeDaemon` 当前不支持"再开一条 daemon ws",本步包含扩展 fakeDaemon:加一个第二 ws server 作 fake daemon,`daemon_handshake` 返回其 url。具体按 `fakeDaemon.ts` 现有结构扩展(它已用 ws server mock backend,复制一份作 daemon)。

- [ ] **Step 2: 跑测试看失败**

Run: `cd apps/momo && npm run test:desktop-ui -- tasks.spec.ts`
Expected: FAIL(`/tasks` 路由/页面还不存在)。

- [ ] **Step 3: 写 taskStore**

`apps/momo/src/lib/taskStore.svelte.ts`(仿 `sessionStore.svelte.ts` 的 runes 约定:模块级 `$state`,异步 load + try/catch + toast):
```typescript
import { loadWorkflowSnapshot, ignoreMonitorTask, type WorkflowMonitorTask } from "./taskApi";
import { pushToast } from "./toast.svelte";

export const monitorTasks = $state<WorkflowMonitorTask[]>([]);
export const taskState = $state<{ loading: boolean; ready: boolean; error: string | null }>({
  loading: false,
  ready: false,
  error: null,
});

export async function loadTasks(): Promise<void> {
  taskState.loading = true;
  taskState.error = null;
  try {
    const snapshot = await loadWorkflowSnapshot();
    const next = snapshot.monitor_tasks ?? [];
    monitorTasks.splice(0, monitorTasks.length, ...next);
    taskState.ready = true;
  } catch (error) {
    // daemon 未就绪/连接失败 → 降级,不崩
    taskState.error = error instanceof Error ? error.message : String(error);
  } finally {
    taskState.loading = false;
  }
}

export async function ignoreTask(taskId: string): Promise<void> {
  try {
    await ignoreMonitorTask(taskId);
    await loadTasks();
  } catch (error) {
    pushToast("Failed to ignore task", "error");
  }
}
```

- [ ] **Step 4: 跑测试(应在 Task 6 页面建好后转 PASS)**

本步先 `npm run check` 确保 store 类型正确;`tasks.spec.ts` 的 PASS 依赖 Task 6 的页面/路由。
Run: `cd apps/momo && npm run check`
Expected: 无类型错误。

- [ ] **Step 5: Commit**

```bash
git add apps/momo/src/lib/taskStore.svelte.ts apps/momo/tests/tasks.spec.ts apps/momo/tests/support/fakeDaemon.ts
git commit -m "feat(momo): add taskStore + tasks page test (fakeDaemon)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: 前端 — Tasks 页面 + 路由 + sidebar

**Files:**
- Create: `apps/momo/src/pages/Tasks.svelte`
- Modify: `apps/momo/src/routes.ts`(加 `/tasks`)
- Modify: `apps/momo/src/components/shell/Sidebar.svelte`(navEntries 加 Tasks)

- [ ] **Step 1: 写 Tasks 页面(momo 风格)**

`apps/momo/src/pages/Tasks.svelte`(仿 `pages/Home.svelte` 结构:`PageHeader` + accessory Refresh + 空态/列表;卡片仿 `components/home/TaskCard.svelte`;按钮用 `components/common/Button.svelte`):
```svelte
<script lang="ts">
  import { onMount } from "svelte";
  import PageHeader from "../components/shell/PageHeader.svelte";
  import Button from "../components/common/Button.svelte";
  import { monitorTasks, taskState, loadTasks, ignoreTask } from "../lib/taskStore.svelte";
  import { RefreshCw } from "lucide-svelte";

  onMount(() => { void loadTasks(); });

  function formatTime(ms?: number | null): string {
    if (!ms) return "";
    return new Date(ms).toLocaleString();
  }
</script>

<PageHeader title="Tasks" subtitle="Telegram monitor tasks">
  {#snippet accessory()}
    <Button variant="secondary" size="sm" label="Refresh" icon={RefreshCw} onclick={() => loadTasks()} />
  {/snippet}
</PageHeader>

<section class="tasks-feed">
  {#if taskState.error}
    <p class="text-body-compact tasks-empty">Daemon not ready — {taskState.error}</p>
  {:else if monitorTasks.length === 0}
    <p class="text-body-compact tasks-empty">No tasks yet. Connect Telegram and you'll see tasks here.</p>
  {:else}
    {#each monitorTasks as task (task.task_id)}
      <article class="task-card">
        <div class="task-card__body">
          <p class="text-task-title">{task.subject}</p>
          <p class="text-body-compact">{task.description}</p>
          <p class="text-eyebrow">{task.monitor_connection ?? "telegram"} · {task.status} · {formatTime(task.updated_at_ms)}</p>
        </div>
        <div class="task-card__actions">
          <Button variant="secondary" size="sm" label="Ignore" onclick={() => ignoreTask(task.task_id)} />
        </div>
      </article>
    {/each}
  {/if}
</section>

<style>
  .tasks-feed { display: flex; flex-direction: column; gap: var(--space-3); padding: var(--space-4); }
  .tasks-empty { color: var(--color-text-muted); padding: var(--space-6); text-align: center; }
  .task-card { display: flex; gap: var(--space-3); align-items: flex-start; justify-content: space-between;
    padding: var(--space-4); background: var(--color-surface-card); border-radius: var(--radius-card); }
  .task-card__body { display: flex; flex-direction: column; gap: var(--space-1); min-width: 0; }
  .task-card__actions { flex-shrink: 0; }
</style>
```
(核对:`PageHeader` 是否用 `{#snippet accessory()}` —— 见 `components/shell/PageHeader.svelte` 实际 props;若不同,按其 API 调整。CSS token 名核对 `styles/tokens.css`。)

- [ ] **Step 2: 加路由**

`apps/momo/src/routes.ts`:顶部 `import Tasks from "./pages/Tasks.svelte";`;在 `routes` 数组加(放在 `/apps` 那条附近):
```typescript
{ pattern: "/tasks", component: Tasks as Component<Record<string, unknown>>, hasShell: true, displayName: "Tasks" },
```

- [ ] **Step 3: 加 Sidebar 入口**

`apps/momo/src/components/shell/Sidebar.svelte`:在 lucide import(:213-225)加 `ListChecks`;在 `navEntries`(:259-263)加:
```typescript
{ label: "Tasks", icon: ListChecks, href: "/tasks", activePrefixes: ["/tasks"] }
```

- [ ] **Step 4: 跑页面测试(Task 5 的 tasks.spec.ts 应转 PASS)**

Run: `cd apps/momo && npm run test:desktop-ui -- tasks.spec.ts`
Expected: PASS(渲染出 "Review KYC" / "Telecom notice")。

- [ ] **Step 5: 类型检查**

Run: `cd apps/momo && npm run check`
Expected: 无类型错误。

- [ ] **Step 6: Commit**

```bash
git add apps/momo/src/pages/Tasks.svelte apps/momo/src/routes.ts apps/momo/src/components/shell/Sidebar.svelte
git commit -m "feat(momo): add Tasks page + /tasks route + sidebar entry

Momo-styled list of Telegram monitor tasks with Refresh + Ignore. Action
buttons (Assess/Summarize/Open) intentionally omitted (out of scope).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: 前端 — connect tg 成功后自动建 monitor

**Files:**
- Modify: `apps/momo/src/pages/ConnectedApps.svelte`(`refreshStatus`)
- Test: `apps/momo/tests/...`(扩展 connect 测,或手动 e2e)

- [ ] **Step 1: 写测试(telegram 翻转为 connected → 调 task_monitor_create)**

在 connect 相关 spec(参照现有 connector 测)里断言:fakeDaemon 收到 `task_monitor_create` 且 `params.connection_slug === "telegram-user"`。若现有测试框架不易断言 daemon 调用,本 step 改为在 `tasks.spec.ts` 加一个 case:模拟 telegram 从 not_connected → connected 后,fake daemon 记录到一次 `task_monitor_create`。

- [ ] **Step 2: 在 refreshStatus 里挂钩**

`apps/momo/src/pages/ConnectedApps.svelte`:import `createMonitor`:
```typescript
import { createMonitor } from "../lib/taskApi";
```
在 `refreshStatus()`(:41-55)里,记录 telegram 上次状态,检测"刚连上"再调 createMonitor(幂等,但避免每次刷新都调):
```typescript
let telegramWasConnected = false; // 模块/组件级状态

async function refreshStatus(): Promise<void> {
  try {
    const status = await getConnectorStatus();
    for (const app of apps) {
      if (app.connectorSlug === "telegram-login") {
        app.status = status.telegram ? "connected" : "not_connected";
      } else if (app.connectorSlug === "email") {
        app.status = status.email ? "connected" : "not_connected";
      }
    }
    // 刚从未连 → 已连:自动建 monitor(connection slug = telegram-user)
    if (status.telegram && !telegramWasConnected) {
      void createMonitor("telegram-user").catch((e) => console.warn("auto createMonitor failed", e));
    }
    telegramWasConnected = status.telegram;
  } catch {
    /* 保持现有降级 */
  }
}
```
(核对 `getConnectorStatus` 返回结构 `{ telegram: boolean; email: boolean }` —— 见 `connectorClient.ts`。`telegramWasConnected` 放组件顶层 `let`。)

- [ ] **Step 3: 跑测试**

Run: `cd apps/momo && npm run test:desktop-ui -- tasks.spec.ts`
Expected: PASS(daemon 收到 `task_monitor_create` with `telegram-user`)。

- [ ] **Step 4: 类型检查**

Run: `cd apps/momo && npm run check`
Expected: 无类型错误。

- [ ] **Step 5: Commit**

```bash
git add apps/momo/src/pages/ConnectedApps.svelte apps/momo/tests/tasks.spec.ts
git commit -m "feat(momo): auto-create Telegram monitor on connect

When Telegram flips to connected, fire createMonitor(\"telegram-user\")
(idempotent create-or-resume). No config UI — connecting starts
monitoring automatically.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: 端到端验证 + 回归

**Files:** 无新增,仅验证。

- [ ] **Step 1: 全量回归**

Run:
```bash
cd apps/momo && npm run check && npm run test:desktop-ui
cargo check --manifest-path apps/momo/src-tauri/Cargo.toml
cargo test --manifest-path apps/momo/src-tauri/Cargo.toml
```
Expected: 全绿。

- [ ] **Step 2: 手动 e2e(需真 puffer 二进制 + 真 tg 账号)**

1. `npm run tauri dev`(确保 `puffer` 在 PATH 或设 `MOMO_PUFFER_BIN`)。
2. 启动后,Tauri 应已 spawn `puffer daemon`(看 stderr 有无 daemon 日志)。
3. Connected Apps → 连 Telegram(走现有 connect 流程)→ 连成功后自动建 monitor。
4. 用另一个 tg 账号给你发一条明确像请求的消息(如"帮我看下这个文件")。
5. 打开 Tasks 页面 → 点 Refresh → 应出现一条 monitor task。
6. 点 Ignore → 该 task 消失/标记 ignored。
7. 验证 task 文件:`~/.puffer/runtime/claude_workflow/monitor_tasks.json` 有对应条目。

- [ ] **Step 3: 验证降级**

不启动 puffer(临时把 `MOMO_PUFFER_BIN` 指向不存在路径)→ 打开 Tasks 页面应显示 "Daemon not ready — ..." 而非崩溃/白屏。

- [ ] **Step 4: (无新代码则跳过 commit)** 如手动验证中发现并修了 bug,逐项 commit。

---

## Self-Review notes(已核对)

- **Spec 覆盖**:目标①daemon 接入=Task1-3;②connect 自动建 monitor=Task7;③Tasks 页面=Task6;④Ignore=Task5/6。非目标(chat 迁移/action 按钮/memory 编辑 UI/手动 New monitor/work-life/实时推送)均未引入。✅
- **类型一致**:`WorkflowMonitorTask`/`WorkflowSnapshot` 在 Task4 定义,Task5/6 一致引用;`ensureDaemonClient`(Task3)被 Task4 引用;`createMonitor("telegram-user")` 全程一致用 connection slug。✅
- **协议**:daemonClient 用 desktop envelope(type:request),与 momo 1431 wsClient(无 type)区分,已在多处标注。✅
- **风险**:风险④(并存破坏 chat)按 spec 不处理;降级(风险②)在 Task5/Task8-Step3 覆盖。✅
- **待实现者注意的核对点**(非 placeholder,是真实环境差异需现场确认):`PageHeader` 的 accessory snippet API、`fakeDaemon` 是否支持第二 ws、CSS token 名——均已在对应 step 标注"核对"。
