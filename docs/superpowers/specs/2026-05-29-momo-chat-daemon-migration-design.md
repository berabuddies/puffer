# Momo Chat 迁 daemon（照搬 puffer-desktop 聊天）— Design Spec

- Date: 2026-05-29
- Author: sean (with Claude)
- Status: Approved for planning
- Scope owner: momo desktop (`apps/momo`)
- 关联前置: `docs/superpowers/specs/2026-05-29-momo-tasks-monitor-design.md`（task monitor 已落地 daemon 基础设施，本 spec 是其规划中的 task #7）

## 1. 背景与目标

momo 当前的 chat 走 `wsClient(1431) → momo backend → spawn 'puffer non-interactive' 一次性子进程`，session 由 momo 自管 `~/.momo/sessions.json`，是过渡态技术债。puffer-desktop 走的是 `前端直连常驻 puffer daemon`，全功能走 daemon RPC。task monitor 已把 daemon 基础设施（`daemon_launcher.rs` / `daemonClient.ts` / `daemon_handshake` RPC）落地并提交。

本 spec 把 momo 的 chat 迁到「前端直连 daemon」，照搬 desktop 的聊天实现。

### 方案与阶段（方案 B，分两阶段）

> 决策：先把 desktop 聊天前端**原样搬进 momo + 直连 daemon 跑通**（UI 暂用 desktop 风格），跑通后再改 momo 气泡。把「迁 transport」和「改 UI」拆成两个独立阶段，避免一边换底座一边改皮肤的复合风险。

- **阶段 1（本 spec 范围）**：desktop 聊天前端搬进 momo + 直连 daemon，跑通 text/tool/thinking + askUserQuestion + 权限审批 Approval + diff。UI 暂用 desktop 风格的 `ConversationView`。
- **阶段 2（后续 spec，out of scope）**：把 `ConversationView` 换成基于 momo `ChatBubble`/`Mascot` 的气泡 UI，渲染同一份 `TimelineItem[]`。

## 2. 决策记录（已敲定）

1. **session 整迁 daemon 管**：弃用 `~/.momo/sessions.json`，chat session 用 daemon 的 `create_session`/`list_grouped_sessions`/`load_session_detail`，transcript 落 `~/.puffer`，前端只持 sessionId。
2. **完整照搬行为**：text-delta / tool / thinking + askUserQuestion + 权限审批 Approval + diff 全做。
3. **provider/key 走 daemon**：worldrouter key 经 daemon `login_with_api_key` 写 `~/.puffer/auth.json`，不再注 env。详见 §4。
4. **默认 `permissionMode = "workspace-write"`**：default project cwd 内写文件/常规操作自由，越界（cwd 外、删除、危险 shell）才弹 Approval。
5. **旧 `~/.momo/sessions.json` 会话丢弃**：不显示、不导入（与 `cc2fc328` 让 legacy work/life 会话「留盘脱离 rail」的既定方向一致）。
6. **前端直连 daemon（不走 1431 跑 chat）**：复用 task monitor 的 `daemonClient`/`ensureDaemonClient()`，与 desktop 同构。

## 3. 架构与数据流

```
momo 前端
  ├─ wsClient(1431) → momo backend:  daemon_handshake / connectors / auth   (chat 不再经此跑 turn)
  └─ daemonClient(直连 puffer daemon, 已有):
        login_with_api_key  ·  update_config  ·  create_session  ·  run_agent_turn  ·  cancel_turn
        load_session_detail ·  list_grouped_sessions  ·  resolve_permission / resolve_user_question
        订阅 session:<id>:event
```

一次 chat 的流转：
1. **登录灌 key**（§4）：前端 mint worldrouter key → `daemonClient.request("login_with_api_key", {providerId:"openai", apiKey})` + `update_config` 设 `openai_base_url` / `default_model`。
2. **开会话**：`ensureDaemonClient()` →（新会话）`create_session({cwd: <default project cwd>})` 拿 sessionId → 订阅 `session:<id>:event`。
3. **发消息**：`run_agent_turn({sessionId, message, permissionMode:"workspace-write"})` → 拿 turnId（fire-and-return）。
4. **事件流**（controller 分发）：`text-delta`→助手 · `tool-calls-requested`/`tool-invocations`→ToolCard · `thinking-delta`→思考 · `permission-request`→Approval · `user-question-request`→QuestionPrompt · diff→DiffCard · `turn-complete`/`turn-error`→收尾；按 `replay:true` 去重。
5. **历史**：切会话 `load_session_detail(sessionId)` 回放 timeline。**列表/sidebar** `list_grouped_sessions` 按 default cwd 过滤（§5e、§8-B3/M2）。**中断** `cancel_turn(turnId)`。

数据流细节以 `apps/puffer-desktop` 为权威样板，daemon RPC schema 见关联调研（daemon.rs `match request.method`）。

## 4. worldrouter key 接入（解决审查 B1/B2）

> 审查发现：daemon 没有 "puffer" provider（"puffer" 只是 momo backend 自创的 id）；`PUFFER_API_KEY` 在 puffer CLI 里是死代码。所以现有 worldrouter key 链路从没真正驱动过 agent（CLAUDE.md:61 标的 TODO）。本 spec 第一次真正打通它。

worldrouter 推理 API（团队已验证）：**OpenAI 兼容 chat/completions**，base_url `https://inference-api.worldrouter.ai/v1`，model 如 `gpt-5.4`，`Authorization: Bearer <key>`。与 mint-key 的 control-api（`control-api.worldrouter.ai`）是两套东西。

接法（不新写 provider，复用 daemon 内置能力）：
- **provider**：用 OpenAI 兼容形态接入。daemon `update_config` 支持 `openai_base_url`（daemon.rs:1864），`apply_openai_base_url_override`（registry.rs:64）在启动/配置时把 base_url 指向 worldrouter。
- **登录**：`login_with_api_key("openai", <sk-worldrouter-key>)` 写 `~/.puffer/auth.json`；`update_config` 设 `openai_base_url = https://inference-api.worldrouter.ai/v1` 与 `default_model = gpt-5.4`。
- **谁/何时写**：momo 登录链路拿到 key 后，经 daemonClient 调上述 RPC（替代现在写 `~/.momo/credentials.json` + 注 env）。落点：onboarding/auth 成功回调。
- **缺 key 表现**：`run_agent_turn` fire-and-return，缺 key 时异步 `turn-error`（非 RPC 同步异常）→ controller 渲染错误 + 提示登录。这修掉现有「登录假成功、聊天静默失败」的坑。

> 验证项（实现首个 turn 时）：确认 daemon 以 OpenAI 兼容 wire 打到 worldrouter `/chat/completions` 能跑通（团队已验证 worldrouter API 可用）。若内置 openai provider 的默认 wire 需调整为 chat-completions 形态，按内置 `openrouter` provider（`default_api: openai-completions`）的配置形态对齐。此为实现细节，不阻塞 spec。

## 5. 前端搬运（阶段 1 核心）

新增代码统一落 `apps/momo/src/lib/agent/`，避免与 momo 现有 `components/agent/*` 撞名。

### 5a. 瘦 RPC 层 `lib/agent/daemonChat.ts`（修正审查 M1）
> 审查发现：desktop `desktop.ts`（2000+ 行）每个 chat 函数带 `canInvokeTauri()`/`invoke()` fallback + browser-preview 兜底 + mockData，整搬进 momo 会在 daemon 出错时报「command not found」而非降级。**不整搬 desktop.ts。**

- **搬**：desktop `sessionEvents.ts`（订阅 `session:<id>:event` + `SessionStreamEvent` 全集类型，仅 1 处 import，干净）；`desktop.ts` 的 `normalizeTimelineItem`/`normalizeSessionDetail` + `types.ts` 的 `TimelineItem` 族类型。
- **自写**：瘦封装 `create_session`/`run_agent_turn`/`resolve_permission`/`resolve_user_question`/`cancel_turn`/`load_session_detail`/`list_grouped_sessions`，底层用 momo 的 `ensureDaemonClient()`（协议与 desktop daemonClient 一致，握手来源经 1431 `daemon_handshake`）。**不带** Tauri-invoke fallback / browser-preview / mockData。

### 5b. 状态机 controller `lib/agent/agentChat.svelte.ts`（修正审查 M3）
摘 desktop `App.svelte` 聊天 reducer 成独立模块。
- **保留**：`handleSessionEvent`（switch 部分）、`submitMessage`、`resolvePermission`、`resolveUserQuestion`、`cancelCurrentTurn`、`refreshSessionAfterTurn`、结算/去重族（`markTurnSettled`/`replaySafeDelta`/`reuseTransientMessageIds`/各 signature）、live id 生成器、`combinedTimeline`/`pendingPermissions`/`pendingQuestions`/`turnRunning` 派生。
- **砍**：多 session 后台缓存（`cacheBackground*`/`transientConversationStates` 族 ~1000 行）、sidebar 染色（`setLiveSidebarAgentState`）、provider 鉴权校验、localStorage 草稿持久化。
- **隐藏依赖（审查 M3，必须处理，不是删了就行）**：
  - `connectionState`：从 momo DaemonClient 的 `onConnectionChange` 派生（新增依赖）。
  - `settingsSnapshot`：`submitMessage` 用它取 `defaultProvider` 做 providerId 来源——砍鉴权后，providerId 固定走 OpenAI 兼容默认/不传，依赖 daemon default routing（§4 已设 default_provider/model）。
- **结构硬约束**：momo 路由 `{#key currentRoute.path}` 会重挂 Agent，所以 state 必须**模块级 keyed by sessionId**（`Record<sessionId, {liveStreamItems, submittedMessages, currentTurnId, ...}>`），不能放组件内 `$state`。注意 Svelte 5 模块作用域的 `state_unsafe_mutation`（momo 现有 `chat.svelte.ts:527` 已踩过）。

### 5c. 组件搬运（→ `lib/agent/`）
- **带**：`ConversationView` + `ToolCard`（删 browser-recording 段，连同 `browserRecording` 依赖）+ `QuestionPrompt` + `Approval` + `DiffCard` + `MessageBody`（desktop 自包含版）+ `HighlightedLine` + `codeHighlight.ts` + `Icon`/`Puffer`/`BrandLogo` + `chat.css` + `types.ts`(TimelineItem 族)。
- **砍**：`ModelPicker`（puffer/单 provider，不需模型切换，连带省 providerIds/listProviderModels 依赖链）、`AgentDetail`/`AgentDetailContent`/IDE panes（Terminal/Files/Browser/DiffView）。
- **硬约束**：ConversationView 吃 desktop `TimelineItem`，controller 产出的也必须是 `TimelineItem[]`（不是 momo `ChatMessage`）。这同时让阶段 2 切气泡 UI 的边界保持干净（换渲染层、不换数据）。

### 5d. momo 落点
- `pages/Agent.svelte`：thread 区（`<section class="agent__thread">`）整段换成 `<ConversationView>`，props 接 controller 的 `combinedTimeline`/`pendingPermissions`/`pendingQuestions`/`turnRunning` 等。header / composer 行保留。
- `components/shell/Composer.svelte`：`appendUserMessage`/`createSessionFromText`/`running`/`onCancel` 指向新 controller（模板/样式不动）。
- 路由 `/agent/:taskId` 不变，taskId = daemon `create_session` 返回的 sessionId。
- **废弃**：momo `chat.svelte.ts`、`components/agent/*`、`agentClient.ts`（chat 部分）。

### 5e. sidebar / session 列表迁 daemon（解决审查 B3 + M2）
> 审查发现：session 迁 daemon 后，momo 自管的 `list_grouped_sessions`(1431) 读不到 chat 会话，左栏直接归零——**必须同阶段处理，不是可选项**。
- `sessionStore.svelte.ts` 的 `loadSessions` 改读 **daemon** 的 `list_grouped_sessions`（经 daemonChat）。
- **按 default project cwd 过滤**（M2）：daemon `list_grouped_sessions` 是全局的，monitor session 会混进来；按 `folderPath === <default project cwd>` 过滤出纯 chat 会话。cwd 字符串要与 `create_session` 传入值/daemon canonicalize 对齐。

### 5f. 新会话 cwd 时序（修正审查 M5）
`create_session({cwd})` 的 cwd = momo `list_projects`(1431) 的 default project cwd（`$MOMO_HOME/projects/default`）。避免竞态：app 启动预取并缓存 default cwd，或 Composer 发送前 `await`。否则 cwd 落到 daemon 默认 `$HOME`，session 进错 group、被 sidebar 过滤漏掉。

## 6. 后端处理

- 前端 chat 不再经 1431 跑 turn，故 momo backend 的 `run_agent_turn`/`run_agent_turn_inner`/`codex_app_server.rs`、自管 session(`~/.momo/sessions.json` + create/list/load/rename)、`login_with_api_key` 在 chat 路径上变死代码。
- **阶段 1 先不删**（降低风险），但建议把 1431 的 `run_agent_turn` 直接 stub 成报错（审查 m6），确保没有任何遗留路径再触发 `puffer non-interactive`（彻底消解 task monitor spec 风险⑦的 tg 互踢）。
- **保留**：`daemon_handshake`、connectors、auth（OAuth/WorldRouter mint）。

## 7. 错误处理与边界

- **replay 去重**：daemon 重连重放最近 500 条事件（payload 带 `replay:true`）。controller 用 `settledTurnKeys` + `replayTextByTurn` + 稳定 live id 去重；**text-delta replay 必须按 flag 跳过 append**（否则历史文本翻倍），不只是 tool callId 去重（审查 m2）。
- **一-session-一-turn**：daemon 硬约束（`start_turn` 对同 session 已有 in-flight turn `bail`）。`turnRunning` 时 composer 禁发送；这条 bail 当幂等提示处理，**不要清空 UI**（审查 m1）。
- **缺鉴权 → 异步 turn-error**：见 §4。
- **daemon 未就绪/掉线**：`ensureDaemonClient()` 失败降级（显示「连接中」+ 重试）；观察到失败请求调 `resetDaemonClient()` self-heal（已实现）。
- **cwd 时序**：见 §5f。

## 8. 审查结论沉淀（go/no-go 应对）

本设计经对抗式审查（NO-GO → 返工后 GO）。逐条应对：

| 审查项 | 级别 | 应对 |
|---|---|---|
| B1 daemon 无 "puffer" provider | Blocker | §4：用 OpenAI 兼容 provider + base_url override，删除一切 `providerId:"puffer"` |
| B2 `PUFFER_API_KEY` 死代码 | Blocker | §4：key 走 `login_with_api_key("openai")` 写 `~/.puffer/auth.json`，不注 env |
| B3 sidebar 变空 | Blocker | §5e：sidebar `loadSessions` 迁 daemon `list_grouped_sessions` |
| M1 desktop.ts 不能整搬 | Major | §5a：自写瘦 RPC 层，剥 Tauri-invoke/browser-preview/mockData |
| M2 list_grouped_sessions 全局混入 monitor | Major | §5e：前端按 default cwd 过滤 |
| M3 状态机隐藏依赖 | Major | §5b：connectionState/settingsSnapshot 显式处理，缓存族重写控制流，keyed store |
| M5 新会话 cwd 时序 | Major | §5f：启动预取 / 发送前 await |
| m1/m2/m6 | Minor | §7 / §6：一-turn 幂等、replay text 跳过、stub 1431 run_agent_turn |

底层机制（直连 daemon 跑 turn、token、事件广播、无需显式 subscribe、replay、TimelineItem 自洽、阶段 1/2 干净切分）经核实成立。

## 9. 测试

- **基建**：扩 `tests/support/fakeDaemon.ts`（task monitor 已建）加 chat 事件合成（emit `text-delta`/`tool-*`/`permission-request`/`user-question-request`/`turn-complete`/`turn-error`），帧格式须模拟 daemon **真实帧**（`{id,result}`/`{event,payload}`），不是 daemonClient 注释里的 `{type,ok}`（审查遗漏3）。
- **Playwright**（`npm run test:desktop-ui`）：发消息→text-delta→渲染；tool 卡片三态；permission→Approval→resolve；question→QuestionPrompt→resolve；turn-complete 收尾；turn-error 显错；cancel；replay 重连去重。
- **单测**：controller `handleSessionEvent` reducer（replay 去重 / settled turn / live timeline 合并）。
- **回归**：`npm run check` + `cargo check --manifest-path src-tauri/Cargo.toml`。
- **端到端手动**：登录 mint key → `login_with_api_key`+`update_config` → 发消息能从 daemon 收到回复（验证 §4 worldrouter wire 跑通）。

## 10. 阶段 2 预告（out of scope）

- 把 `ConversationView` 换成 momo `ChatBubble`/`Mascot` 气泡 UI，渲染同一份 `TimelineItem[]`。
- 删除阶段 1 暂留的后端 chat 死代码。

## 11. 风险与未决

- **worldrouter wire 匹配**：团队已验证 worldrouter OpenAI 兼容 API 可用；实现首个 turn 时确认 daemon 出去的 wire 与 worldrouter `/chat/completions` 对齐（§4 验证项），必要时按 `openrouter` provider 形态对齐。非 blocker。
- **与 task monitor 共用同一 daemon**：chat 直连 + task monitor 直连共享 `daemonClient` 单例（OK）；需确保前端无任何路径再走 1431 `run_agent_turn`（§6 stub）。
- **状态机抽取工作量**：M3 的缓存族解耦 + keyed store 改造是阶段 1 的主要工作量与风险，非机械复制。
