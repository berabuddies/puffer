# Momo Onboarding 用户画像写入 puffer 全局 memory — Design Spec

- Date: 2026-05-30
- Author: sean (with Claude)
- Status: Approved for planning
- Scope owner: momo desktop (`apps/momo`)
- 关联背景: `apps/momo/CLAUDE.md`（memory / provider / 两条通信链路）；`crates/puffer-core/runtime/system_prompt.rs`（memory 注入机制）

## 1. 背景与目标

momo 的 onboarding（Where → Role → Apps → Done）目前每屏只把选择存在页面本地 `$state` 并弹 toast，走完即丢——用户填的国家、职业没有任何通道传给 puffer agent。本 spec 做两件相关的事：

1. **写画像**：onboarding 完成时把 **国家 + 职业** 写进 puffer 的**用户级全局 memory**，使后续所有对话里 agent 都知道"用户在哪、做什么"。
2. **修 onboarding gate**：onboarding 是**电脑用户级一次性 gate**——完成**或** Skip 后都不该再出现入口，**切换账号也不行**。当前完成路径已正确落 flag，但从 Where/Role 页 Skip 会绕过它（见 §11），需补齐。

### puffer 全局 memory 机制（调研结论，实现前必读）

`crates/puffer-core/runtime/system_prompt.rs::load_memory_prompt` 在每次组装 system prompt 时，从磁盘 fresh 读取 memory 文件（无缓存、无需重启 daemon）：

- 来源固定三处：`<session cwd>/<file>`、`$HOME/.claude/<file>`、`$HOME/.puffer/<file>`。
- **provider 决定文件名**：`openai` provider 优先读 `AGENTS.md`，**只有三处都没有 AGENTS.md 时才回退 `CLAUDE.md`**；其它 provider（anthropic 等）直接读 `CLAUDE.md`，**根本不读 AGENTS.md**。
- momo 全程用 `openai` provider（worldrouter 走 openai 兼容，见 `src/lib/agent/daemonAuth.ts`），所以 momo 会话读 `AGENTS.md`。

由此，"用户级全局画像（对该机器所有 puffer 会话生效，与 cwd 无关）"的落点 = `$HOME/.puffer/AGENTS.md`（覆盖 openai 会话）+ `$HOME/.puffer/CLAUDE.md`（覆盖 anthropic 等终端会话）。

> 注：`apps/momo/CLAUDE.md` 旧表述"没有用户级全局 memory"不准确——`~/.puffer/{AGENTS,CLAUDE}.md` 即用户级全局注入。本 spec 落地后顺带订正该文档。

### 关键约束

- daemon 的 `write_file` RPC **不能创建新文件**（`crates/puffer-cli/src/daemon_files.rs::handle_write_file` 先 `metadata`/`canonicalize`，文件不存在直接 `bail!`），且 onboarding 早于 chat 登录、daemon 连接未必就绪。因此**不**走 daemon 写，由 momo 自己的 Tauri backend（Rust，完整 FS 权限）直接落盘。

## 2. 决策记录（已敲定）

1. **作用域 = B2 全局**：写 `~/.puffer/AGENTS.md` **和** `~/.puffer/CLAUDE.md` 两个文件，provider 无关地覆盖所有会话。
2. **画像内容 = ① 国家 + 职业**。已连接的 App 属 connector 状态，不写入（易过期，另有体系管）。
3. **写入时机 = A 走到 Done 一次性写**；中途 Skip（没到 Done）不写。
4. **写入方式 = 方案 1**：前端经现有 1431 链路调 momo backend 新 RPC，由 Rust 直接落盘 + 托管块合并。
5. **副作用知情接受**：写 `~/.puffer/AGENTS.md` 后，openai 会话因"AGENTS.md 命中即提前返回"将不再注入任何 `CLAUDE.md`（含 `~/.claude/CLAUDE.md`）。已知并接受。
6. **onboarding gate = 机器级一次性**：Skip = 视为已 onboarded。修法选 **A**——在 `OnboardingShell.onSkip()` 统一 `markOnboarded()`（不加 Home 兜底）。画像不写（Skip 没采集），但 gate flag 落，入口不再出现。

## 3. 数据流

```
Where.svelte  --setCountry-->┐
Role.svelte   --setRole----->┤ onboarding store ($state, 模块级)
                             │
Done.svelte (onMount) --commitProfile()
   └─> wsClient.request("write_user_profile", { country, role })
         └─> backend.rs: write_user_profile
               └─> user_profile::upsert_managed_block()
                     └─ 写 ~/.puffer/AGENTS.md + ~/.puffer/CLAUDE.md
下一次 run_agent_turn → daemon load_memory_prompt 读两文件 → 注入 system prompt
```

无需重启 daemon（memory 每 turn 从磁盘 fresh 读）。

## 4. 组件设计（职责单一、可独立测）

### ① `src/lib/onboarding.svelte.ts`（新）— 跨页收集器
- 模块级 Svelte 5 `$state`：`{ country: string | null, role: string | null }`。
- 导出 `setCountry(c)` / `setRole(r)` / `commitProfile()`。
- 沿用 `sessionStore.svelte.ts` / `projectStore.svelte.ts` 的模块级 store 模式（onboarding 各页是独立路由，本地 `$state` 无法跨页收集）。
- `commitProfile()` 调 `wsClient.request("write_user_profile", { country, role })`，fire-and-forget；失败弹 toast，**不阻断** Done → /home 的导航。

### ② `Where.svelte` / `Role.svelte` — 采集选择
- 在现有 `pick` / `pickPreset` / 自定义输入回调里**追加** `setCountry` / `setRole`，其余行为（toast、300ms 自动前进、自定义输入 debounce）不动。
- Role 取最终生效的 `selected`（含自定义输入的字符串）。

### ③ `Done.svelte` — 触发提交
- `onMount` 调 `commitProfile()`。完成路径的 `markOnboarded()` 已存在，不动。

### ③' `src/components/onboarding/OnboardingShell.svelte` — Skip 落 gate flag
- `onSkip()` 在 `navigate(skipTo)` 前调 `markOnboarded()`。一处覆盖 Where/Role/Apps 全部 Skip 链接（都共用此 shell）。
- 这是修 onboarding gate 的全部代码改动（账号切换已天然满足，见 §11）。

### ④ `src-tauri/src/user_profile.rs`（新）— 纯合并逻辑
- `upsert_managed_block(existing: &str, block: &str) -> String`：
  - 定位 BEGIN/END 标记行；
  - 两标记齐全 → 替换标记间内容（含标记）为新块；
  - 无标记 → 在末尾追加新块（空行分隔）；
  - 仅半个标记（残缺）→ 视为无有效块，追加新块；
  - 保证结尾换行、保留其余原文。
- 纯函数，便于 Rust 单测。

### ⑤ `src-tauri/src/backend.rs` — 新 RPC `write_user_profile`
- 在 `handle` 的 `match method` 加分支。
- 解析 `country` / `role`（沿用 `optional_string_param`）。
- 解析 `$HOME/.puffer`（env `HOME`，与 daemon / system_prompt 解析一致）→ `create_dir_all`。
- 构造托管块（见 §5）；对 `AGENTS.md`、`CLAUDE.md` 各 read-or-empty → `upsert_managed_block` → 写回。
- 返回 `{}`（或写入路径，便于测试断言）。

## 5. 托管块格式

```markdown
<!-- BEGIN momo-user-profile (managed by onboarding) -->
## About the user
- Lives in: United States
- Role / occupation: Founder
<!-- END momo-user-profile -->
```

- 某项为空 → 省略该 bullet；两项都空 → 不写文件。
- BEGIN/END 标记字符串固定，用于幂等定位替换。

## 6. 边界与错误处理

- `HOME` 解析失败 → backend 返错；前端 toast，onboarding 照常完成（非阻断）。
- 文件已有内容但无标记 → **追加**块，原内容不动。
- 仅半个标记（残缺）→ 视为无有效块，追加新块。
- 重跑 onboarding → 替换块，幂等（写两次结果 == 写一次）。
- Skip（没到 Done）→ 不写。
- 并发：onboarding 单次触发，不涉及并发。

## 7. 测试

- **Rust 单测**（`user_profile.rs`，纯函数）：
  - 空输入 → 生成块；
  - 有内容无标记 → 追加且原文保留；
  - 有标记 → 替换且周围内容不动；
  - 幂等（写两次 == 写一次）；
  - 残缺标记 → 追加新块。
- **前端**（扩展/新增 Playwright，复用 `tests/support/fakeDaemon.ts` + `bootHelpers.ts`）：选国家 + 职业 → 到 Done → 断言 `write_user_profile` 以 `{ country, role }` 被调用。
- **手动验收**：跑 onboarding → 查 `~/.puffer/AGENTS.md` + `CLAUDE.md` 含托管块 → 起 chat 确认 agent 知道用户国家/职业。
- **onboarding gate**（扩展 `tests/onboarding-persistence.spec.ts`）：从 Where Skip → `puffer.onboarded === "true"` 且 root 解析为 `/home`（不回 onboarding）；从 Role Skip 同样。补齐现有"完成 + signOut 保留 + 账号切换"用例之外的 Skip 路径。

## 8. 顺带订正文档

实现后更新 `apps/momo/CLAUDE.md`"memory 是 project 级，没有用户级全局 memory"一段：补充 `~/.puffer/{AGENTS,CLAUDE}.md` 是用户级全局注入、provider 决定 AGENTS.md vs CLAUDE.md、momo 现往此写 onboarding 画像。

## 9. 验证命令

```bash
cd apps/momo
npm run check
npm run test:desktop-ui
cargo check --manifest-path src-tauri/Cargo.toml
cargo test  --manifest-path src-tauri/Cargo.toml user_profile
```

## 10. 非目标（YAGNI）

- 不写已连接 App 列表 / 不引入结构化用户档案 schema。
- 不做 onboarding 后的"编辑画像" UI（重跑 onboarding 即可覆盖）。
- 不改 puffer-core 的 memory 注入机制（现机制已够用）。
- 不做 project 级 `MEMORY.md` 注册（本 spec 走全局 AGENTS/CLAUDE.md）。

## 11. onboarding gate 现状与缺口（调研结论）

flag `puffer.onboarded` 存 localStorage，是**机器级**判断（`src/lib/auth.svelte.ts`）：

- `isOnboarded()` 读，`markOnboarded()` 写（幂等），`clearOnboarded()` 定义了但**全代码库无人调用**。
- gate 在 `getRootRedirect()`（`routes.ts:100`）：`signedIn && !isOnboarded()` → `/onboarding/where`，否则 `/home`。仅在根路径 `/` 解析时生效。
- **账号切换已天然满足**：`signOut()`（auth.svelte.ts:786）只删 `TOKEN_KEY`/`REFRESH_TOKEN_KEY`/`API_KEY_KEY`/`API_KEY_OWNER_KEY`，**不删** `ONBOARDED_KEY`。已 onboarded 的机器，signOut→换账号登录后 `isOnboarded()` 仍为 true，不重弹。现有 `tests/onboarding-persistence.spec.ts` 已 pin。

**唯一缺口 = Skip 未统一落 flag**：

| 退出方式 | 路径 | 当前是否落 flag |
|---|---|---|
| 完成 | → `Done.svelte` onMount `markOnboarded()` | ✓ |
| Apps 页 Skip | `skipTo="/onboarding/done"` → Done | ✓ |
| Where 页 Skip | `skipTo="/home"` 直接跳 | ✗ |
| Role 页 Skip | `skipTo="/home"` 直接跳 | ✗ |

从 Where/Role Skip 落 `/home` 但不写 flag → 下次启动 root 解析 `!isOnboarded` → 又回 `/onboarding/where`。`Done.svelte` 注释提到 markOnboarded "safe to call on every /home hit"，但 `Home.svelte` 实际未兜底。修法 A（§4 ③'）在 `OnboardingShell.onSkip()` 统一 `markOnboarded()` 即补齐，不依赖 Home 兜底。
