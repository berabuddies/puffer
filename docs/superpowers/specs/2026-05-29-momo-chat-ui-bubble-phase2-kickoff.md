# Momo Chat UI 气泡化(阶段 2)— Kickoff / 交接 note

- Date: 2026-05-29
- Status: **brainstorming 进行中** —— 范围已定(全气泡化),设计细节待敲定。本文档供**新会话接续**:`invoke superpowers:brainstorming`,读本文档 + 引用的代码/commit,从 §6 开放问题继续走 design → spec → writing-plans → 实现。
- 关联:阶段 1 = `docs/superpowers/specs|plans/2026-05-29-momo-chat-daemon-migration*`(已完成 merged)。

## 1. 目标与范围
把当前 **desktop IDE 风格**的 `ConversationView` 换成 **momo 气泡 + Mascot 风格**,渲染**同一份 `TimelineItem[]`**。这是 chat 迁 daemon 路线图的阶段 2(阶段 1 已把数据/渲染切干净,就是为这一步留的)。范围 = 全气泡化(用户已确认),不是局部微调。

## 2. 起点(阶段 1 已落地,新会话先理解)
- chat 已**直连 puffer daemon**(merge `0de463d9` + `77fb1a9b`),不再 spawn `puffer non-interactive`。
- **数据/状态层**:`src/lib/agent/agentChat.svelte.ts`(controller,keyed-by-sessionId)+ `daemonChat.ts`(daemon RPC),产出标准 `TimelineItem[]`,管 turn 生命周期 / tool 归属 / replay 去重 / permission·question 往返。
- **渲染层**:`src/lib/agent/ConversationView.svelte`(从 puffer-desktop 照搬的 IDE 风格)——**这就是阶段 2 要替换的**。
- 架构全貌见 `apps/momo/CLAUDE.md` 的 "chat 架构" 章节(刚更新)。

## 3. 关键约束(必读,别踩)
1. **只动渲染层,别碰 controller / daemonChat。** 数据/状态已封装好;碰 controller 最容易把刚修的 **Bug2(多轮 tool 错位)** 弄回来。新气泡 UI 只消费 controller 的输出 + 触发其回调。
2. **别丢两个 bug fix 的成果**(两 bug puffer-desktop 同源也有,改时别回归):
   - `components/MessageBody.svelte` 的 **rAF 节流 + `{#each}` key** 是 streaming 抖动 fix —— 新气泡若渲染 markdown,复用它/保留这两点。
   - tool 顺序 controller 已保证对,**按 `TimelineItem[]` 顺序渲染**即可,**别再自创"把 tool 归到某轮"的分组**(那正是 Bug2 形态)。
3. **worktree 隔离**:主仓 `feat/momo-desktop` 正被别的 session(credits/kyc)活跃用 + dirty,从其 HEAD 切 worktree 做;**git 操作一律 `git -C <worktree>` 不能 cd**(见 memory `worktree-subagent-git-pitfall`,已踩坑 2 次);测试端口避开主仓 1466(用 1477/1478,merge 前 revert)。
4. **测试**:`npm run check` + `npm run test:desktop-ui`;fakeDaemon e2e(真实 API,默认 legacy protocol,参考现有 `tests/agent/*.spec.ts`),覆盖 发消息→气泡渲染 / permission / question / cancel。

## 4. 接口契约(新气泡 UI 要对接的)
**最关键:渲染层与 controller 的契约,新气泡 UI 必须沿用,Agent.svelte/Composer/controller 才能不动。**

- **`ConversationView` Props**(`src/lib/agent/ConversationView.svelte:31-61`,新组件要么实现同款 props、要么 `Agent.svelte` 改挂新组件):
  - 输入:`session` / `timeline: TimelineItem[]` / `pendingPermissions` / `pendingQuestions` / `resolvingPermissionIds?` / `resolvingQuestionIds?` / `loading` / `turnRunning?` / `turnCancelable?` / `turnStartedAtMs?` / `turnThinking?` / `turnStatusHint?` / `backendConnected?` / `userDisplayName?`(默认 "Otter")
  - 回调:`onSubmitMessage(message, options?)` / `onResolvePermission(permissionId, choice)` / `onResolveUserQuestion(questionId, answers, annotations?)` / `onCancelTurn?` / `onOpenFileLink?` / `onDraftChange?`
- **controller getter**(`agentChat.svelte.ts`,Agent.svelte 已在用):`combinedTimeline()` / `pendingPermissions()` / `pendingQuestions()` / `turnRunning()` / `state()`;动作 `appendUserMessage` / `createSessionFromText` / `resolvePermission` / `resolveUserQuestion` / `cancelCurrentTurn`。
- **`TimelineItem` 族**(`src/lib/agent/types.ts:178`)—— 气泡 UI 要渲染这 6 类:
  - `MessageTimelineItem`(kind `user`/`assistant`/`system`/`command`)、`ToolTimelineItem`(`tool`)、`PermissionTimelineItem`(`permission`)、`UserQuestionTimelineItem`(`question`)、`DiffTimelineItem`(`diff`)。
- 当前 ConversationView 内部 `buildRows` 把连续 tool 聚成 **"Agent activity" 折叠组** + 末尾 assistant 作主气泡 —— 气泡化要决定**保留这套 roll-up 还是改成纯气泡流**(见 §6)。

## 5. 可参考的现成资产
- **旧 momo 气泡组件**(阶段 1 `c96711a2` 删掉前的,git 可捞):`components/agent/{ChatBubble,ThinkingBlock,ToolCallPill,AnswerForm,ToolBlock,AgentText,OptionsCard,ResultCard}.svelte`。捞:`git show c96711a2^:apps/momo/src/components/agent/ChatBubble.svelte`。**⚠️ 它们当时吃旧 `ChatMessage` 类型,阶段 2 要改成吃 `TimelineItem`。** 这是 momo 原来的气泡风格起点(右气泡 + 友好中文 pill + prod 隐藏思考链)。
- **Mascot 还在**:`components/common/Mascot.svelte`(没被 T7 删,可直接用作助手头像)。
- **当前 desktop 子组件**(`src/lib/agent/components/`):`ToolCard` / `QuestionPrompt` / `Approval` / `DiffCard` / `MessageBody` / `HighlightedLine` / `Icon`。气泡化可**保留部分逻辑只改外层布局**(如 DiffCard/QuestionPrompt 的解析逻辑),或全重写;`MessageBody` 的节流+key 务必保留。

## 6. 待 brainstorm / 决策的开放问题(新会话从这继续)
1. **气泡布局**:用户右气泡 + 助手左气泡(带 Mascot 头像)?间距/圆角/配色?
2. **tool 调用呈现**:友好 pill(旧 `ToolCallPill` 风格 + 中文 `toolLabels`)折叠?还是保留 desktop 的 "Agent activity" 分组卡片?prod 是否隐藏技术细节(旧 momo 用 `debugFlags` DEV 才显 raw)?
3. **思考链(thinking-delta)**:折叠成 "I'm working on it..." pill(旧 `ThinkingBlock`)?
4. **权限审批 / askUserQuestion**:在气泡流里怎么呈现(旧 `AnswerForm` 单问单选 vs 当前 `QuestionPrompt` 多问多选)?
5. **diff**:气泡流里怎么展示(保留 `DiffCard` 还是简化)?
6. **roll-up vs 纯时序**:保留 ConversationView 的 final-response roll-up,还是改成纯按 TimelineItem 时序的气泡流?
7. **视觉风格基准**:旧 momo 气泡 / onboarding·home 的设计语言 / 有无设计稿(Figma)?—— 这步可能值得用 brainstorming 的 visual companion 出 mockup 对比。
8. **composer**:已用 momo shell `Composer.svelte`(阶段 1),气泡化是否动它?
9. permission/question 解决后当前是 **dismiss**(不留 answered 态),保留?

## 7. 怎么接续(给新会话)
1. `invoke superpowers:brainstorming`,读本文档 + §2/§4/§5 引用的代码与 commit。
2. 范围(全气泡化)已定,直接从 §6 开放问题逐条 clarify(气泡设计偏视觉,可 offer visual companion 出 mockup)。
3. 走完 design → 写 spec(`docs/superpowers/specs/`)→ writing-plans → 用 subagent-driven 在 **worktree** 实现。
4. 实现时严守 §3 约束(别碰 controller、保留两 bug fix、git -C、端口避让)。
