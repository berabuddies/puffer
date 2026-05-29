# Momo Chat UI 气泡化(阶段 2)— Design Spec

- **Date**: 2026-05-30
- **Status**: design approved(brainstorming 完成,经 visual-companion + Paper 设计图逐项确认 + opus reviewer 对抗式审查并已并入修正),ready for writing-plans。
- **分支 / worktree**: `feat/momo-chat-bubble-phase2`(从 `feat/momo-desktop` @ `985d41f6` 切出的独立 worktree `.claude/worktrees/feat-momo-chat-bubble-phase2`);spec / plan / 实现都在此分支,完成后整体合回 `feat/momo-desktop`。
- **关联**:
  - kickoff/交接 note:`docs/superpowers/specs/2026-05-29-momo-chat-ui-bubble-phase2-kickoff.md`(commit `c9f28e19`)——本 spec 是它 §6 开放问题走完 brainstorming 的产物。
  - 阶段 1(daemon migration,已 merged):`docs/superpowers/specs|plans/2026-05-29-momo-chat-daemon-migration*`。
  - 设计真值源:`apps/momo/src/DESIGN_SYSTEM.md` + `apps/momo/src/styles/tokens.css`;Paper 文件 "Momo" 的 artboard **"Agent"** 与 **"Agent · Restaurant Booking"**(精确 token 已用 `mcp__paper__get_jsx` 抠出,与 tokens.css 一致)。

---

## 1. 目标与范围

把当前 desktop IDE 风格的 `ConversationView.svelte` 换成 **momo 气泡 UI**,渲染**同一份 `TimelineItem[]`**(阶段 1 已把数据/状态层切干净,就为这一步留的)。

**硬范围 = render-layer only**:只动渲染层(`src/lib/agent/` 的视图组件),**不碰** controller(`agentChat.svelte.ts`)/ `daemonChat.ts` / `normalize.ts` / daemon。视觉基准 = Paper "Agent" artboard + momo `DESIGN_SYSTEM.md` / `tokens.css`。

---

## 2. 决策(已锁定 · 含 reviewer 修正)

对应 kickoff §6 的九个开放问题 + 范围决策:

| # | 问题 | 决策 |
|---|---|---|
| 6 | roll-up vs 纯时序 | **纯气泡时序流**:按 `combinedTimeline()` **数组顺序**逐项渲染,**无 "Agent activity" roll-up 分组**(天然避开 Bug2 的分组逻辑)。view 层**不按 `createdAtMs` 重排**(见 §6)。 |
| 1 | 气泡布局 | 用户**右**气泡 / 助手**左**气泡,均 `#f4f4f4`(`--color-surface-rail`);**无头像**;顶部 **serif 标题**。 |
| 2 | tool 呈现 | **友好 pill,默认折叠成一行**;点开成**统一卡片**,展开 body 复用 `ToolCard` 渲染(复用方式见 §5/§11.1,非零成本)。prod 隐 raw、用中文 `toolLabels`;dev(`SHOW_RAW_AGENT_ACTIVITY`)显 raw toolId/args。 |
| 3 | 思考链 | momo 的 thinking = **瞬时 `turnThinking` 状态,不进 timeline**(reducer 丢弃 delta 内容、无历史)。→ 渲染为**底部 typing 指示器**(`turnRunning && turnThinking` 驱动,文案 `turnStatusHint`),流里不留 item、无 dev raw thinking(数据源不存在)。 |
| 4 | permission / question | question → **选项卡**;permission → **审批卡**(均保留原逻辑只换皮)。**resolve 行为对齐现状**:live turn 里答完即 dismiss(卡片消失,不碰 controller);**历史回看**时 question 因持久化为 `answered` item 自动以"已答"折叠态出现(`QuestionPrompt` 已支持),**permission 无持久化 choice 数据故不渲染已答态**(要做属阶段 3)。 |
| 5 | diff | **diff 卡**(沿用 `parseHunks`/`stats` 解析,换卡片皮)。 |
| 7 | 视觉基准 | Paper "Agent" artboard + `DESIGN_SYSTEM.md`;**复用 `tokens.css`,不新增字体/token**。 |
| 8 | composer | **不动**(shell `Composer.svelte` 的 pill + 奶油发送键已贴合设计)。 |
| — | 结构化卡片数据 | **方案 A**:助手最终总结按普通 **markdown 气泡**渲染(复用 `MessageBody`)。设计图里"选项列表卡 / 结果确认卡"的**结构化数据**(标题/事实/操作按钮)puffer 现不吐 → 出本阶段范围,留**阶段 3**。 |

---

## 3. 关键约束(从 kickoff §3 继承,必守)

1. **别碰 controller / daemonChat / normalize**(Bug2「多轮 tool 错位」高发区)。新气泡 UI 只**消费** controller 输出 + 触发其回调。
2. **按 `combinedTimeline()` 数组顺序渲染,别自创分组、别按 `createdAtMs` 重排**(那正是 Bug2 形态)。
3. **复用 `MessageBody.svelte` 不动**——它的 rAF 节流 + `{#each}` key + parseInline LRU memo 是 streaming 抖动 fix,**不可丢/不可回归**。新气泡渲染 markdown 一律走它。
4. **worktree 隔离**:已切 `feat/momo-chat-bubble-phase2`;**git 操作一律 `git -C <worktree 绝对路径>`,不能 cd**(本仓 Bash cwd 每次重置,已踩坑)。
5. **测试端口避开主仓 1466**(用 1477/1478),**merge 前 revert** 端口改动。
6. 复现回归测试 `tests/agent/multiturn-tool-grouping.spec.ts` 必须仍绿(它驱动 controller 数据层、不碰渲染)。

---

## 4. 视觉语言(token 映射)

全部值已与 `tokens.css` 对齐;Paper 抠出的数值一一对应。**无需新增字体/token**(Source Serif 4 + Inter 已由 `base.css:13` 的 Google Fonts `@import` 加载;Source Sans 3 **未加载** → 统一映射到 Inter `--font-sans`)。

| 角色 | token / 值 |
|---|---|
| 页面背景 | `--color-surface-app` `#fff` |
| 气泡底(用户+助手) | `--color-surface-rail` `#f4f4f4` |
| 用户气泡圆角 | `16px 4px 16px 16px`(右上削尖) |
| 助手气泡圆角 | `4px 14px 14px 14px`(左上削尖) |
| pill / 卡片外边框 | `--color-input-border` `#e0e0e0` |
| 卡片内 hairline / 分隔线 | `--color-card-border` `#ececec` |
| cream 主按钮 | fill `--color-action-cream` `#f8eedc` · border `#f0e2c7` · text `--color-action-cream-text` `#795600` |
| best-fit 高亮(**硬编码,刻意非 token**) | bg `#fff7e8` · border `#f2dca7` · text `#8a650e` |
| 绿勾确认(**硬编码**) | bg `#e5efe0` · stroke/icon `#3f6d3f` |
| 失败态 / diff | del 行底 `#fdecec` 文字 `#b3261e`;add 行底 `#e9f5ea` 文字 `#1e7a34` |
| 标题 | `--font-serif` 24/30(`--font-size-section`),weight 400 |
| 卡片标题 / 按钮 | `--font-sans`(Inter)13–14px weight 500 |
| 正文 / 气泡 / pill | `--font-system`(气泡 14/20,pill 13/16) |
| 圆角 | 卡片 `--radius-card` 16 · pill/按钮 `--radius-pill` 999 · **工具 pill 12px**(token 无此值,沿用 Paper 的 12 作为 chat 局部常量) |
| 内容列 | 宽 `--shell-page-max` 760 · 两侧 `--shell-page-padding` 24 · 顶 36px · 消息间 gap `--space-2`~`--space-3` |

> **⚠️ 调色板桥接(reviewer S2)**:被复用的 `ToolCard.svelte` / `MessageBody.svelte` 用的是 **desktop 调色板别名**(`--muted` / `--accent` / `--border` / `--foreground` / `--destructive` / `oklch(...)`),它们靠 `ConversationView` 第 1 行 `import "./chat.css"` 在 `.pf-chat` 作用域注入才显色。`BubbleConversation` **必须保留这层别名桥接**(继续 import `chat.css` 的 token 别名块,或在新根容器上重定义这批别名)。删 `ConversationView` 时**别把 `chat.css` 的 token 别名块一起删了**。

---

## 5. 组件架构(`apps/momo/src/lib/agent/`)

### 新建
- **`BubbleConversation.svelte`** — 顶层,**替换 `ConversationView`**。消费 `Agent.svelte` 已传的 props 子集(reviewer 已核验吻合):`session` / `timeline` / `pendingPermissions` / `pendingQuestions` / `turnRunning` / `turnStartedAtMs` / `turnThinking` / `turnStatusHint` / `loading`,回调 `onResolvePermission` / `onResolveUserQuestion` / `onCancelTurn`。**不声明** composer/draft/`onSubmitMessage`(momo 输入走 shell `Composer`,那套是死代码)。
- 子组件(放 `components/` 或新 `bubbles/`):
  - **`ChatBubble.svelte`** — user / assistant / system / command 消息气泡,markdown 走 `MessageBody`。
  - **`ToolPill.svelte`** — 折叠 pill + 展开统一卡片。展开 body 复用 `ToolCard` 逐工具渲染——但 `ToolCard` 是**单体、无导出 body 渲染器**(见 §11.1,plan 须选 (a) 加 `variant="pill"` 还是 (b) 抽 `ToolBody`)。
- **底部 typing 指示器** — 不是 timeline 组件,由 `turnRunning`/`turnThinking`/`turnStatusHint`/`turnStartedAtMs` 驱动(承载"思考中…"状态,见 §7.3)。
- **`toolLabels.ts`**(中文友好 label + `lookupToolLabel` 嗅探)、**`debugFlags.ts`**(`export const SHOW_RAW_AGENT_ACTIVITY = import.meta.env.DEV`,只用于 tool pill 的 prod/dev label,不再用于 thinking)。

### 复用 + restyle(保留逻辑,换皮)
- **`QuestionPrompt.svelte`** → 选项卡样式(保留 answer 收集 / 多问多选 / searchable / input 全部逻辑 + 已有的 `answered` 折叠回显分支)。
- **`Approval.svelte`** → 审批卡(保留 `variantFor`;允许=cream,deny=red text)。
- **`DiffCard.svelte`** → 卡片样式(保留 `parseHunks`/`stats`)。

### 不动(直接复用)
`MessageBody.svelte`(关键 fix,**禁改**)、`HighlightedLine.svelte`、`Icon.svelte`、`ToolCard.svelte`(body 渲染逻辑被 `ToolPill` 复用)。注:复用 ToolCard/MessageBody 须保留 §4 的 `chat.css` 调色板桥接。

### 修改
- **`Agent.svelte`**:挂 `BubbleConversation` 替换 `ConversationView`,props/回调照旧接 controller。

### 删除(测试全绿后)
- 旧 `ConversationView.svelte`(reviewer 已核:momo 内仅 `Agent.svelte` 引用,puffer-desktop 用独立 `AgentDetailContent.svelte` 不共享,删除安全)。**保留 `chat.css` 的 token 别名块**。

---

## 6. Timeline → 气泡映射(按 `combinedTimeline()` 数组顺序)

逐项渲染,**不重新分组**:

| TimelineItem | 渲染 |
|---|---|
| `message` kind=user | `ChatBubble` 右(16/4/16/16) |
| `message` kind=assistant | `ChatBubble` 左(4/14/14/14,`MessageBody` markdown) |
| `message` kind=system | 左侧 muted note(error 状态红);verified-skill-gate 降级为普通 system 文本(见 §10) |
| `message` kind=command | muted pill(显 `/command`) |
| `tool` | `ToolPill` |
| `diff` | `DiffCard` |
| `permission` | `Approval`(item 在 combinedTimeline 里、按时序位置;`pendingPermissions()` 仅判定可操作。resolve 后 dismiss,见 §7.5) |
| `question` | `QuestionPrompt` 选项卡(同上;resolve 后 dismiss,历史回看显 answered,见 §7.4) |

**与旧 ConversationView 的关键差异**:旧实现把 permission / pending question 从 timeline 里**剔除再注入到末尾**(`ConversationView.svelte:450-458` + `593-619`);新实现**按 combinedTimeline 数组顺序原地渲染**,用 `pendingPermissions()`/`pendingQuestions()` 只判定可操作性。这更贴时序。

> **不要按 `createdAtMs` 重排(reviewer S3)**:`combinedTimeline()` = `[...persisted, ...submitted, ...live]` 拼接(`agentChat.svelte.ts:1137-1142`),这个**数组顺序**正是 controller 维护的 Bug2-safe 顺序;view 自己 sort 反而可能打散 live/persisted 归属、踩回 Bug2。

底部 typing 指示:`turnRunning` 时用 `turnStartedAtMs`/`turnThinking`/`turnStatusHint` 显示思考/计时(沿用 kickoff §4 契约,Agent.svelte 已传);turn 结束消失,不留 item。

---

## 7. 各元素细节

### 7.1 标题
`session.title`(`SessionListItem`)用 `--font-serif` 24/30 渲染在 flow 顶部;空则隐藏。

### 7.2 ToolPill
- **折叠**:`Icon`(由 toolLabels 决定)+ 中文 label + 状态(running=spinner / success=✓ 绿 / failed=⚠ red)。`width:fit-content`,border `#e0e0e0`,radius 12,min-height 36,padding 8/14。
- **点击展开**:整条变统一卡片——`overflow:hidden` 容器(border `#e0e0e0` radius 12)+ head(占满宽,chevron 右)+ 分隔线 `#ececec` + body。body 复用 `ToolCard` 的 file/bash/mcp/list/web 渲染器。圆角始终对齐。
- prod:label 走 toolLabels 友好文案,隐 raw toolId/原始 args;dev(`SHOW_RAW_AGENT_ACTIVITY`):显 raw。
- 默认折叠(历史 + 当前轮);运行中 tool 是否自动展开 = plan 决定(默认否)。

### 7.3 思考指示(typing/thinking)
**核验结论(reviewer B2,见 `agentChat.svelte.ts:805-808`)**:momo 的 thinking 只 set `turnThinking`/`turnStatusHint`,**从不 append timeline item**,delta 内容被丢弃。`ConversationView` 里把 thinking 当 `ToolTimelineItem(thinking)` 的分支是从 desktop 照搬的、momo live 流走不到的死代码。
- 因此**没有 thinking 气泡 / 无历史 / 无 dev raw thinking**(数据源不存在,别去找)。
- thinking 体现在**底部 typing 指示器**:`turnRunning` 时显示;`turnThinking` 时文案偏"思考中…"否则"处理中…",可叠 `turnStatusHint` + 基于 `turnStartedAtMs` 的计时。turn 结束即消失。

### 7.4 question 选项卡
- 卡片:白底 border `#e0e0e0` radius `4/16/16/16` width≤540;intro `--font-sans` 500;选项行 radius 10、border `#ececec`、min-height 34;多选可勾、input/searchable 保留;提交 = cream pill。
- "推荐"高亮(`#fff7e8`/`#f2dca7`/`#8a650e`)**仅当**某选项被标记 recommended/default 时(puffer `askUserQuestion` 当前无此字段 → 通常不高亮,样式预留)。
- **resolve 行为**:live turn 里答完 dismiss(对齐现状)。**仅历史回看**显已答态——持久化为 `status:"answered"` 的 question item(`normalize.ts:284-308`),`QuestionPrompt` 现有 `answered` 分支折叠回显已选(`--color-text-secondary`),**免费、无需改 controller**。

### 7.5 permission 审批卡
- 卡片样式;标题(`--font-sans` 500)+ reason/summary(secondary)+ command(mono chip,`--color-surface-rail` 底)+ choices 按钮(允许=cream / session·always=neutral / deny=red text)。保留 `variantFor`。
- **resolve 后 dismiss(对齐现状)**:`resolvePermission` 只 dismiss、不写回 status/choice(`agentChat.svelte.ts:1215-1234`),持久化侧也无 choice 字段(`normalize.ts:426-448`)→ **本阶段不做 permission 已答态回显**(要做须改 controller/normalize,属阶段 3)。

### 7.6 diff 卡
- 卡片样式;header(edit icon + title/filename + `+adds −dels`)+ hunk body(mono,add=绿底/del=红底/ctx=secondary,`HighlightedLine` 语法高亮)。保留 `parseHunks`/`stats`。默认折叠。

---

## 8. toolLabels(中文)

- 新 `toolLabels.ts`:`Record<toolId, { icon: IconName; label: string }>`,**中文**友好文案(Read=正在读取文件… / Edit=正在编辑文件… / Write=正在写入文件… / Bash=正在执行命令… / Grep·Glob=正在搜索… / Skill=正在调用技能… / TaskCreate·Update=记录任务… / MCP·connector=对应动作)。
- `lookupToolLabel(toolId, input?)`:嗅探 Bash command 前缀(`telegram `/`email `/…)把通用工具伪装成高层动作(沿用旧 momo 逻辑)。
- 未命中:prod → 通用 "正在处理…";dev → raw toolId。

---

## 9. 测试

- `npm run check`(类型)+ `npm run test:desktop-ui`。
- fakeDaemon e2e(真实 API,默认 legacy protocol),参考 `tests/agent/*.spec.ts`,覆盖:
  - 发消息 → 用户右气泡 + 助手左气泡渲染;
  - tool pill 折叠 → 点击展开统一卡片;
  - permission 往返(允许/拒绝),答完卡片消失(本阶段不留已答态);
  - question 往返(单选 / 多选 / 输入),live 答完卡片消失;**reload 后历史回看显 answered 折叠态**;
  - **permission / question 按 timeline 位置原地渲染**(出现在其对应 tool 之后,而非流末尾);
  - cancel turn;
  - streaming:`MessageBody` 不抖(rAF 节流仍生效)。
- **`tests/agent/multiturn-tool-grouping.spec.ts` 仍绿**(驱动 controller 数据层、不碰渲染)。
- **必改的现有断言(reviewer N3)**:`tests/agent/chat-interactions.e2e.spec.ts:167-169` 断言"question 答完不以 answered card 持久"——那是针对旧 view 写的;新行为是 live 答完 dismiss、历史回看显 answered,该断言需随渲染层同步**改写**。
- 端口用 1477/1478,**merge 前 revert**。

---

## 10. 不做 / 阶段 3

- 结构化 result/options 卡片**数据**(需 agent 结构化输出或改 puffer)= 阶段 3。本阶段助手总结 = markdown 气泡。
- **permission 已答态回显** = 阶段 3(需 controller 写回 choice / 改 normalize;本阶段 resolve 后 dismiss)。
- **verified-skill-gate 详情结构化解析**降级为普通 system 文本(旧 view 的 `verifiedSkillGateDisplay`/`gateDetailRows` 不搬;如需富展示后续再说)。
- composer 改动 = 不做。
- Mascot / 头像进对话流 = 不做(设计图无)。

---

## 11. 开放问题(minor,plan 阶段定)

1. **`ToolPill` 复用 `ToolCard` body 的方式**:(a) 给 `ToolCard` 加 `variant="pill"/headless` prop(改动集中,但要重写 head 用 momo token)vs (b) 抽出独立 `ToolBody` 子组件(更干净,~250 行解析搬家)。注:"import 私有渲染函数"经核验**不可行**(未 export + 依赖组件作用域)。
2. 运行中 tool 是否默认展开(默认否)。
3. system / command 消息样式细化(确实出现:`normalize.ts:366-392` + reducer error item)。

> 已由 reviewer 关闭的原开放问题:thinking item 形态(→ 无 item,走 typing 指示器,§7.3);ConversationView 其它引用(→ 仅 Agent.svelte,删除安全)。
