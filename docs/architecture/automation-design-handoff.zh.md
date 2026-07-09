# Automation 设计交接

## 目标

桌面端 Automation tab 是一个以提示词为起点的入口，用来创建和管理简单的
automation，不引入画布。当前实现已经接入 daemon Automation 合约，用于持久化
records、加载 catalog、runtime sync、preview run、启用、删除和 run history。本地
Svelte state 仍然负责创建页/详情页的临时编辑态和乐观 UI 状态。

设计意图：

- 创建路径保持线性，用户可以 review 后再保存。
- 视觉风格贴近 Puffer 现有桌面端 UI，保持紧凑、克制。
- 所有可见文案都面向用户，不使用内部实现口吻。
- 避免节点图、无限画布控制、内部状态说明等复杂表达。

## Runtime 术语边界

UI 可以继续把可选择能力叫 `tool`，这是面向用户的 automation 语言。内部实现里，
tool 只是 umbrella term：

- `AgentEnv` tool 使用 `runtime_owner: "agentenv"`，会直接编译成 AgentEnv
  workflow node。
- connector-backed tool 使用 `runtime_owner: "puffer"`，持久化为
  `puffer_connector_action` step。它是 Puffer-owned runtime boundary：
  daemon 通过 connector/subscription stack 执行 connector action，再把 bridge result
  作为后续 AgentEnv continuation workflow 的输入。
- connector event trigger 也由 Puffer 拥有。它从当前 connector event envelope
  生成 automation input；AgentEnv 看到的是 workflow input，不是 connector schema。

不要把 `puffer_connector_action` 当成 AgentEnv node type。它是稳定 wire marker，
用于把 Automation flow 在 Puffer-executed connector action 两侧切成 AgentEnv
可执行片段。

## 当前入口

### 侧边栏

Automation 已作为桌面端 shell 的侧边栏入口出现。页面标题为 `Automation`。

### 首页输入框

首页以 `Create an automation` 开始，并复用 Puffer 现有 composer 结构：

- 附件按钮。
- 模型选择器。
- Fast toggle。
- Thinking 选择器。
- Permissions 选择器。
- 发送按钮。

输入框 placeholder 引导用户用自然语言描述想自动化的事情。提交后会进入完整创建页；
如果提示词命中当前已支持的模式，会自动预填部分配置，等待用户 review 后保存。
daemon catalog 可用时，预填会优先选择 catalog-backed trigger 和 tool；否则会回退到
本地 starter 形状。

### 列表区域

输入框下方是一个 segmented control：

- `Your automations`
- `Template Library`

`Your automations` 来自 daemon `automation_list` 结果；没有已保存 daemon record 时
为空。空态文案是 `创建你的第一个automation，处理重复的工作流`，主按钮是
`create automation`。右上角工具栏按钮是 `new`。

`Template Library` 展示模板卡片。点击模板会打开创建页，并带入预设名称、说明和
trigger。

## 当前创建路径

创建页是完整页面，不是 modal，也不是侧边面板。

顶部栏：

- 返回 `Automations` 的面包屑。
- `Create New` 标签。
- `Cancel`。
- `Save`。

页面主体：

- `Name`
- `Triggers`
- `Instructions`
- `Tools`
- `Run location`

点击 Save 会调用 daemon `automation_save` RPC，把返回的 record upsert 到本地列表，
并返回首页。点击 Cancel 会直接返回首页，不会创建 record。

### 自然语言预填

当前 prompt parser 会识别几类宽泛关键词：

- Pull request 相关提示词会生成 `PR review draft`。
- Calendar、invite、RSVP、meeting 相关提示词会生成 `Calendar RSVP`。
- Gmail 或 email 相关提示词会生成 `Email reply draft`。
- Slack、message、reply 相关提示词会生成 `Reply draft`。
- Daily、weekday、morning、digest、every 相关提示词会生成 `Morning digest`。

这部分预填仍然是启发式逻辑，但现在会优先使用 daemon catalog 中匹配的 trigger 和
action。它只负责生成可 review 的草稿；最终保存的是 daemon `AutomationSpec`，不是
prompt parser 的输出。

### 模板

当前模板：

- `Review PRs`
- `Reply drafts`
- `Calendar RSVP`
- `Morning digest`

每个模板都会映射到名称、说明、图标和初始 trigger。

## 当前 Trigger 模型

Trigger 以紧凑的句子式 row 展示。Trigger picker 的设计目标仍然是：

- `Every day at` `09:00`
- `Custom schedule` `Cron`
- `PR opened in` `Select repos` `by` `Anyone`
- `Draft opened in` `Select repos`
- `Comment added in` `Select repos`
- `Label changes in` `Select repos`

已添加的 trigger 可以通过 trigger picker 修改，也可以在 row 上删除。点击 picker 外部
会关闭 trigger picker。

实现进度：

- 主要 trigger 菜单现在来自 daemon `automation_catalog` 结果。
- 当前 catalog-backed 家族包括 `Webhook`、schedule triggers（`Every day at`,
  `Custom schedule`），以及支持 workflow trigger 的 connector template 对应的
  connector event triggers。
- 带 required inputs 的 catalog trigger 会在 row 下方渲染 inline 配置字段。
- 如果 catalog 无法加载，UI 会回退到上面列出的本地 starter triggers。

当前限制：

- 虽然 `AutomationSpec.triggers` 支持多个 trigger，且 UI 显示的是 `Add Trigger`，
  但 UI state 里目前只表示一个 trigger。
- 现有多 trigger 或 rich trigger spec 还不能在 UI 里完整编辑；当 spec 不能被 UI
  round-trip 时，保存会保留隐藏字段。
- Connector trigger 的 label 和 input 已经来自 catalog，但仍然比较粗粒度；后续还需要
  更准确的来源 app、event 名称、app-specific required inputs，以及当前 generic
  filter 之外的配置状态。
- Trigger 搜索框已经可见，但还没有真正过滤 catalog，也没有搜索空结果状态。

## 当前 Tool 和 MCP 模型

Tool 按 app 的 API capability 粒度选择。一个 app 可以提供多个可选能力，每个能力都
会成为单独一行。

App 分组和能力的设计目标仍然是：

- GitHub: `Watch Pull Requests`, `Comment on Pull Request`, `Update Commit Status`
- Slack: `Read Slack Channels`, `Send to Slack`, `Reply in Slack Thread`
- Gmail: `Read Gmail Threads`, `Create Gmail Draft`, `Apply Gmail Label`
- Google Calendar: `Read Calendar Events`, `Check Availability`, `Draft RSVP`
- Linear: `Read Linear Issues`, `Create Linear Issue`, `Comment on Linear Issue`
- Notion: `Search Notion`, `Create Notion Page`, `Update Notion Page`

带目标或模式的能力会展示 inline target chip，例如
`Send to Slack` `to` `#teams`。target chip 当前会在本地候选项之间切换。

已选择的 tool 可以编辑或删除。点击 picker 外部会关闭 tool picker。

`Memories` 始终作为内置 context tool 展示。

实现进度：

- Picker 现在使用 daemon catalog actions，不再硬编码 app mock。
- daemon 当前暴露的 catalog-backed capability 是 Local Runtime:
  `Local JavaScript Transform`。
- Tool picker 搜索会过滤 app group 和 capability。
- Catalog-backed row 会在 action metadata 存在时显示连接、权限和
  approval-required 状态。

当前限制：

- Connector actions 和 MCP tools 还没有大范围暴露；daemon catalog 目前暴露的是 local
  transform action 和 connector-backed triggers。
- Required action inputs 已经存在于 catalog type 中，但 UI 还没有完整 editor。
- 部分 side-effect action 的可选 target 仍然是本地 UI 候选，直到真实 connector/MCP
  target discovery 接入。

## 当前 Runtime 模型

创建页和详情页包含 `Run location` 区域：

- `Local`
- `AgentEnv Cloud`

新 automation 默认使用当前 workflow backend mode。`Configure Runtime` 会打开
Automation Runtime 设置面板。保存时会把 `run_location` 写入 `AutomationSpec`；启用
automation 时会通过所选 runtime 路径 compile 和 deploy。

## 当前详情页

点击已保存的 automation 卡片会打开完整详情页。

顶部栏：

- 返回 `Automations` 的面包屑。
- `Test Run`。
- `Save`。
- 带 `Delete` 的更多菜单。

身份区域：

- 可编辑 automation 名称。
- Active toggle。
- Owner 文案，目前是 `You`。

Tab：

- `Settings`
- `Run History`

### Settings Tab

Settings 复用创建页的控制：

- Trigger row 和 trigger picker。
- Instructions 输入区域。
- Tool rows 和 tool picker。

修改会先保存在本地编辑态里。用户点击 `Save` 后会携带当前 record revision 调用
`automation_save`，把返回的 daemon record 更新到本地列表，并刷新标题、描述、状态、
trigger 摘要、已选 tools、启用状态、runtime 状态和图标。

### Run History Tab

没有运行记录时展示 `No runs yet`。

这个 tab 包含 `Test input` 编辑器。用户可以在 preview 前粘贴 JSON event object
或纯文本。JSON object 会作为 preview input 发送；纯文本会包装成 text payload。

面向 review 的 run row 设计目标仍然是：

- Title: `Test run`
- Status: `Waiting for review`
- Started: `Just now`
- Duration: `-`
- Summary: `Puffer is checking the current configuration.`

实现进度：

- 点击 `Test Run` 会先创建一条本地 `Running` row，并自动切换到 `Run History` tab。
- 随后它会保存当前详情页修改，调用 `automation_sync_preview`，再用解析后的 test
  input 调用 `automation_run_preview`。
- 本地 running row 的 summary 是
  `Puffer is running the current configuration through daemon preview.`

daemon preview 完成后，本地 row 会替换成 preview 结果或错误。daemon 也会把持久化
run-history record 追加到 `automation_runs.json`，包含 status、source event、duration、
runtime status、结构化 result 或 error，以及 preview approval metadata。打开详情页或
完成 preview 后，会通过 `automation_run_history` 刷新 run history。

运行后，`Result preview` 会在完整 run-history 列表前展示最近一次运行的 summary、
结构化输出或错误。

### 删除

更多菜单会打开一个紧凑操作菜单。点击 `Delete` 会调用 `automation_delete`，从本地列表
移除当前 automation，并返回首页。

## State 边界

当前 UI 实现位于 `apps/puffer-desktop/src/lib/screens/Automation.svelte`。

重要本地 state：

- `screenMode`: `home`, `new`, `detail`。
- `savedAutomations`: 本地已保存的用户 automation。
- `selectedAutomationId`: 当前选中的详情 automation。
- `automationName`, `automationPrompt`, `automationTrigger`, `selectedTools`,
  `automationEnabled`: 当前草稿或详情编辑态。
- `activeAutomationLibraryTab`: 首页列表 tab。
- `activeAutomationDetailTab`: 详情页 tab。
- `triggerMenuOpen`, `toolMenuOpen`, `automationActionMenuOpen`: 弹窗状态。
- `automationLoadError`, `automationCatalogError`, `automationSaving`,
  `automationStatusChanging`, `automationRunning`: daemon 交互状态。
- `triggerCatalog`, `commonApps`: daemon catalog 状态。
- `automationRunLocation`: 当前选择的 runtime location。

后端实现目前包括：

- `crates/puffer-automation`: typed `AutomationSpec`, `AutomationRecord`、
  validation、hashing、storage 和 compiler support。
- `crates/puffer-cli/src/daemon_automations.rs`: `automation_list`,
  `automation_get`, `automation_save`, `automation_delete`, `automation_catalog`。
- `crates/puffer-cli/src/daemon_automation_runtime.rs`: `automation_compile_deploy`,
  `automation_sync_preview`, `automation_run_preview`, `automation_run_history`、
  runtime compilation、local/cloud execution、generated workflow bindings 和
  run-history storage。

剩余后端缺口主要是更完整的 connector/MCP action catalog coverage、更丰富的
trigger/action 配置、approval UI integration，以及所有 trigger 类型上的 deployed
scheduling 生产级打磨。

## 已补上的交互

当前已经实现：

- 从侧边栏打开 Automation。
- 从首页 prompt 创建。
- 从 `new` 创建。
- 从模板卡片创建。
- 通过 daemon RPC 保存 automation。
- 取消创建。
- 打开已保存 automation 的详情页。
- 在详情页重命名 automation。
- 在详情页编辑 instructions。
- 在详情页切换 active 状态。
- 带 revision check 保存详情页修改。
- 添加、编辑、删除 UI trigger row。
- 渲染 catalog-backed trigger 配置字段。
- 添加、编辑、删除、切换 tool target。
- 在 tool picker 里选择 app API capability。
- 搜索和过滤 tool picker capability。
- 点击外部关闭 trigger 和 tool picker。
- 在 `Settings` 和 `Run History` 之间切换。
- sync 并执行 daemon test-run preview。
- 编辑 test-run 输入，并预览最新运行结果。
- 加载持久化 daemon run history。
- 通过 compile/deploy 启用 automation，通过保存 status 暂停 automation。
- 打开更多菜单并删除 daemon-backed automation。
- 在允许修改 title/instruction 的同时，保留不支持 UI round-trip 的 rich Automation spec。
- 选择 Local 或 AgentEnv Cloud run location，并链接到 runtime settings。
- 当前页面自有可见文案尽量使用 automation 语义，避免多余的 automation 堆叠。

## 还没补上的交互

### 创建和编辑

- 一个 automation 内多个 trigger 的完整 UI 编辑。
- 更丰富的 connector-backed trigger 选项，包括准确的来源 app、event 名称、必填输入
  和配置状态。
- 当前 local transform catalog action 之外的真实 connector actions 和 MCP tools，包括
  能力名称、必填输入、可选 target 和权限要求。
- Generic catalog inputs 之外的 trigger 专属配置面板，例如 repo picker、cron editor、
  contact picker、calendar picker、label picker。
- Trigger target chip 的手动编辑。
- 创建页和详情页里的独立模型选择器。
- 当前 run location picker 和 settings link 之外的 runtime health、credential 和
  workspace 详情。
- 脏状态、未保存离开提示、保存成功反馈。
- 使用 Escape 关闭弹窗。
- Trigger 和 tool 菜单内更完整的键盘导航。
- 点击外部关闭更多菜单。
- 真正生效的 Trigger 搜索过滤。
- Trigger 搜索空结果状态。
- Picker 打开时，更清楚地区分“新增 tool”和“编辑已有 tool”。
- Duplicate automation。
- 卡片上的 archive 或 pause 操作。

### 首页和列表

- 搜索或过滤已保存 automations 和 templates。
- 按最近更新、名称、状态或来源排序。
- 保存卡片上更明确的 status chips。
- 卡片级快捷操作。
- Template 分类。
- 打开创建页前的 template 详情预览。
- 导入或粘贴已有 automation 配置。

### 详情页

- Run history 过滤。
- Run history 详情 drawer 或 timeline。
- Test run 输入来源，例如选择 sample event 或历史消息。
- 在当前 summary 和结构化结果预览之外，补包含 generated draft、上下文和错误的
  test-run 结果预览。
- 更明确的 Active toggle 成功、失败和 pending 反馈。
- 删除确认。
- 对危险或不可用操作展示 disabled 状态。
- Owner 选择器或分享信息。
- 最近保存时间。

### Review 和 Approval

- Review inbox。
- Pending draft review 详情页。
- 可编辑的 proposed action 或 draft output。
- Approve、reject、snooze、edit 决策控件。
- Outward action 的 destination preview。
- Reject reason 记录。
- 清晰的 audit trail，显示谁在什么时候批准了什么。

### 后端和合约

- 更完整的 connector-backed tool capability discovery。
- 覆盖所有 triggers 和 tools 的完整 permission 与 credential readiness 状态。
- 后端合约返回的 field-level validation errors。
- 覆盖所有 trigger 类型的、经过打磨的真实执行调度。
- Workspace 或 team policy 约束。

## 建议的下一步设计

1. 给创建页和详情页补脏状态与保存反馈。
2. 优先补 GitHub repo 和 schedule 相关的 trigger 专属配置。
3. 扩展 test-run preview，支持保存的 sample events 和历史 connector messages。
4. 设计 review inbox 和 approval 详情页。
5. 扩展后端合约，覆盖更丰富的 connector actions、MCP tools、trigger configs、
   field-level validation 和 approval metadata。
6. 补删除确认，以及 duplicate/archive 操作。

## 验证资产

当前 UI 覆盖位于 `apps/puffer-desktop/tests/automation-ui.spec.ts`。

测试覆盖：

- Prompt-first home。
- `Your automations` 空态。
- Template library。
- Builder 布局和控件。
- Trigger 和 tool picker 行为。
- Capability-level tool selection。
- 保存卡片创建。
- Daemon-backed save/update/delete 和 activation flow。
- Runtime location 默认值和 runtime settings 链接。
- 详情页 settings。
- Run history 空态、test-run 输入、daemon preview 和结果预览。
- UI 编辑时保留 unsupported rich Automation spec。
- 更多菜单里的 delete 可见性。
- Segmented-control 背景对比度。
