# Momo Desktop — 架构与上下文

> Standalone Tauri 桌面 GUI:puffer 驱动的 chat + WorldClaw U-card 钱包。
> Fork 自 `apps/puffer-desktop/src-v2/`(2026-05)。
> 负责人:sean。对话用中文,git 产物(commit/PR/代码注释)用英文。

## momo 与 puffer 的关系(最重要)

**一句话:momo 是 GUI 壳,puffer 是 agent。momo 自己不实现 LLM agent runtime,而是把 `puffer` CLI 当 agent 跑。** momo 是 **puffer-only**(host 需要 `puffer` 在 `PATH`,或用 `MOMO_PUFFER_BIN` 指定路径)。

### 两段通信链路(别混淆)

```
[momo 前端 svelte]  --WebSocket(ws://127.0.0.1:1431/ws)-->  [momo Tauri backend]  --spawn 子进程-->  [puffer CLI = agent]
        wsClient.ts                                          src-tauri/websocket.rs + backend.rs            puffer-core runtime
```

1. **前端 ↔ momo backend**:WebSocket,端口 1431(`src-tauri/src/websocket.rs` 起 server,前端 `src/lib/wsClient.ts` 连)。JSON-RPC 协议 mirror 自 puffer-desktop 的 `websocket.rs`。**这层 ws 是 momo 内部的,不是连 puffer。**
2. **momo backend ↔ puffer(agent)** — ⚠️ **当前实现违反下方设计原则,是待改造的技术债**:`backend.rs::run_agent_turn_inner` 直接 **spawn puffer CLI 子进程**(`Command::new("puffer")`,backend.rs:801-833 / 1031-1044),`provider=puffer` 跑 `puffer non-interactive --user-message <msg> --json-events`(一次性 stdio,ndjson:`text-delta` / `tool-invocation` / `turn-complete`)。默认 `DEFAULT_PROVIDER="codex"`,codex/claude/puffer 三个分支都在(codex 另走 `codex_app_server.rs`);README 产品定位是 puffer-only。**没有连 `puffer daemon`、没有朝向 puffer 的 ws client**(`tungstenite` 仅用于第 1 条那个对前端的 server)。

### 跟 puffer-desktop 的关键差异

puffer-desktop(同源 fork)走的是**另一条路**:它的 `src-tauri/daemon_launcher.rs` 会 spawn 一个常驻 **`puffer daemon`**(本身是 WebSocket/NDJSON server,`crates/puffer-cli/src/daemon.rs`),连 `ws://127.0.0.1:<port>/ws`,所有功能走 **daemon RPC**(`workflow_list` / `delete_session` / `set_session_tags` / `delete_project` / `set_project_tags` / `monitor_*` / `run_agent_turn` …)。

**momo 当前没有 `daemon_launcher`、不连 `puffer daemon`**,而是上面的一次性 `puffer non-interactive`。所以 puffer 团队说的"参考 desktop 实现"= 参考 desktop 连 daemon + 调那批 RPC 的模式。

### 🎯 设计原则与改造方向(sean 已明确,优先级高)

**原则:momo 不直接依赖 puffer CLI,而是通过 WebSocket 与 puffer(daemon)通信。**

- **现状(已核实 2026-05,违反原则)**:momo 直接 spawn `puffer non-interactive`(见上),用不到 daemon 的 `run_agent_turn` / `workflow_list` / session/project/task 那批 RPC。
- **目标**:momo backend 改为连 `puffer daemon` 的 ws,agent turn 及所有功能走 daemon RPC,与 puffer-desktop 一致。
- **可复制样板**:`apps/puffer-desktop/src-tauri/src/daemon_launcher.rs`(momo 同源 fork)。已实现 spawn `puffer daemon` 子进程 → 解析 handshake(ws URL + token)→ 连 `ws://127.0.0.1:<port>/ws`。momo `Cargo.toml` 已有 `tungstenite`,可直接用其 `connect`。
- **改造落点**:`backend.rs::run_agent_turn_inner`(:749)的 spawn+stdio 逻辑(:801-932)在 puffer 分支替换为 ws RPC 收发;新增连 daemon 模块;再决定 session/workflow/task 是否从自管的 `puffer-session-store` 迁到 daemon RPC。
- 一旦改成连 daemon,本文件后续"puffer 提供的能力边界"那批 RPC 即可直接调用。

## puffer 提供的能力边界(开发新功能前先看这)

puffer daemon(= puffer-core runtime)暴露的 RPC 分组(定义见 `crates/puffer-cli/src/daemon.rs` 的 `match request.method`):

- **session**:`list_grouped_sessions` / `load_session_detail` / `rename_session` / `delete_session` / `set_session_tags` / `create_session`
- **project**(注意:puffer 里 project 不是一等实体,只是 cwd 分组 + `project_metadata.json` tag):`delete_project` / `set_project_tags`
- **workflow**:`workflow_list`(返回快照,含 `workflows` / `runs` / `tasks` / `monitor_tasks` / `connectors` / `connections`) / `workflow_save` / `workflow_binding_create|delete` / `workflow_connection_delete` / `workflow_toggle` / `workflow_runs_list` / `workflow_run_show`
- **task / monitor**(别名:`monitor_*` == `task_monitor_*`):`task_monitor_create` / `task_monitor_ignore` / `task_monitor_memory_save`。**task 列表没有独立 RPC,从 `workflow_list` 的 `tasks[]` / `monitor_tasks[]` 取。**
- **agent turn**:`run_agent_turn` / `dispatch_slash_command` / `cancel_turn` / `resolve_permission` / `resolve_user_question`
- **文件 / 其它**:`read_file` / `write_file` / `list_dir` / pty / browser / lsp / mcp / 凭据 等

### 两个跨概念的坑(已踩过)

- **task 的分组维度 ≠ work/life**。puffer task 的 `task_scope` 是 `workspace` / `session:<id>` / `team:<id>` / `monitor`,**没有 work/life**。work/life 是 momo 自己的固定 project(`backend.rs::list_projects`,cwd = `$MOMO_HOME/projects/work|life`)。要做"首页 work/life task 列表",需 momo 拿到 `tasks[]` 后按 `session:<id> → session.cwd → 命中哪个 project` 自己 roll up。task 由 puffer-core agent 用 `TaskCreate` 工具产生。
- **memory 是 project 级,没有"用户级全局 memory"**。puffer 有 project `MEMORY.md`(`~/.puffer/projects/<slug>/MEMORY.md`,需在 `~/.puffer/projects.toml` 注册;puffer-core 跑 agent 时**按 cwd 自动注入**上下文)。没有专门的"写 memory" RPC,但可用 `write_file` 把 md 落到 `MEMORY.md` 或 cwd 的 `AGENTS.md`(agent 启动读取)。onboarding 国家/职业这类用户画像若要"对该用户所有对话生效",因 momo 所有对话都在 work/life 两个 cwd 下,写进这两个 project 即可覆盖;真正跨 cwd 的全局画像 puffer 目前没有,需向 puffer 提需求。

## 存储与端口

- App home:`~/.momo`(覆盖用 `MOMO_HOME`)。`sessions.json`(会话元数据+transcript) / `config.json` / `credentials.json` / `permissions.json` / `pins.json`。**momo 自管 session(`puffer-session-store`),不是 puffer 的 `~/.puffer`。**
- 端口:Vite 1466 / Tauri WS backend 1431 / OAuth loopback 1457(与 V1 Corbina 共用,同时只能一个 app 监听)。
- Auth:momo 有自己的 auth(WorldRouter Auth Station,`VITE_AUTH_STATION_URL`),OAuth 走 OS browser → `http://localhost:1457/callback`。

## 待验证 (TODO)

- [x] **worldrouter API key mint 端到端 —— 已实测通过(2026-05-29)。** 在隔离 clone + 全新 `MOMO_HOME` + 清空 webview localStorage 下走完整登录(点登录 → 系统浏览器 OAuth → control-api 两跳 mint),`credentials.json` 写入了**与日常不同的全新 key**(证明是本次 mint 产出、非缓存继承),并用该 key 驱动 `puffer non-interactive` 正常聊天出回复;`auth.svelte.ts:388-393` 担心的 CORS/未测试风险在当前 `auth.worldrouter.ai` + control-api 环境下**不成立**。原始背景如下:登录拿到 Auth Station JWT 后,前端 fire-and-forget 两跳换 `sk-worldrouter-…` key,再经 `login_with_api_key` RPC 写入 `~/.momo/credentials.json`,聊天时注入 `PUFFER_API_KEY`(链路:`src/lib/auth.svelte.ts:395-520`、`backend.rs:817-827`)。但 `auth.svelte.ts:388-393` 注释自标 **"截至 2026-05-26 未测试 + CORS 风险"**(control-api 是 backend-to-backend,浏览器 fetch 可能被 preflight 拦)。**失败时登录仍假成功并进首页,但聊天因缺 key 失败、UI 无任何提示(仅 console.warn)** —— 这是登录→聊天链路上唯一未验证的环节,也是 QA 最难自诊断的坑。需实测一次完整链路:登录 → mint 成功 → `credentials.json` 含 puffer key → 发消息能收到回复。

## 关键文件

- `src-tauri/src/backend.rs` — RPC handler + `run_agent_turn`(spawn provider 进程)
- `src-tauri/src/codex_app_server.rs` — codex provider 桥接
- `src-tauri/src/websocket.rs` — 前端↔backend 的 ws server
- `src-tauri/src/connectors.rs` — Connected Apps / connector
- `src/lib/wsClient.ts` — 前端 ws JSON-RPC 客户端
- `src/lib/chat.svelte.ts` — chat 状态机(turn / tool pills / askUserQuestion)
- `src/lib/sessionStore.svelte.ts` / `projectStore.svelte.ts` — session / 固定 Work·Life project

## 验证

```bash
npm run check
npm run test:desktop-ui
cargo check --manifest-path src-tauri/Cargo.toml
```
