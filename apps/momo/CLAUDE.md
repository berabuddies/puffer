# Momo Desktop — 架构与上下文

> Standalone Tauri 桌面 GUI:puffer 驱动的 chat + WorldClaw U-card 钱包。
> Fork 自 `apps/puffer-desktop/src-v2/`(2026-05)。
> 负责人:sean。对话用中文,git 产物(commit/PR/代码注释)用英文。

## momo 与 puffer 的关系(最重要)

**一句话:momo 是 GUI 壳,puffer 是 agent。momo 自己不实现 LLM agent runtime,而是连一个常驻 `puffer daemon` 跑 agent。** momo 是 **puffer-only**(host 需要 `puffer` 在 `PATH`,或用 `MOMO_PUFFER_BIN`/`PUFFER_BINARY` 指定路径——daemon_launcher 会 spawn 它)。

### 两段通信链路(别混淆)

```
                         ┌─ wsClient.ts ──ws 1431──▶ momo backend: daemon_handshake / connectors / auth
[momo 前端 svelte] ──────┤                            (src-tauri/websocket.rs + backend.rs)
                         └─ daemonClient.ts ──直连 ws──▶ 常驻 puffer daemon: chat / session / task RPC
                                                          (daemon_launcher.rs spawn,workspace=$HOME → ~/.puffer)
```

1. **前端 ↔ momo backend**:WebSocket,端口 1431(`src-tauri/src/websocket.rs` 起 server,前端 `src/lib/wsClient.ts` 连)。**只剩 connectors / auth / `daemon_handshake`(返回 daemon 的 url+token)。chat 不再经此跑 turn。**
2. **前端 ↔ puffer daemon(直连)**:前端 `src/lib/daemonClient.ts` 经 backend 的 `daemon_handshake` 拿 `{url,token}` 后**直连常驻 daemon**(`crates/puffer-cli/src/daemon.rs`,WebSocket/NDJSON server),chat / session / workflow / task 全走 daemon RPC。daemon 由 `src-tauri/src/daemon_launcher.rs` spawn(`puffer daemon --print-handshake`,workspace cwd=$HOME → session/凭据落 `~/.puffer`),随 app 退出而 kill。

> **这是 2026-05-29 完成的架构迁移**:momo 从"backend spawn `puffer non-interactive` 一次性子进程 + 自管 `~/.momo` session"迁到"前端直连常驻 daemon"(照搬 puffer-desktop),分两阶段做(chat task #7)。详见 `docs/superpowers/specs|plans/2026-05-29-momo-chat-daemon-migration*`。

### chat 架构(已迁 daemon,照搬 puffer-desktop)

- **前端 `src/lib/agent/`**:`daemonChat.ts`(瘦 RPC 封装,直连 daemon)、`agentChat.svelte.ts`(chat 状态机 controller,**keyed by sessionId** 模块级 store,因路由 `{#key}` 会重挂 Agent 组件)、`sessionEvents.ts`(订阅 `session:<id>:event` + `SessionStreamEvent` 类型)、`normalize.ts`(timeline DTO 归一)、`ConversationView.svelte` + `components/`(从 desktop 照搬的渲染组件,**阶段1 暂用 desktop IDE 风格;阶段2 才改 momo 气泡 UI,渲染同一份 `TimelineItem[]`**)。
- **一次 chat 的流转**:`login_with_api_key`(灌 key,见下)→ `create_session({cwd: $MOMO_HOME/projects/default})` 拿 sessionId → 订阅 `session:<id>:event` → `run_agent_turn({sessionId, message, permissionMode:"workspace-write"})` 拿 turnId(fire-and-return)→ 事件流(`text-delta`/`tool-calls-requested`/`tool-invocations`/`thinking-delta`/`permission-request`/`user-question-request`/`turn-complete`/`turn-error`)→ `resolve_permission`/`resolve_user_question` → 收尾。历史 `load_session_detail`,列表 `list_grouped_sessions`(按 default cwd 过滤,否则 monitor session 会混入)。
- **session 由 daemon 管**(`~/.puffer`),前端只持 sessionId;momo 自管的 `~/.momo/sessions.json` 已废弃。create/rename 也走 daemon(`create_session`/`rename_session`),别再用 1431 的旧路径(否则新建/重命名的会话不在 daemon 列表里)。
- **⚠️ provider / key(最易踩)**:**daemon 没有 "puffer" 这个 provider**(内置只有 openai/anthropic/openrouter/xai/...)。worldrouter 推理 API 是 **OpenAI 兼容 chat/completions**,所以 key 走**内置 openai provider + `openai_base_url` override 指向 `https://inference-api.worldrouter.ai/v1`** + `default_model=gpt-5.4`:`login_with_api_key("openai", sk-worldrouter-key)` + `update_config({openaiBaseUrl, defaultProvider:"openai", defaultModel})`(`src/lib/agent/daemonAuth.ts`)。**绝不传 `providerId:"puffer"` 给 daemon**(create_session/run_agent_turn 都会被 `unknown provider` 拒)。worldrouter 的 mint-key control-api(`control-api.worldrouter.ai`)与推理 API 是两回事。缺 key 表现为异步 `turn-error`,不是 RPC 同步异常。
- **backend 旧 chat 路径已退役**:`backend.rs::run_agent_turn` 已 stub 成报错;`run_agent_turn_inner`/`codex_app_server.rs` 留作死代码(阶段2 删)。

### ⚠️ 两个已修 chat bug(2026-05-29;puffer-desktop 同源也有,改时别回归)

- **多轮 tool 调用错位**:`agentChat.svelte.ts::refreshSessionAfterTurn` 的 `currentTurnId` 守卫**不能在新 turn 已开始时整体放弃 refresh**——否则已完成 turn 的 live tool 不被对账清除、跨轮在 `liveStreamItems` 累积、`combinedTimeline` 拼末尾、`ConversationView.buildRows`(按 user 边界分组)把它们全归到最后一个 agent row。已改"窄路径"(更新 persisted + `reconcileCompletedTurnLiveItems` 只清已完成 turn 的 live,不碰运行中 turn)。复现测试 `tests/agent/multiturn-tool-grouping.spec.ts`。**daemon 的 `timeline_items` persisted 顺序本身是对的(pending_assistant 机制把 assistant 移到 tool 后),别去改 daemon。**
- **streaming 文本抖动**:`components/MessageBody.svelte` 别每个 text-delta 都全量重解析 markdown——半成品 markdown 逐帧在 纯文本/加粗/代码块 间翻转会导致 layout 跳动。已 **rAF 节流**(每帧最多解析一次,`onDestroy` cancel,不丢尾)+ blocks/inline `{#each}` 加 key + parseInline LRU memo。**节流放 MessageBody,不放 reducer**(reducer 须保持同步契约,否则破坏 reducer 测试)。配套:`Agent.svelte` 必须把 `turnStartedAtMs`/`turnThinking`/`turnStatusHint` 传给 ConversationView(否则计时器空转 + typing 恒显 "Running")。

### telegram subscriber 互踢事故(已随 chat 迁 daemon 根治)

> 历史背景(迁移前的真实事故,记录在此供回归排查):旧 `puffer non-interactive` chat 进程**跑完不退出**(装了 subscription manager + 起 telegram subscriber 长连接),每次聊天 spawn 一个新的 → 多个 `__subscriber telegram-user` 并存、同连一个 telegram 账号 → MTProto 会话互踢 → monitor 收不到新消息、别人 @ 你也不触发 task(connection 仍显示 `active`、Tasks 页只剩旧 task,极具迷惑性)。这是 spec 风险④从"已接受"变"必须做"的实证。
> - **现已根治**:chat 迁 daemon 后全程只有**一个常驻 daemon** 连 tg,争抢消失。
> - **回归排查**(万一又出现):`ps -Ao pid,command | grep -i 'puffer __subscriber'` 正常应只有 **1 个**(momo daemon 起的);若见多个 → `pkill -f 'puffer non-interactive'; pkill -f '\.cargo/bin/puffer __subscriber'` 后**重启 momo app**(daemon liveness 是 no-op,杀 daemon 不自愈,要重启整个 app)。

## puffer 提供的能力边界(开发新功能前先看这)

puffer daemon(= puffer-core runtime)暴露的 RPC 分组(定义见 `crates/puffer-cli/src/daemon.rs` 的 `match request.method`):

- **session**:`list_grouped_sessions` / `load_session_detail` / `rename_session` / `delete_session` / `set_session_tags` / `create_session`
- **project**(注意:puffer 里 project 不是一等实体,只是 cwd 分组 + `project_metadata.json` tag):`delete_project` / `set_project_tags`
- **workflow**:`workflow_list`(返回快照,含 `workflows` / `runs` / `tasks` / `monitor_tasks` / `connectors` / `connections`) / `workflow_save` / `workflow_binding_create|delete` / `workflow_connection_delete` / `workflow_toggle` / `workflow_runs_list` / `workflow_run_show`
- **task / monitor**(别名:`monitor_*` == `task_monitor_*`):`task_monitor_create` / `task_monitor_ignore` / `task_monitor_memory_save`。**task 列表没有独立 RPC,从 `workflow_list` 的 `tasks[]` / `monitor_tasks[]` 取。**
- **agent turn**:`run_agent_turn` / `dispatch_slash_command` / `cancel_turn` / `resolve_permission` / `resolve_user_question`
- **凭据 / 配置**:`login_with_api_key` / `login_with_oauth` / `logout_provider` / `update_config`(`openai_base_url`/`default_provider`/`default_model` 等) / `load_settings_snapshot`
- **文件 / 其它**:`read_file` / `write_file` / `list_dir` / pty / browser / lsp / mcp 等

### skill 系统(装 / 更新 / 移除)

puffer 原生 skill(`<name>/SKILL.md` + builtin/user/workspace 三层加载 + `resources/plugins/puffer-builtins.yaml` 注册)。**注意触发是「软提示」**:skill 加载且进 system prompt ≠ 模型一定调用——worldrouter 的 native `web_search` 会压过「找/查」类意图,需用 web_search 做不到的意图措辞(如「打电话/预订」)才触发。完整的安装 / 更新 / 移除流程与坑(尤其**删除内置 skill 必须重编译**,嵌入副本删目录删不掉)见 [`docs/architecture/skills.md`](../../docs/architecture/skills.md)。momo 已内置 `book-by-phone`(搜索本地商家 + AI 电话预订/取消/改期/咨询;走 WorldRouter API,认证/计费记在用户自己的 WorldRouter 账号——**需 env `WORLDROUTER_API_KEY` + `WORLDROUTER_BASE_URL`,无第三方 token**)。

### 两个跨概念的坑(已踩过)

- **momo 只有一个固定 default project,puffer task 也不按它分组**。momo 把所有会话收在单一 default project 下(`backend.rs::list_projects`,cwd = `$MOMO_HOME/projects/default`;Work/Life 分类已于 2026-05 移除)。puffer task 的 `task_scope` 是 `workspace` / `session:<id>` / `team:<id>` / `monitor`,与 momo 的 project 无关。要做"首页 task 列表",momo 拿到 `tasks[]` 后按 `session:<id> → session.cwd → 是否命中 default project` 自己 roll up。task 由 puffer-core agent 用 `TaskCreate` 工具产生。
- **memory 是 project 级,没有"用户级全局 memory"**。puffer 有 project `MEMORY.md`(`~/.puffer/projects/<slug>/MEMORY.md`,需在 `~/.puffer/projects.toml` 注册;puffer-core 跑 agent 时**按 cwd 自动注入**上下文)。没有专门的"写 memory" RPC,但可用 `write_file` 把 md 落到 `MEMORY.md` 或 cwd 的 `AGENTS.md`(agent 启动读取)。onboarding 国家/职业这类用户画像若要"对该用户所有对话生效",因 momo 所有对话都在单一 default cwd 下,写进该 project 即可覆盖;真正跨 cwd 的全局画像 puffer 目前没有,需向 puffer 提需求。

## 存储与端口

- **chat session / 凭据 → daemon 的 `~/.puffer`**(daemon workspace=$HOME):session transcript、`auth.json`(provider key)、monitor/connection、runtime。**这是 chat 的真值源,前端只持 sessionId。**
- **momo 自己的 `~/.momo`**(覆盖用 `MOMO_HOME`):`config.json` / `credentials.json`(WorldRouter key 缓存) / `permissions.json` / `pins.json` / `projects/default`(default project cwd)。**`sessions.json` 自管 transcript 已废弃**(chat session 迁 daemon)。
- 端口:Vite 1466 / Tauri WS backend 1431 / OAuth loopback 1457(与 V1 Corbina 共用,同时只能一个 app 监听)。
- Auth:momo 有自己的 auth(WorldRouter Auth Station,`VITE_AUTH_STATION_URL`),OAuth 走 OS browser → `http://localhost:1457/callback`;登录后 mint 的 `sk-worldrouter-…` key 经 daemon `login_with_api_key("openai")` 落 `~/.puffer/auth.json`(详见上 chat 架构 provider/key)。

> **⚠️ 在普通浏览器里登录走不通(只有桌面 app 能登)**:web 路径下 `goToLogin()`(`src/lib/auth.svelte.ts:641-643`)用 `window.location.origin` 当 `redirect_uri`,即 **`http://localhost:1466/auth/callback`**;但 Auth Station 的 `ALLOWED_REDIRECT_ORIGINS` **没有 1466**(实测含 `1456`/`1457`/`3000-3002`/`3011`)。桌面 app 走 `1457` loopback(在白名单)所以正常;浏览器走 1466 会**静默失败**——auth station 不报错、照样 307 跳 WorkOS,但**把 `redirect_uri` 从签名 `state` 里删掉** → 登录成功后 fallback 到 `return_to:"/"`(落 auth 自己域名),token 永远回不到 1466,表现为"点登录登一圈回不来"。
>   - **判白名单别看 HTTP 码**(所有端口都 307):探 `/login?redirect_uri=http://localhost:<port>/auth/callback&client_state=x` 的 302 `state=`,base64url decode 看 JSON 里 `redirect_uri` 是否被保留。**auth-deploy skill 文档里的白名单已过期(漏了 1456/1457),判真值要 live 探测。**
>   - **浏览器调试 UI/登录的临时解(不改任何文件)**:`./node_modules/.bin/vite --host 127.0.0.1 --port 1456 --strictPort`,浏览器开 `localhost:1456` 即可走通(1456 在白名单)。
>   - **永久修**(二选一,今天没做):① 把 dev 端口 1466→1456,需同步改 `package.json`/`vite.config.ts`/`src-tauri/tauri.conf.json`/`playwright.config.ts` 并重启 vite+桌面 app;② 走 `auth-deploy` skill 给 `ALLOWED_REDIRECT_ORIGINS` 加 `http://localhost:1466` 并 redeploy。生产正式登录始终走桌面 app `1457`,与 vite 端口无关。
>   - 溯源:1456 是 puffer-desktop 时代 vite 端口(`src-tauri/src/oauth_listener.rs:20` 残留注释),迁 momo 时 vite 挪到 1466 却没同步给白名单加 1466——这就是不一致根源。

## 关键文件

**chat(直连 daemon)**
- `src/lib/daemonClient.ts` — 前端直连 daemon 的通用 RPC client(`ensureDaemonClient()` 单例 + `request`/`on`)
- `src/lib/agent/` — `daemonChat.ts`(chat RPC)、`agentChat.svelte.ts`(状态机 controller)、`sessionEvents.ts`、`normalize.ts`、`daemonAuth.ts`(worldrouter→openai login)、`ConversationView.svelte` + `components/*`(渲染)
- `src/pages/Agent.svelte` — chat 页(挂 ConversationView + controller);`src/components/shell/Composer.svelte` — 输入(接 controller)
- `src-tauri/src/daemon_launcher.rs` — spawn 常驻 daemon + handshake
- `src-tauri/src/backend.rs` — 1431 RPC handler(`daemon_handshake` / connectors / auth;`run_agent_turn` 已 stub)
- `src-tauri/src/websocket.rs` — 前端↔backend 的 ws server

**其它**
- `src-tauri/src/connectors.rs` — Connected Apps / connector
- `src/lib/wsClient.ts` — 前端↔1431 backend ws 客户端
- `src/lib/sessionStore.svelte.ts` / `projectStore.svelte.ts` — sidebar 会话列表(读 daemon `list_grouped_sessions` 按 default cwd 过滤)/ 单一固定 default project

**死代码(阶段2 删)**:`src/lib/chat.svelte.ts`(旧 chat 状态机)、`src/components/agent/*`(旧气泡组件)已移除;`backend.rs::run_agent_turn_inner` / `codex_app_server.rs` 仍在但不再被 dispatch。

## 验证

```bash
npm run check
npm run test:desktop-ui
cargo check --manifest-path src-tauri/Cargo.toml
```
