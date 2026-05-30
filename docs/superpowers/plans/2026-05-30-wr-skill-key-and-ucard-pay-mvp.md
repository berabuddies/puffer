# WorldRouter Skill Key 传递 + U-card 支付取卡 MVP 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 puffer agent 能用用户自己的 WorldRouter 账号驱动 `book-by-phone` 技能,并在需要支付时安全取用 U-card 的卡号/CVV——卡敏感数据全程不进 agent 上下文。

**Architecture:** 两个**相互独立**的子系统,可分别交付:
- **Part A(不敏感)**:`book-by-phone` 的 search/detail/book/poll 由 agent 经 bash 直接调 WorldRouter API,API key 经 `~/.wr/.creds` 文件传递(momo 登录时按环境写入)。
- **Part B(敏感)**:取卡号/CVV 走"前端取卡"(模式 B),agent 通过既有 `user-question-request` 通道发起,前端取卡 → 注入执行通道 → 只把非敏感结果回 agent。卡号/CVV 用完即扔。

**Tech Stack:** Tauri 2 + Svelte 5(momo 前端/Rust backend)、puffer-cli(Rust daemon + builtin skill)、ucard-backend(Go,卡数据合规后端)、WorldRouter control-api。

---

## 已锁定的 MVP 决策(来自需求讨论)

1. **全栈自有**:momo / puffer / ucard-backend 都是我们的代码,无外部团队对接。
2. **Part B 用模式 B**(前端取卡),**MVP 无 step-up 鉴权**,**先不做正式 PCI 合规审查**。
3. **环境切换**复用 momo 现有 `VITE_*` + `.env` + Vite build mode 机制(`VITE_WORLDROUTER_CONTROL_URL` / `VITE_BACKEND_BASE_URL`)。
4. Part A 执行方式"怎么方便怎么来"——本计划选 agent bash 直调(无需新写 bin)。

## ⚠️ 已知 MVP 债务(上线前必须回填,本计划不实现)

- **无 step-up**:agent 可在无用户当场确认下取用用户的卡。
- **PAN/CVV 经过前端 JS(WebView 渲染进程)**:暴露面高于 backend/bin 方案。
- **PCI scope**:模式 B 把客户端拉入 PAN/CVV 处理范围,需专业合规审查。
- **Part A 的 WorldRouter key 会进 agent bash 上下文**(`source` 后的 env);book 阶段判定可接受。

## 仍需拍板的小决策(带推荐;不阻塞动笔,影响 Part B 末端)

- **D-SINK(Part B)**:取来的卡号/CVV **注入给谁**?(电话担保填给 WorldRouter 语音服务 / agent 在某表单填 / 其它)。推荐 MVP 先实现**通用取卡 API + 一个可插拔 sink**,首个具体 sink 由你指定。Task B5 处理。
- **D-TRIGGER(Part B)**:agent 发起取卡用 **既有 `user-question-request`**(无需改 daemon 协议,推荐 MVP)还是**新增专用工具**(更干净,记为债务)。Task B3 按推荐走 user-question。

---

# Part A — book-by-phone 的 API key 传递

## A.现状

- `book-by-phone` skill 已落地(commit `40942482`):`resources/skills/book-by-phone/SKILL.md` 当前是 `curl -H "Authorization: Bearer $WORLDROUTER_API_KEY"` + 文档说"需 env var"的形态。
- 目前**没有任何机制把 key 送到 agent 的执行环境**——这就是 Part A 要补的。
- momo 登录后已有 worldrouter 凭据:`~/.puffer/auth.json` 存有 `worldrouter` provider;前端 `auth.svelte.ts` 的 `ensureWrSession` / `creditStore` 已用 `VITE_WORLDROUTER_CONTROL_URL` 访问 control-api。

## A.文件结构

- 新增 `apps/momo/src-tauri/src/wr_creds.rs` — Tauri command `write_wr_creds`,把 `{apiKey, baseUrl}` 写 `~/.wr/.creds`(0600)。
- 修改 `apps/momo/src-tauri/src/lib.rs`(或现有 command 注册处)— 注册 `write_wr_creds`。
- 修改 `apps/momo/src/lib/auth.svelte.ts` — 登录成功(拿到 wr key)后调用 `write_wr_creds`。
- 修改 `resources/skills/book-by-phone/SKILL.md` — Authentication / Quick start 改成 `source ~/.wr/.creds` 再 curl。

## Task A1: momo 写 `~/.wr/.creds`(Rust backend command)

**Files:**
- Create: `apps/momo/src-tauri/src/wr_creds.rs`
- Modify: `apps/momo/src-tauri/src/lib.rs`(命令注册 `invoke_handler`)
- Test: `apps/momo/src-tauri/src/wr_creds.rs`(`#[cfg(test)]` 模块)

- [ ] **Step 1: 写失败测试**(校验格式化 + 路径解析,纯函数部分)

```rust
// wr_creds.rs
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn formats_creds_file_body() {
        let body = render_creds("wr_live_abc", "https://control-api.worldrouter.ai");
        assert_eq!(
            body,
            "WORLDROUTER_API_KEY=wr_live_abc\nWORLDROUTER_BASE_URL=https://control-api.worldrouter.ai\n"
        );
    }
}
```

- [ ] **Step 2: 运行,确认失败**

Run: `cargo test --manifest-path apps/momo/src-tauri/Cargo.toml render_creds`
Expected: FAIL（`render_creds` 未定义）

- [ ] **Step 3: 最小实现**

```rust
// apps/momo/src-tauri/src/wr_creds.rs
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;

fn render_creds(api_key: &str, base_url: &str) -> String {
    format!("WORLDROUTER_API_KEY={api_key}\nWORLDROUTER_BASE_URL={base_url}\n")
}

#[tauri::command]
pub fn write_wr_creds(api_key: String, base_url: String) -> Result<(), String> {
    let home = std::env::var("HOME").map_err(|e| e.to_string())?;
    let dir = std::path::Path::new(&home).join(".wr");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(".creds");
    // 0600: owner read/write only — contains a secret.
    let mut f = std::fs::OpenOptions::new()
        .write(true).create(true).truncate(true).mode(0o600)
        .open(&path).map_err(|e| e.to_string())?;
    f.write_all(render_creds(&api_key, &base_url).as_bytes())
        .map_err(|e| e.to_string())?;
    Ok(())
}
```

- [ ] **Step 4: 注册 command**

`apps/momo/src-tauri/src/lib.rs` 的 `tauri::generate_handler![...]` 加入 `wr_creds::write_wr_creds`,并在文件顶部 `mod wr_creds;`。

- [ ] **Step 5: 运行测试,确认通过**

Run: `cargo test --manifest-path apps/momo/src-tauri/Cargo.toml render_creds`
Expected: PASS

- [ ] **Step 6: 提交**

```bash
git add apps/momo/src-tauri/src/wr_creds.rs apps/momo/src-tauri/src/lib.rs
git commit -m "feat(momo): write_wr_creds command — persist WorldRouter key to ~/.wr/.creds (0600)"
```

## Task A2: 登录后调用 write_wr_creds(前端)

**Files:**
- Modify: `apps/momo/src/lib/auth.svelte.ts`(`ensureWrSession` 解析出 key 之后的收尾处)

- [ ] **Step 1: 在拿到 wr key 处调用 command**

在 `auth.svelte.ts` 中,登录/`ensureWrSession` 成功拿到 worldrouter API key 后:

```ts
import { invoke } from "@tauri-apps/api/core";

// base_url 取当前环境(test/prod 由 .env 决定),与 creditStore 同源
const baseUrl =
  (import.meta.env.VITE_WORLDROUTER_CONTROL_URL as string | undefined) ??
  "https://control-api.worldrouter.ai";

// 写入凭据文件,供 book-by-phone skill 的 agent bash 读取。
// 失败不阻断登录(skill 自己会在缺 key 时报错)。
try {
  await invoke("write_wr_creds", { apiKey: wrApiKey, baseUrl });
} catch (e) {
  console.warn("write_wr_creds failed", e);
}
```

> 注:`wrApiKey` 用 `auth.svelte.ts` 里已解析的 worldrouter key 变量名(实现时按现有变量对齐)。

- [ ] **Step 2: 验证(手动)**

```bash
# momo 登录后:
ls -la ~/.wr/.creds          # 期望 -rw------- (0600)
grep -c WORLDROUTER_API_KEY ~/.wr/.creds   # 期望 1
```

- [ ] **Step 3: 提交**

```bash
git add apps/momo/src/lib/auth.svelte.ts
git commit -m "feat(momo): write ~/.wr/.creds on login for book-by-phone skill"
```

## Task A3: SKILL.md 改成 `source ~/.wr/.creds` + 重编译

**Files:**
- Modify: `resources/skills/book-by-phone/SKILL.md`(Authentication 段 + Quick Start 的 curl)

- [ ] **Step 1: 改 Authentication 段**

把"需要 env var / 从环境变量读"改为:

```markdown
## Authentication

All requests require `Authorization: Bearer $WORLDROUTER_API_KEY`.

**Loading credentials:** Before calling the API, load the user's credentials:

    source ~/.wr/.creds

This sets `WORLDROUTER_API_KEY` and `WORLDROUTER_BASE_URL` for the curl
calls below. The file is written by the host app at login (0600). If it is
missing, tell the user they need to sign in first — do not invent a key.
```

- [ ] **Step 2: Quick Start 的每个 curl 前加 `source`**

`BASE="$WORLDROUTER_BASE_URL/v1/services/lifeclaw/skill"` 前加一行 `source ~/.wr/.creds`。

- [ ] **Step 3: 重编译 puffer(builtin skill 嵌入二进制)**

Run: `cargo install --path crates/puffer-cli --force`
Expected: `Finished release` + `Replacing ~/.cargo/bin/puffer`
验证嵌入: `grep -ac 'source ~/.wr/.creds' ~/.cargo/bin/puffer`(>0)

- [ ] **Step 4: 端到端验证**

重启 momo → 用"帮我打电话订位"类意图触发 skill → 观察 agent 是否 `source ~/.wr/.creds` 后对 `$WORLDROUTER_BASE_URL` 发起 search。Test 环境下应连到 `control-api-test-…`。

- [ ] **Step 5: 提交**

```bash
git add resources/skills/book-by-phone/SKILL.md
git commit -m "feat(skills): book-by-phone reads key from ~/.wr/.creds via source"
```

---

# Part B — U-card 敏感卡数据获取(卡号/CVV)

> ⏸ **ON HOLD — 待后续详聊,本段暂不实现(TODO)。**
> 当前只实施 **Part A**。Part B 的任务清单(B1–B6)保留在下方作为待办。
> **开聊前必须先回答的生死前提**:Task B1 —— ucard-backend / Strada 能否**程序化 reveal CVV**?
> - 能 → 按本段"前端取卡"方案细化。
> - 不能 → 整个前端取卡作废,回退**模式 A(卡不出后端、后端代付)**,Part B 需重设计。
> 其余待详聊点:D-SINK(卡注入给谁)、D-TRIGGER(user-question vs 新工具)、step-up 与 PCI(MVP 后回填)。

## B.现状

- U-card 后端 `ucard-backend`(Go);momo 前端 `walletApi.ts` 已接 `/card/list`/`/card/balance`/`/card/transactions` 等**只读非敏感**端点,**只拿 `maskedCardNumber`**。
- reveal 卡详情的端点(`getCardDetails`)在 `walletApi.ts` 注释里标为"out-of-scope, 故意没接"——Part B 要接它。
- 鉴权链已有:Auth Station JWT(`puffer.authToken`)→ `/auth/exchange` 换 ucard `sessionToken`(`ucardSession.ts`)→ `backendFetch` 带 Bearer。**Part B 直接复用这条链**。
- agent↔前端通道:`user-question-request`(daemon→前端)+ `resolveUserQuestion`(前端→daemon),见 `sessionEvents.ts` / `daemonChat.ts`。

## B.文件结构

- (ucard-backend, 跨 repo)`GET/POST /card/getCardDetails` — 按契约返回 `{pan, cvv, expMonth, expYear, nameOnCard}`,sessionToken 鉴权。**契约见 Task B1**。
- 修改 `apps/momo/src/lib/walletTypes.ts` — 加 `SensitiveCardDetails` 类型 + `revealCardDetails` 到 `WalletClient` 接口。
- 修改 `apps/momo/src/lib/walletApi.ts` — 实现 `revealCardDetails(cardId)`(**返回局部值,绝不进 store**)。
- 新增 `apps/momo/src/lib/agent/paymentBridge.ts` — 拦截带 payment 标记的 `user-question-request`,取卡 → 注入 sink → resolve 非敏感结果;含"用完即扔"。
- 修改 `apps/momo/src/lib/agent/agentChat.svelte.ts` — 在 user-question 分发处,把 payment 标记的请求路由给 `paymentBridge`。

## Task B1: 定义 reveal 端点契约(ucard-backend)

> 跨 repo,本计划只定**契约**;实现由懂 ucard-backend 的人按此做。MVP 不加 step-up。

- [ ] **Step 1: 契约**

```
POST /api/card/getCardDetails
Auth: Bearer <ucard sessionToken>   (与现有 /card/* 一致)
Body: { "cardId": <int> }
200 envelope { code:0, data: {
  pan: string,            // 完整卡号
  cvv: string,            // ⚠️ 实时返回,服务端不得持久化(PCI 3.2)
  expMonth: string,       // "04"
  expYear: string,        // "2028"
  nameOnCard: string
}}
错误码沿用现有 envelope(1005/1007/1008/1009 = sessionToken 失效)
```

- [ ] **Step 2: 确认预付卡 CVV 语义**

在 ucard-backend 确认:U-card 的 CVV 是静态还是动态?若动态,`getCardDetails` 必须返回当前有效值。把结论记到本文件。

- [ ] **Step 3: 后端实现 + 自测后,提交(ucard-backend repo)**

```
确认:Strada 处理器是否允许程序化 reveal CVV;若仅 PAN 可 reveal、CVV 不可,则 Part B 的支付路径要改为"凭 cardId 由后端代付"(回到模式 A)——这会推翻 D-SINK 的前端注入前提,需要回到设计层。
```

## Task B2: 前端 revealCardDetails(不进 store)

**Files:**
- Modify: `apps/momo/src/lib/walletTypes.ts`
- Modify: `apps/momo/src/lib/walletApi.ts`

- [ ] **Step 1: 加类型 + 接口方法**

```ts
// walletTypes.ts
export interface SensitiveCardDetails {
  pan: string;
  cvv: string;
  expMonth: string;
  expYear: string;
  nameOnCard: string;
}
// WalletClient 接口加:
//   revealCardDetails(cardId: number): Promise<SensitiveCardDetails>;
```

- [ ] **Step 2: 实现(RestWalletClient)**

```ts
// walletApi.ts — RestWalletClient
async revealCardDetails(cardId: number): Promise<SensitiveCardDetails> {
  // 复用现有 backendFetch(带 ucard sessionToken)。
  // ⚠️ 调用方必须把返回值当“用完即扔的局部变量”:不得写入任何 $state /
  //    store / localStorage,不得 console.log,不得回传 agent。
  const d = await backendFetch<{
    pan: string; cvv: string; expMonth: string; expYear: string; nameOnCard: string;
  }>('/card/getCardDetails', { method: 'POST', body: { cardId } });
  return { pan: d.pan, cvv: d.cvv, expMonth: d.expMonth, expYear: d.expYear, nameOnCard: d.nameOnCard };
}
```

- [ ] **Step 3: 验证(手动,临时,验完删)**

dev 下临时调一次 `walletApi.revealCardDetails(<cardId>)`,确认拿到字段;**确认后删除临时调用,不留任何打印**。

- [ ] **Step 4: 提交**

```bash
git add apps/momo/src/lib/walletTypes.ts apps/momo/src/lib/walletApi.ts
git commit -m "feat(momo): walletApi.revealCardDetails — fetch card PAN/CVV (never stored)"
```

## Task B3: agent→前端 取卡触发(复用 user-question-request)

**Files:**
- Create: `apps/momo/src/lib/agent/paymentBridge.ts`
- Modify: `apps/momo/src/lib/agent/agentChat.svelte.ts`(user-question 分发处)

- [ ] **Step 1: 约定 payment 请求的识别方式**

agent 通过 `user-question-request` 发起取卡。约定:`questions[0]` 带 `{ kind: "payment_card", cardId?, sink, merchant, amount }`。前端据 `kind === "payment_card"` 路由给 paymentBridge,而非常规问答 UI。

- [ ] **Step 2: paymentBridge 实现**

```ts
// apps/momo/src/lib/agent/paymentBridge.ts
import { walletApi } from "../walletApi";
import { resolveUserQuestion } from "./daemonChat";
import { injectToSink } from "./paymentSink"; // Task B5

export interface PaymentCardRequest {
  kind: "payment_card";
  cardId: number;
  sink: string;          // 见 Task B5
  merchant?: string;
  amount?: number;
}

/** 取卡 → 注入 sink → 用完即扔 → resolve 非敏感结果回 agent。 */
export async function handlePaymentCardRequest(
  turnId: string,
  requestId: string,
  req: PaymentCardRequest
): Promise<void> {
  let card: { pan: string; cvv: string; expMonth: string; expYear: string } | null = null;
  try {
    const d = await walletApi.revealCardDetails(req.cardId);
    card = { pan: d.pan, cvv: d.cvv, expMonth: d.expMonth, expYear: d.expYear };
    await injectToSink(req.sink, d);          // sink 消费敏感数据,见 B5
    // 回 agent 的只有非敏感结果:末四位 + 状态。
    await resolveUserQuestion(turnId, requestId, {
      result: "injected",
      last4: d.pan.slice(-4),
    });
  } catch (e) {
    await resolveUserQuestion(turnId, requestId, { result: "error" });
  } finally {
    // 用完即扔:覆盖引用(JS 无法真正 zeroize,但确保不再可达)。
    card = null;
    void card;
  }
}
```

- [ ] **Step 3: 在 agentChat 分发**

`agentChat.svelte.ts` 处理 `user-question-request` 的分支里,先判断:

```ts
const q = (event.questions?.[0] ?? {}) as { kind?: string };
if (q.kind === "payment_card") {
  await handlePaymentCardRequest(event.turnId, event.requestId, q as PaymentCardRequest);
  return; // 不进常规问答 UI
}
```

- [ ] **Step 4: 验证**

见 Task B6 端到端。

- [ ] **Step 5: 提交**

```bash
git add apps/momo/src/lib/agent/paymentBridge.ts apps/momo/src/lib/agent/agentChat.svelte.ts
git commit -m "feat(momo): payment-card bridge — reveal+inject card off-agent via user-question"
```

## Task B4: "用完即扔" + 不泄露的硬约束

**Files:**
- Modify: `apps/momo/src/lib/agent/paymentBridge.ts`(审查)
- Modify: `apps/momo/src/lib/agent/normalize.ts` 或 transcript 持久化处(redaction)

- [ ] **Step 1: 审查清单(写进 paymentBridge 顶部注释)**

```
// 卡敏感数据(PAN/CVV)硬约束:
// 1. 只在 handlePaymentCardRequest 的局部变量内存活。
// 2. 绝不写 $state / store / localStorage / sessionStorage。
// 3. 绝不 console.log / 不进 telemetry。
// 4. 绝不作为 resolveUserQuestion 的 answer 回 agent(只回 last4)。
// 5. sink 注入后立即丢弃引用。
```

- [ ] **Step 2: transcript redaction 防御**

确认 user-question 的 `answers`(回 agent 的内容)不含卡数据(B3 已只回 last4)。若 momo 本地有 transcript 落盘,加一道正则兜底:`\b\d{13,19}\b`(PAN)在落盘前打码。

- [ ] **Step 3: grep 自检**

```bash
grep -rn 'pan\|cvv' apps/momo/src/lib/agent | grep -iv 'expand\|company'
# 人工确认:无任何 store/log/persist 路径碰到 pan/cvv
```

- [ ] **Step 4: 提交**

```bash
git add -A apps/momo/src/lib/agent
git commit -m "chore(momo): enforce card data is local-only, never logged/stored/returned to agent"
```

## Task B5: 定义并实现执行通道 sink(D-SINK 决策)

**Files:**
- Create: `apps/momo/src/lib/agent/paymentSink.ts`

- [ ] **Step 1: 决策 sink(需你拍板首个场景)**

```
MVP 首个 sink 选一:
  (s1) book-by-phone 电话担保:把卡 POST 给 WorldRouter 语音服务的担保端点(关联 booking task_id)。
  (s2) agent 指定的本地填表通道。
  (s3) 其它(你指定)。
推荐先做 (s1)——与 Part A 的预订场景闭环。
```

- [ ] **Step 2: 实现 injectToSink(以 s1 为例)**

```ts
// apps/momo/src/lib/agent/paymentSink.ts
import type { SensitiveCardDetails } from "../walletTypes";

export async function injectToSink(
  sink: string,
  card: SensitiveCardDetails
): Promise<void> {
  if (sink.startsWith("wr-booking:")) {
    const taskId = sink.slice("wr-booking:".length);
    // 把卡提交给 WorldRouter 担保端点(全栈自有);卡数据 client→WR,不经 agent。
    // 端点契约由 WorldRouter 服务端侧定义(全栈自有)。
    await submitCardToBooking(taskId, card); // 实现:fetch VITE_WORLDROUTER_CONTROL_URL + sessionToken
    return;
  }
  throw new Error(`unknown sink: ${sink}`);
}
```

- [ ] **Step 3: 验证 + 提交**(端到端见 B6)

```bash
git add apps/momo/src/lib/agent/paymentSink.ts
git commit -m "feat(momo): payment sink — submit revealed card to booking guarantee (s1)"
```

## Task B6: 端到端验证

- [ ] **Step 1:** 重启 momo(确保用新 puffer 二进制 + 已登录写好 creds)。
- [ ] **Step 2:** 触发一个需要担保的预订 → agent 走 Part A 完成 book → 需支付时发 `payment_card` user-question → 前端取卡 → 注入 sink。
- [ ] **Step 3:** 断言:
  - DevTools / Svelte store 里**搜不到** PAN/CVV;
  - daemon transcript / session detail 里只见 `last4`,无完整卡号;
  - sink 收到卡、担保成功。

---

## Self-Review(写计划者自检结论)

- **Spec 覆盖**:Part A 覆盖"key 传递"(A1 写盘 / A2 登录写入 / A3 skill 读取);Part B 覆盖"取卡号+CVV 特殊处理"(B1 端点 / B2 取卡 / B3 触发 / B4 不泄露 / B5 sink / B6 验证)。✓
- **跨 repo 标注**:B1(ucard-backend)、B5 的 WorldRouter 担保端点为契约级,已明确标注"全栈自有、按契约实现",非 TBD。
- **类型一致**:`SensitiveCardDetails`(B1/B2/B3/B5)、`PaymentCardRequest`(B3)、`injectToSink`(B3/B5)签名一致。
- **关键风险**:B1-Step3 指出——若 Strada **不允许程序化 reveal CVV**,模式 B 前端注入前提不成立,需回退模式 A(后端代付)。**这是 Part B 能否成立的硬前提,建议第一步先验证 B1。**

## 建议实施顺序

1. **先做 B1**(验证 reveal CVV 是否可行)——它能一票否决整个 Part B 的前端方案。
2. Part A(A1→A2→A3)独立可交付,可并行。
3. B1 通过后再做 B2→B6。
