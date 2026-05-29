# Momo Tasks (Telegram monitor) — Design Spec

- Date: 2026-05-29
- Author: sean (with Claude)
- Status: Approved for planning
- Scope owner: momo desktop (`apps/momo`)

## 1. 背景与目标

puffer-desktop 已验证可跑通的闭环:**Connectors 连 Telegram → 自动/手动建 monitor → 别人在 tg 给你发消息/@你 → 后台 triage agent 判定为 actionable → 生成 monitor task → Tasks 页面展示并可管理**。本设计把这条闭环移植到 momo desktop。

**业务/数据流照搬 puffer-desktop;UI 用 momo 自己的设计风格,并隐藏 "config" 交互(connect 后自动配置)。**

### 目标(本次范围)

1. momo 接入常驻 `puffer daemon`(基础设施,前端直连 daemon ws,完全参考 puffer-desktop)。
2. 用户在 Connected Apps 连上 Telegram 成功后,**自动**建立 monitor(无 config UI)。
3. 新增 momo 风格的 **Tasks 页面**,展示 telegram monitor task 列表。
4. 单条 task 支持 **Ignore**。

### 非目标(明确排除,记为后续)

- **chat (`run_agent_turn`) 迁移到 daemon** —— 本次 chat 仍走 `spawn puffer non-interactive`(过渡态)。sean 已确认 chat 后续同样迁 daemon(见 task #7)。
- task 的 **action 按钮**(Assess impact / Summarize / Draft follow-up …)与 **Open** —— 不渲染,留到 chat 迁 daemon 时一起做(它们要跑 agent turn)。
- **Monitor memory 编辑 UI** —— 隐藏;monitor memory 用 `task_monitor_create` 初始化的默认值。
- **手动 "New monitor / Task configuration" modal** —— 隐藏,改为 connect 后自动建。
- task 与 momo **work/life project 的关联/分组** —— Tasks 是独立全局列表(monitor task 归属 connection,不归属 work/life)。
- task 列表**实时事件推送** —— MVP 用拉取 + 手动 Refresh。

## 2. 架构(子方案 1:前端直连 daemon,参考 puffer-desktop)

```
momo 前端 ──ws 1431──▶ momo backend          (chat: 仍走 backend spawn `puffer non-interactive`,过渡)
   │                      └─ daemon_launcher: spawn `puffer daemon` + 取 handshake(url+token)
   └──daemon ws client(直连)──────────────────▶ 常驻 puffer daemon   ← task RPC 直接发这里
                                                   ▲ 监听 tg → triage agent → TaskCreate
                                                   └─ ~/.puffer/runtime/claude_workflow/monitor_tasks.json
```

- **daemon workspace**:user-level(让 monitor memory 落 `~/.puffer/runtime/monitors/`、task 落 `~/.puffer/runtime/claude_workflow/monitor_tasks.json`),与 momo 已连的 tg(`~/.puffer/connections.json`)、与 puffer-desktop 一致。
- **前端直连 daemon**:与 desktop 同构。"两个 ws 并存(backend 1431 + daemon)"是过渡态;chat 迁 daemon 后,backend 的 non-interactive 路径逐步退役。

### 与 puffer-desktop 的唯一适配差异

desktop 前端用 tauri `invoke` 拿 daemon handshake;momo 前端走 ws(1431),所以 backend 需**新增一个 ws 方法 `daemon_handshake`** 把 `{url, token}` 暴露给前端,前端拿到后再开 ws 直连 daemon。其余(daemon client、task API、类型)直接抄 desktop。

## 3. 组件拆分

### Backend (Rust, `apps/momo/src-tauri/src`)

| 组件 | 说明 | 样板 |
|---|---|---|
| `daemon_launcher.rs`(新增) | 启动时 spawn `puffer daemon --print-handshake`(user-level workspace)、解析 handshake、随 app 退出 kill。**裁掉 remote/SSH 部分。** | 抄 `apps/puffer-desktop/src-tauri/src/daemon_launcher.rs`:`ensure_started`(:88)、`spawn_daemon`(:239,`.arg("daemon")`:246 / `--print-handshake`:249)、`local_url`(:201)、`default_workspace_cwd`(:289);**不抄** `connect_remote`/`spawn_remote_daemon`(:311+) |
| `backend.rs`/`websocket.rs` 新增 ws 方法 `daemon_handshake` | 返回 `{ url, token }` 给前端 | momo `BackendState::handle` dispatcher |
| 生命周期接线 | 在 `lib.rs` app setup 里启动 daemon_launcher(类似现有 `websocket::start_backend_ws`) | momo `src-tauri/src/lib.rs` |

### 前端 (Svelte/TS, `apps/momo/src`)

| 组件 | 说明 | 样板 |
|---|---|---|
| `lib/daemonClient.ts`(新增) | 先用现有 `wsClient`(1431)调 `daemon_handshake` 拿 handshake,再开 ws 直连 daemon;提供 `request(method, params)` + lazy connect/重连 | 抄 `desktop.ts` 的 `DaemonClient`/`ensureLocalDaemonClient` |
| task API(新增,在 daemonClient 之上) | `loadWorkflowSnapshot()`→`workflow_list`;`createMonitor(slug)`→`task_monitor_create`;`ignoreMonitorTask(taskId, reason?)`→`task_monitor_ignore` | `desktop.ts:1133-1163` |
| 类型 | `WorkflowSnapshot`/`WorkflowMonitorTask`/`WorkflowMonitorMemory` 子集 | `puffer-desktop` `lib/types.ts:428-521` |
| `lib/taskStore.svelte.ts`(新增) | 拉 `snapshot.monitor_tasks[]`,暴露给页面;Refresh 动作 | momo `sessionStore.svelte.ts` 风格 |
| Tasks 页面(新增,momo 风格) | 列表渲染每条 task(标题/描述/status/来源/时间)+ Ignore 按钮 + Refresh;**不渲染 action 按钮**;空态/daemon 未就绪态 | momo `components/` 设计语言 |
| 路由 + Sidebar 入口 | `/tasks` 路由 + Sidebar `navEntries` 加一项 | momo `router.svelte` + `components/shell/Sidebar.svelte`(navEntries ~:86) |
| connect 自动建 monitor | Connected Apps 连 tg 成功后,前端调 `createMonitor("telegram-user")`(幂等,create-or-resume,无 UI) | momo `components/apps` + `connectorClient.ts` |

## 4. 数据流

1. **启动**:app 起 → backend daemon_launcher spawn `puffer daemon`(user workspace)→ 取 handshake → 就绪。
2. **拿 handshake**:前端 daemonClient 首次 request 时,经 backend ws(1431)`daemon_handshake` 取 `{url, token}` → 开 ws 直连 daemon。
3. **connect 自动建 monitor**:用户连 tg 成功 → 前端 `createMonitor("telegram-user")` → daemon `task_monitor_create` 建 binding + 起 tg 监听子进程。
4. **task 触发**:别人 tg 发消息/@你 → daemon 内 tg 监听 → triage agent 跑一轮 → `TaskCreate` → `~/.puffer/runtime/claude_workflow/monitor_tasks.json`。
5. **展示**:Tasks 页面 → `loadWorkflowSnapshot()`(`workflow_list`)→ `monitor_tasks[]` → momo 风格渲染。
6. **Ignore**:点 Ignore → `ignoreMonitorTask(taskId)`(`task_monitor_ignore`)→ 刷新列表。

## 5. task 刷新方式(MVP)

- 打开 Tasks 页面拉一次 + **Refresh 按钮**;可选 ~15s 轻量轮询。
- 实时事件推送(daemon `subscribe_event` + replay)留作后续增强。

## 6. 关键决策记录

- **子方案 1(前端直连 daemon)而非 backend 代理**:sean 决定完全参考 desktop;chat 后续同样迁 daemon,前端直连是最终形态,避免引入"backend 代理"这种将来要拆的中间态。
- **隐藏 config、connect 后自动建 monitor**:momo 面向终端用户,"建 monitor"是技术概念,不暴露;连上 tg 即自动开始监控。
- **Ignore 保留**:给用户 dismiss 单条 task 的手段;属 task 级操作,非 config。
- **风险④接受破坏 chat**:见下。

## 7. 风险与未决

- **(已接受,不缓解)non-interactive 与 daemon 同连 tg 冲突**:常驻 daemon 有 enabled monitor binding → 起 tg 监听子进程(连你的 tg account);chat 的 `non-interactive` 跑 turn 时也装 subscription manager、读同一份 `~/.puffer` binding,可能 autostart 同一个 tg subscriber → 两个进程同连同一 telegram account(grammers session),有互踢/锁冲突风险。**sean 已确认接受过渡期 chat 可能被破坏**(chat 紧接着迁 daemon,届时自然消解)。本次不投入隔离工作量。
- **(需实现时确认)daemon 就绪时序**:Tasks 页面/自动建 monitor 在 daemon 未就绪时应降级(显示"连接中"/重试),不报错崩溃。
- **(需实现时确认)momo daemon 的 user-level workspace 解析**:确保 `task_monitor_create` 与 momo 已连的 `telegram-user` connection、monitor task 落在同一 `~/.puffer`,否则建的 monitor 看不到已连的 tg。

## 8. 测试

- 后端:`daemon_launcher` spawn/handshake 解析单测;daemon 连接失败降级。
- 前端:`taskStore` 渲染/Ignore 组件测(复用 `npm run test:desktop-ui`)。
- 端到端手动:连 tg → 自动建 monitor → 另一账号发消息 → Tasks 页面出现 task → Ignore 生效。
- 回归:`npm run check` / `cargo check --manifest-path src-tauri/Cargo.toml`。

## 9. 后续(out-of-scope,已记 todo)

1. chat (`run_agent_turn`) 迁 daemon(task #7)。
2. task action 按钮 / Open(跑 agent turn)。
3. Monitor memory 编辑 UI。
4. task 实时事件推送。
