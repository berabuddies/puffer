# Monitor 入站过滤链修复设计(plan-04 / agentenv/monorepo#766)

日期:2026-07-04
分支:`fix/plan-04-monitor-inbound-filter-chain`
关联 issue:#766(主),children #756 / #594 / #592

## 背景与根因(已核实到代码)

Monitor 入站过滤链在三个层面漏判,把不该建任务的消息放进任务队列:

1. **#756 — 自发消息识别单 bit 依赖**。`event_is_self()`(`crates/puffer-subscriptions/src/router.rs:1352`)只看 `kind == "message_self" || payload.is_outgoing`,而 `is_outgoing` 唯一来源是 grammers 的 `out` 位(`crates/puffer-subscriber-telegram-user/src/events.rs:90`)。issue 日志证明 envelope `8945828d` 以非 self 身份走完 classify + triage 并被派发到非 monitor 绑定 `tg-review-auto-reply` 执行 send_message(被发送授权挡住而 Failed)——self 事件对非 monitor 绑定结构上必然跳过,因此该消息到达 router 时标志已丢。无 sender 身份兜底。
2. **#594 — Gmail 行 ID 含屏幕位置**。gmail-browser 是 DOM 抓取器;行 ID 回退键为 `[sender, subject, snippet, index].join(":")`(`crates/puffer-cli/src/gmail_browser_script.rs`)。归档/移除 Inbox 标签导致下方行 `index` 位移 → 旧邮件产生新键 → 被 `should_emit_row`(`gmail_browser.rs:713`)当新邮件重发 → 为旧邮件建任务。不存在真正的"archive 事件类型",issue 设想的"事件白名单"在 DOM 抓取模型下没有落点。
3. **#592 — monitor binding 无分类层**。`monitor_create.rs:216-221` 创建 monitor binding 时 `filter: None`、`classify_prompt: None`、`ignore_filters: []`——架构里现成的两级闸门(确定性 ignore 规则 + yes/no LLM 分类器)对 monitor 完全未启用,所有未见过的邮件行直达昂贵的 triage agent,唯一防线是宽松的 triage prompt("Someone expects a response" 字面命中自我介绍邮件;"needs my review" 命中 GitHub 通知)。

## 决策记录

| 决策 | 结论 |
|---|---|
| 自发消息角色 | 保留"有开放任务时派发 triage 做完成检测"的产品能力,只修识别可靠性(双信号) |
| Gmail 无稳定 thread-id 时 | 内容键兑底(不含 index),容忍极少误报,不接受静默漏报 |
| 分类层形态 | 确定性规则 + LLM 分类器两级 |
| 打包方式 | 三处定点修复;否决统一 InboundGate 管线抽象(router 过滤链已是该管线,且 #594 根因在 connector 端,抽象覆盖不到) |
| unread 白名单 | 不做。稳定 ID + seen 集合即"新到达"判定;unread 白名单会漏掉在手机上先读过的新邮件 |
| 向后兼容 | 不考虑。seen.json 键格式直接切换,首轮 poll 重新 baseline(现有初始窗口限制防洪泛) |

## 区域 1 — 自发消息识别双信号化(#756)

### Subscriber 端(权威打标)

- 登录/启动时经 `get_me()` 取自身 `user_id` 存入 subscriber 运行态(登录切换时刷新)。
- `build_message_event`(`events.rs`)新增 payload 字段 `sender_is_self: bool = message.outgoing() || sender_id == self_user_id`。
- `is_outgoing` 字段保留原语义(grammers out 位)不变,两字段语义各自干净。

### Router 端(判定收敛)

- `event_is_self()` 扩展:`kind == SELF_MESSAGE_KIND || payload.is_outgoing || payload.sender_is_self`。
- 行为矩阵不变:
  - self + 无开放任务 → 拦截(`router_self_gate_skipped`)
  - self + 有开放任务 → 派发 monitor triage(完成检测)
  - self → 永不到达非 monitor 绑定(消除 #756 的 Failed 来源:自动回复绑定对自发消息动作)

### 可观测性

- self 事件被放行派发时新增 trace 阶段 `router_self_dispatch_open_task`(目前仅 skip 有打点,放行静默,误放行无法归因)。

### 收益

- 任何一条 grammers 投递路径丢 out 位(update gap、unknown-peer 恢复等)都被 sender 身份兜住。
- 自发消息不再漏进 classify + triage,LLM 成本下降。
- 消除"若发送授权存在,puffer 会自动回复用户自己消息"的潜在事故。

## 区域 2 — Gmail 行身份去 index 化 + 拒绝打点(#594)

### 抓取脚本(`gmail_browser_script.rs`)

- 行 `id` 派生顺序:thread-id 属性(`data-legacy-thread-id` 等)→ **内容哈希 `hash(sender + fromEmail + subject + snippet)`,不含 index**。
- 哈希在 JS 端用简单非密码学哈希(FNV/djb2)——只是 dedup 键,不引入 crypto 依赖。
- `index` 仅保留为诊断字段,彻底退出身份派生。

### emit 判定(`gmail_browser.rs`)

- `should_emit_row` 拒绝时经现有 `diag::` 通道打点 skip 原因:`seen_duplicate` / `initial_window_excluded`(对齐 plan-04 第 4 条)。
- seen 集合加上限(5000 键),超限按插入序淘汰最老一半(BTreeSet 换带序号结构)。防长期运行膨胀。

### 语义修正

归档/移标签后行位移不再产生新键,误报源头消失。已知残余误报面:同一封邮件的 snippet 被 Gmail 重算时可能重发一次(接受,极少)。

## 区域 3 — Gmail monitor 两级分类闸门(#592)

### 第一级 — 确定性 ignore 规则(免 LLM)

- `monitor_create.rs` 为 gmail-browser 类 monitor binding 默认安装 `ignore_filters`(现有字段):
  - 发件人规则:`no-reply@` / `noreply@` / `notifications@github.com` / `*@notifications.*`
  - 文本规则:unsubscribe 标记类
- 规则数据放常量表,按 connector 类型选择。命中走现有 `router_ignore_filter` 打点,零 LLM 成本。

### 第二级 — 默认 classify_prompt(廉价 yes/no LLM)

- gmail monitor 创建时给默认分类 prompt:"这封邮件是否需要收件人采取行动或回复?自动通知、营销、newsletter、陌生自我介绍回答 no"。
- **分类输入补强**:`gmail_browser.rs` 的 `emit_message` 把 `fromEmail` 并入 `event.text` 首行(发件地址是最强通知类信号,目前不在分类输入里)。
- 分类器 `Inconclusive → Reject` 安全默认已存在,不动。

### 第三级 — triage prompt 收紧(兜底)

- `monitor_triage_prompt` 的 Ignore 清单补规则:"自动化通知(CI/PR/构建/营销/newsletter)与无既有关系的自我介绍/冷启动邮件 → 得分 ≤3,不建任务"。

## 测试矩阵

| 场景 | 测试 |
|---|---|
| 自发连发 5 条全拦 | router 单测:5 条 `sender_is_self`(部分不带 `is_outgoing`)→ 全部 `router_self_gate_skipped`,dispatcher 零调用 |
| out 位丢失兜底 | subscriber 单测:`outgoing()==false` 但 sender==self → payload `sender_is_self: true` |
| 自动回复绑定不碰自发消息 | router 集成测试:self 事件 + 非 monitor connector_act 绑定 → 不派发(#756 直接回归) |
| 归档不误报 | gmail 单测:同批 row 内容键,index 全体位移后 `should_emit_row` 全 false |
| 真实新邮件正常建任务 | gmail 单测:新内容键 → emit;人写求回复邮件不命中 ignore 规则(防过度收紧) |
| GitHub 通知拦截 | ignore 规则单测:`notifications@github.com` 命中 `router_ignore_filter` |
| Gmail 标签整理 | 行集合减少(归档/移标签)→ 零 emit,skip 原因打点可见 |

## 明确不做

- 统一 InboundGate 管线抽象
- unread 白名单、Gmail 事件类型系统
- `is_monitor_binding` 的 slug/description 启发式改造(与本 plan 无关)
- 接线闲置的 `MonitorDebounce`(self 连发修复后全被拦;非 self 已有 digest 队列)
- 旧 seen.json 键格式兼容
