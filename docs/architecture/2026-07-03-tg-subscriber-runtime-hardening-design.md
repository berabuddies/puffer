# TG 订阅器运行时健壮性设计（plan-02 / agentenv/monorepo#764）

日期：2026-07-03
分支：`refactor/tg-subscriber-runtime-hardening`
覆盖 issue：#610 #639 #717（hydration 无就绪语义）、#604（降级检测慢 + 重连无退避）
约束：不考虑向后兼容；只考虑长期收益、稳定性与性能；防止过度设计；仅改 puffer 仓库。

## 0. 根因摘要（本设计的输入）

1. **同步阻塞 hydration**：daemon 在 contacts RPC 路径上起线程拨号 Telegram，`recv_timeout(15s)` 超时后线程被遗弃继续跑（`daemon_contacts_telegram_peer_cache.rs:230-350`），且无 in-flight 去重。
2. **就绪语义缺失**：`telegram_peer_cache_needs_hydration` 以"peers 非空"代替"完整"（:213-228）；`contacts_search` 与带 query 的 list 硬编码 `ready: true`（`daemon_contacts.rs:177,245`）；contact-book hydration 完成与否零状态记录。
3. **session 写竞争**：daemon hydrate 客户端与 subscriber live 客户端共用同一份 `telegram.session` 并各自回写（`daemon_contacts_telegram_peer_cache.rs:311-314` vs `persist_live_session_state`）。
4. **降级检测靠 60s 轮询**：`health_from_control_event`（`puffer-subscriptions/manager.rs:1001-1052`）不消费 `resume_failed` / `update_loop_error` 事件；auth 类失败要等 `spawn_auth_monitor` 的 60×1s tick。
5. **运行期重连脆弱**：update loop 恢复分支 = 固定 1s 延迟 + 单次 resume，失败即进程退出（`client.rs:486-531`），网络抖动/睡眠唤醒被迫重新登录。

## 1. 核心决策

| 决策 | 选择 | 理由 |
|---|---|---|
| Telegram 连接所有权 | **全归 subscriber**，daemon 永不拨号 | session 竞争在架构上消灭；单一连接、单一写方 |
| contacts RPC 语义 | **完全非阻塞**：读缓存立即返回 + 状态字段，完成后事件推送 | RPC 延迟稳定毫秒级；无超时参数要调 |
| 运行期重连 | **复用登录阶段的离线停靠状态机**，不新建重试循环 | 一套退避机制服务两个场景；净减代码 |
| UI 范围 | puffer-desktop 最小消费（提示条 + 事件重拉），不动翻页/布局 | 新契约在本仓库内有真实消费方 |

## 2. Hydration 所有权重构

### 2.1 Subscriber 侧（`crates/puffer-subscriber-telegram-user`）

- **新增命令** `TelegramHydrateContacts { target: usize }`（定义于 `puffer-subscriber-runtime/src/command.rs`）。
- 运行期处理：在已有 live client 上 `tokio::spawn` 执行 contact-book hydration（`contacts.GetContacts{hash:0}` + `contacts.GetSaved`）+ recent-dialog 扫描到 `target`。
- **单飞**：进程内最多一个 in-flight hydration 任务（持有 `JoinHandle`）；任务在跑时重复命令直接 ack `contacts_hydrated { ok: false, state: "hydrating" }`，不排队不叠加。
- 完成/失败：写 peer-cache v2（见 2.2）+ 发 control event `contacts_hydrated { ok, error?, peer_count }`。
- 登录阶段收到该命令：回 `contacts_hydrated { ok: false, state: "auth_required" }`。

### 2.2 peer-cache.json v2

顶层新增：

```json
"contact_book": {
  "state": "ready | hydrating | failed",
  "hydrated_at_ms": 1730000000000,
  "last_error": null
}
```

- `CACHE_VERSION` 递增。旧缓存（无 `contact_book` 字段）一律视为 not-ready，触发一次 hydration 后自愈；**不写迁移代码**。
- 此文件从此**只有 subscriber 一个写方**。

### 2.3 Daemon 侧（`crates/puffer-cli`）

**删除**：`hydrate_telegram_peer_cache_from_session_blocking`、`hydrate_telegram_recent_peer_cache_from_session_blocking`、`TEST_HYDRATOR` 桩、daemon 内 `Client::connect` 拨号路径。`daemon_contacts_telegram_peer_cache.rs` 收缩为：缓存读取 + 就绪状态计算 + hydrate 命令派发。

**行为**：

- `contacts_list` / `contacts_search`：纯读缓存立即返回；发现账号 not-ready 时 fire-and-forget 发一次 hydrate 命令。
- `contacts_refresh`：无条件发 force-hydrate 命令后立即返回当前快照。
- **按需拉起 subscriber**：复用 `start_connection_subscriber`（放宽 `has_consumer` 前置条件），有 session 文件但 subscriber 未跑时先拉起再发命令。
- **响应契约（破坏性变更）**：`ready: bool` 替换为

  ```json
  "sync": { "state": "ready | hydrating | failed | auth_required", "updated_at_ms": ..., "error": null }
  ```

  多账号时聚合规则：任一账号 `hydrating` → `hydrating`；否则任一 `failed` → `failed`；否则任一 `auth_required` → `auth_required`；全 ready → `ready`。`has_more` / `next_cursor` 不变。
- daemon 消费 `contacts_hydrated` control event → 向前端事件总线（`DaemonState::events` broadcast）发布 `contacts_updated` 事件。

## 3. 事件驱动降级（`crates/puffer-subscriptions/manager.rs`）

`health_from_control_event` 新增两个映射（事件本身 subscriber 早已发射，仅 daemon 未消费）：

| 事件 | class | ConnectionHealthStatus |
|---|---|---|
| `resume_failed` | `auth` | `AuthRequired`（→ 立即 `Degraded`） |
| `resume_failed` | `network` / 其他 | `Retrying`（→ 立即 `Degraded`） |
| `update_loop_error` | `auth` | `AuthRequired`（→ 立即 `Degraded`） |
| `update_loop_error` | `network` / 其他 | `Retrying`（→ 立即 `Degraded`） |

- subscriber 侧零改动。
- 60s 轮询（`spawn_auth_monitor`）保留为兜底（覆盖进程静默死亡），不再是主检测路径。
- 降级感知从最多 60s 降到秒级。

## 4. 运行期有界退避重连（`crates/puffer-subscriber-telegram-user/src/client.rs`）

**统一为一套离线停靠状态机**（现有 `OfflineResumeState`：5s 起、×2、封顶 60s、停靠期间命令可响应、发 `resume_offline` 事件）：

- `UpdateLoopExit` 新增变体 `WentOffline(String)`。
- update loop 遇 **network 类**流错误：删除现有"固定 1s + 单次 resume"分支，改为返回 `WentOffline(detail)` → `run()` 以 `OfflineResumeState::new(detail)` 重新进入登录循环的离线停靠分支。
- **auth 类**错误：走现有 `ReauthStarted` 路径回登录阶段（`login_required` 已映射 `Degraded`）。
- **进程不再因流错误退出**；fatal exit 只留给 stdin 断开等真正意外。
- `next_offline_retry_delay` 补 **full jitter**：`delay/2 + rand(delay/2)`，用 `SystemTime` 纳秒做种子，不引入新依赖；封顶 60s 不变。

### 明确不做（防过度设计）

- 不加新的 `ConnectionState` 变体（复用 `Degraded`）。
- 不做 MTProto 心跳/keepalive 探测（更新流本身即活性信号）。
- 不把轮询周期做成可配置。
- 不做 hydration 任务队列/优先级（单飞足够）。

## 5. puffer-desktop 最小消费

- `src/lib/api/desktop.ts`：`ContactsSnapshot` 增加 `sync` 字段；订阅 `contacts_updated` 事件。
- `src/lib/screens/Contacts.svelte`：
  - `sync.state === "hydrating"` → Refresh 旁非阻塞"同步中"提示，当前候选照常展示；
  - `sync.state === "failed"` → 显示错误，Refresh 即重试入口（不加新按钮）；
  - 收到 `contacts_updated` → 自动重拉。
- 不改翻页、布局、其余交互。

## 6. 测试矩阵

| Issue 场景 | 测试落点 |
|---|---|
| 冷启动 hydration 延迟 30s | daemon contacts 测试：注入 `state:"hydrating"` 缓存文件，断言 RPC 立即返回、`sync.state` 正确（纯文件注入，无线程桩） |
| hydration 进行中请求列表 | 同上 + 断言部分候选照常返回 |
| hydration 完成 | subscriber 单测：`contacts_hydrated` 发射 + v2 元数据置位；manager 测试：→ `contacts_updated` 转发 |
| 网络断开秒级感知 | manager 测试：`update_loop_error{class=network}` envelope → 记录变 `Degraded/Retrying` |
| 退避重连成功 | subscriber 单测：`WentOffline` → 离线停靠 → 恢复（扩展现有 offline resume 测试） |
| 反复断连无风暴 | jitter 单测：区间 `[delay/2, delay]`、封顶 60s |

补充用例：hydrate 命令单飞（重复命令不叠加）；登录阶段 hydrate 命令返回 `auth_required`。

## 7. 交付物与删除清单

**改动**：

- `crates/puffer-subscriber-runtime/src/command.rs`：+1 命令。
- `crates/puffer-subscriber-telegram-user`：hydrate 任务 + 单飞、peer-cache v2、`WentOffline`、jitter。
- `crates/puffer-subscriptions/src/manager.rs`：+2 事件映射、`contacts_hydrated → contacts_updated` 转发。
- `crates/puffer-cli`：contacts 纯读化、按需拉起 subscriber、`sync` 契约。
- `apps/puffer-desktop`：类型 + 提示条 + 事件监听。

**删除**：daemon 两条 blocking hydration 路径、`TEST_HYDRATOR`、update loop 单次恢复分支、`ready: bool` 旧契约。预计净代码量下降。

**风险点**：按需拉起 subscriber 引入 "contacts RPC → manager 启动进程" 的新依赖方向；`start_subscriber` 已被 auth monitor 线程并发调用过，基建成熟，实现时需验证无锁顺序问题。

**外部影响（接受）**：bobo 仓库消费的 `ready: bool` 契约被 `sync` 对象替换，bobo 需后续跟进适配；本设计不为其保留兼容层。
