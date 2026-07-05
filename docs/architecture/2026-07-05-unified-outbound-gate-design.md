# 统一出站动作人审闸门（plan-05 / agentenv#767）设计

日期：2026-07-05
分支：`plan-05/unified-outbound-gate`
关联 issue：agentenv/monorepo#767（父单），#728 / #561 / #634（子单）
约束：不考虑向后兼容；以长期收益、稳定性为先；防止过度设计。

## 1. 问题

"向外发消息"在现有代码里经过 4 处互不复用的判定和 3 套并行的草稿状态机：

| 层 | 位置 | 缺陷 |
|---|---|---|
| 工具层 ConnectorAct | `connector_tools.rs:646` | `send_like_action_slug` 启发式脆弱；查用户模板可被弱化覆盖（#728-V1/V2） |
| 工具层 MonitorReplySend | `task_tools.rs:473/2677` | 非 human-gated 任务免审直发；判定靠 metadata 形状嗅探（#728-V3） |
| daemon 层 | `daemon.rs:5337` | 第二份 `monitor_task_is_human_gated` 实现，与 core 版漂移 |
| 人审 RPC | `monitor_reply_send` / `monitor_action_execute` / `connector_action_execute` | 三个独立状态机；**没有任何 cancel RPC**，取消是纯客户端手势（#561） |

三套草稿存储：`pending_reply`、`pending_action`（task metadata 内嵌）、`outbound_action_drafts.json`。
monitor action turn 被 `allowed_tools` 硬锁 + prompt 明文禁止，显式用户指令也无法向其他收件人发起人审（#634）。

## 2. 核心不变式

**整个代码库只有一个函数能触发 LLM 发起的外部发送：`OutboundStore::execute_approved()`，它只接受 `approved` 态的 action。**

人审卡拦截的是模型意志，不拦截用户意志：

- LLM 发起（agent 会话 / 任务会话 / 后台任务）→ 一律先落草稿，人审后发。无免审分支。
- 规则自动动作（subscriptions ActionDispatcher，用户配置监控规则触发）→ 豁免闸门（配置规则即长效授权），但记审计。
- 人审批准 → 放行执行。

## 3. 架构

落位：`puffer-subscriptions` 新增两个模块（不新建 crate）：

- `outbound_gate.rs` — 纯判定函数
- `outbound_store.rs` — 单一存储 + 状态机

依赖方向天然成立：`puffer-core`（工具层）与 `puffer-cli`（daemon RPC）均已依赖 subscriptions；catalog/permission 事实源本就在此。

```
LLM 意志                                人类意志
────────                               ────────
ConnectorActionDraft (唯一草稿工具)      approve/cancel RPC (唯一裁决入口)
        │                                    │
        ▼                                    ▼
   ┌─────────────────────────────────────────────┐
   │ OutboundStore (~/.puffer/outbound_actions.json)│
   │ draft_ready ──approve──▶ sending ──▶ sent    │
   │     │  │                    │                │
   │  cancel TTL过期          send失败            │
   │     ▼  ▼                    ▼                │
   │ cancelled expired    failed / uncertain      │
   └─────────────────────────────────────────────┘
        ▲
   OutboundGate::evaluate() — 纯函数
```

## 4. 数据模型

单一 action 记录 schema（替代现有三种草稿形状）：

```
id, version,
connector_slug, connection_slug, action, input,
recipient_stable_id, recipient_source: "stamped" | "model",
message, content_hash,
origin { session_id, turn_id, task_id? },
status,           # draft_ready | sending | sent | cancelled | expired | failed | uncertain
created_at, expires_at,   # 默认 24h TTL
approved_message, approved_by, approved_at,
client_request_id, send_attempt_id,
receipt, error,
events[]          # 生命周期事件（沿用 monitor_reply_events 事件形状）
```

要点：

- **monitor 任务不再内嵌草稿**：task metadata 只存 `outbound_action_id` 引用；`pending_reply` / `pending_action` 内嵌形状与状态机代码全部删除。
- **收件人来源分级**：monitor 任务的收件人仍由服务端 source_context stamp（模型不可指定，`recipient_source: stamped`）；显式指令场景（#634）收件人由模型从用户指令提取（`recipient_source: model`），审批卡区分显示、人眼确认。
- **cancelled / expired 是终态**：不可 supersede、不可 approve。"取消后重新要求发送" = 创建全新 action（新 id、新 version）。
- 并发：沿用现有 per-id 锁模式（参考 `DRAFT_LOCKS`），文件原子写。

## 5. 闸门判定

```rust
enum SendOrigin {
    LlmInitiated { session_id, turn_id, task_id: Option<String> },
    RuleAutomation { rule_id },
    HumanApproved { action_id },
}
fn evaluate(origin, connector_slug, action_slug, catalog) -> GateDecision
// GateDecision: Allowed { audit } | RequiresDraft | Blocked { reason }
```

规则（净简化，无新增分支）：

1. `LlmInitiated` + 外发动作 → `RequiresDraft`，无例外。两份 `monitor_task_is_human_gated` 及配套 delivery-target 嗅探全部删除。
2. `RuleAutomation` → `Allowed` + 审计。
3. `HumanApproved` → 放行（只能来自 execute RPC）。
4. **外发动作判定以 builtin catalog 为唯一事实源**：删除 `send_like_action_slug` 启发式；catalog 中 `external_side_effect: true` 的 action 一律 `RequiresDraft`。轻动作豁免（如 `react`）必须在 catalog 显式标 `category: external_reaction` 白名单，不靠 slug 猜。
5. **模板加固（堵 #728-V1）**：`ConnectorCatalogStore::upsert` 校验——用户模板覆盖同 slug builtin 时，各 action 的 `category` / `external_side_effect` 不得弱于 builtin 同名 action；gate 读 catalog 时以 builtin 权限为下限合并。

## 6. RPC 面（daemon）

三套发送 RPC 收敛为三个统一方法，旧 RPC 直接删除：

| 新 RPC | 替代 |
|---|---|
| `outbound_action_execute {action_id, version, approved_message, client_request_id}` | `monitor_reply_send` + `monitor_action_execute` + `connector_action_execute` |
| `outbound_action_cancel {action_id, version, reason?}` | （此前不存在——#561 根修） |
| `outbound_action_status {action_id, version}` | `connector_action_draft_status` |

- `execute` 保留现有防重语义：version 校验、provenance 校验（created_by=ConnectorActionDraft）、stale `sending` → `uncertain` → `duplicate_risk_ack_required`。
- TTL 惰性判定：execute 时 `now > expires_at` → 拒绝并标 `expired`。不做后台清扫任务。
- BOBO 与 desktop 同步适配新 RPC（⚠️ 需 BOBO 修改，父单标 in-review）。desktop 审批卡（ToolCard connector-draft）增加 Cancel 按钮调 `outbound_action_cancel`。

## 7. 工具层

- `ConnectorActionDraft` 成为唯一草稿工具，扩展 `task_id` 参数（monitor 场景写回任务引用）。
- 删除 `MonitorReplyDraft`、`MonitorActionDraft`、`MonitorReplySend` 三个工具及 dispatch 分支。
- `ConnectorAct` 保留给非外发动作（read_history 等）；外发动作返回引导错误 "use ConnectorActionDraft"。
- **monitor action turn（#634）**：`monitor-telegram-action.yaml` / `monitor-reply-action.yaml` 的 `allowed_tools` 改为 `ConnectorActionDraft + WebSearch + WebFetch + AskUserQuestion`；prompt 措辞改为"任务回复的收件人以任务来源为准；仅当用户显式指令要求时，才可向其他收件人创建草稿，同样走人审"。

## 8. 审计

- 每次 gate 决策 append 一行 `~/.puffer/outbound_audit.ndjson`：
  `{at_ms, origin, connector, action, decision: allowed_rule|draft_required|blocked|approved_send|cancelled|expired, action_id?, rule_id?}`
- action 记录内 `events[]` 保留生命周期事件（draft_created / cancelled / send_started / sent / send_failed …）。
- 职责：NDJSON 回答"闸门整体是否一致"（回归验证、可 grep）；`events[]` 回答"这条 action 经历了什么"（审批卡、排查）。
- 审计写失败不阻塞发送（best-effort + stderr 告警）。

## 9. 删除清单（本方案主要收益之一）

- daemon workflows：`monitor_reply_send.rs`、`monitor_action_execute.rs`、`connector_action_execute.rs` → 合并为一个 `outbound_action.rs`
- 两份 `monitor_task_is_human_gated` + `monitor_task_has_telegram_delivery_target` + `monitor_task_has_delivery_target`
- 工具：`MonitorReplySend` / `MonitorReplyDraft` / `MonitorActionDraft` 及 dispatch 分支
- `send_like_action_slug` 启发式
- task metadata 中 `pending_reply` / `pending_action` 全部读写代码
- 磁盘遗留旧 draft 数据：不迁移、直接忽略（旧字段无人读取 = 天然作废，无发送风险）

## 10. 错误处理

- 执行失败：`sending → failed`，可重新 approve 重试。
- 进程中断残留 `sending`：状态探测标 `uncertain`，需 `duplicate_risk_ack` 才能重试（沿用现有语义）。
- 找不到 action / version 不匹配 / 终态 action：execute 与 cancel 均明确报错，不做静默兜底。

## 11. 测试矩阵（对应 #767 验收表）

1. agent 会话直发 TG → 必出草稿卡，approve 后才发（工具层单测 + daemon RPC 集成测试）。
2. cancel 后 approve/execute → 拒绝，终态不可逆；supersede 到 cancelled action → 拒绝。
3. 取消后重新要求发送 → 新 action id、新人审。
4. 任务会话显式指令发第三方 → 出草稿卡（`recipient_source: model`），approve 后发出。
5. 无显式指令自发 → gate `RequiresDraft`，永不静默发送。
6. 用户模板弱化 builtin 权限 → upsert 被拒。
7. TTL 过期 → execute 拒绝并标 `expired`。
8. 迁移 `monitor_reply_send.rs` 现有防重测试族（forged provenance / stale sending / version mismatch）到统一 RPC。

性能说明：人审路径频率量级低，无额外性能设计；文件锁 + 原子写与现状一致。

## 12. 明确不做（防过度设计）

- 规则自动动作过闸 / 逐条确认（Q1 决策：豁免 + 审计）。
- 显式指令的意图检测机制（#634 用工具解锁 + prompt 措辞解决）。
- 草稿的隐式作废（新 turn / turn 停止触发）——turn 取消传播属 plan-06。
- 后台 TTL 清扫任务（惰性过期足够）。
- 新建独立 crate。
- 旧数据迁移。

## Out of scope（与 #767 一致）

- turn 取消传播与后台任务收割（plan-06）。
- 审批对话框 UX 与 ACL 确定性（plan-09）。
