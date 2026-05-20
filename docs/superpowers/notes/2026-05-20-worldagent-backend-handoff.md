# worldagent 端到端联调 — 待解决问题

Date: 2026-05-20
Author: sean (with Claude)
Status: 阻塞中，待 Auth + Infer 后端处理

---

## 背景

puffer 桌面端新增 `worldagent` provider，OAuth 登录流程：

```
puffer-cli auth login worldagent
  → 浏览器打开 https://auth.worldrouter.ai/login?redirect_uri=http://127.0.0.1:1456/callback&client_state=...
  → 用户登录（Auth Station + WorkOS AuthKit）
  → Auth Station 302 回 http://127.0.0.1:1456/callback?token=<JWT>&refresh_token=...&state=...
  → puffer 监听器抓回调
  → puffer 用 token JWT 调 control-api 创建 WR api_key
  → 把 api_key 落本地 AuthStore，worldagent 就能跑 inference
```

代码已经实现并 push 到 `feat/worldagent-provider` 分支（commit `1132c94` 及之前），单测 7/7 通过，workspace 编译干净。下面是**联调时发现的两个上游问题**，puffer 这边已经做不了，需要后端改。

---

## 问题 1：Production Auth Station 没读到 `ALLOWED_REDIRECT_ORIGINS` 的最新值

### 现象

浏览器打开
`https://auth.worldrouter.ai/login?redirect_uri=http://127.0.0.1:1456/callback&client_state=X`，
用户登录成功后，Auth Station **没有** 302 回 `127.0.0.1:1456`，而是落到 `https://auth.worldrouter.ai/`（404）。这是 auth-docs §troubleshooting 里写的"**redirect_uri 不在白名单 → 静默忽略 → 落到首页**"。

### 验证（已做）

- `vercel env pull --environment=production` 拉出来 `ALLOWED_REDIRECT_ORIGINS` 的当前值，**包含** `http://127.0.0.1:1456`（格式正确，逗号分隔，与现有 `https://worldrouter.ai` 等条目同行）。
- `auth/src/lib/sanitize.ts::validateRedirectUri` 的逻辑读 env、`split(',').map(trim)`、`new URL(uri).origin` exact match，完全正确。
- 关键探活（用 `wangshun+3@tomo.inc` 登录后浏览器拿到的 `__auth_session` cookie）：

  ```bash
  COOKIE='<__auth_session value>'
  
  # 探针 A：我们想要的 redirect_uri
  curl -i -b "__auth_session=$COOKIE" \
    'https://auth.worldrouter.ai/session/check?redirect_uri=http%3A%2F%2F127.0.0.1%3A1456%2Fcallback&client_state=probeA'
  # 结果：HTTP/2 200，没有 Location → 静默拒绝 ❌
  
  # 探针 B：白名单里已有的 worldrouter.ai
  curl -i -b "__auth_session=$COOKIE" \
    'https://auth.worldrouter.ai/session/check?redirect_uri=https%3A%2F%2Fworldrouter.ai%2Ftest&client_state=probeB'
  # 结果：HTTP/2 302
  # location: https://worldrouter.ai/test?state=probeB&token=<Auth Station JWT>&refresh_token=<...>  ✅
  
  # 探针 C：明显非法的 URL
  curl -i -b "__auth_session=$COOKIE" \
    'https://auth.worldrouter.ai/session/check?redirect_uri=https%3A%2F%2Fnonsense.example.com%2Ftest&client_state=probeC'
  # 结果：HTTP/2 200，没有 Location → 静默拒绝 ❌
  ```

  → 证明 cookie / silent SSO / 白名单校验逻辑全都正常。**只是当前运行的 deployment 不认识 `http://127.0.0.1:1456`。**

### 推断（修正版 2026-05-20）

**不是** "Vercel redeploy 复用旧 env snapshot"，**不是** "Vercel 过滤 loopback IP literal"。

真实根因：Vercel UI 上对 env entry 做 **edit + append + save** 时，CLI / `vercel env pull` 能立即读到新值（UI 状态层），但 **deployment build 注入用的是上一个真正持久化版本**（存储层）。前者快、后者慢，两层不同步。

### 修复（已生效）

在 Vercel UI Settings → Environment Variables 找到 `ALLOWED_REDIRECT_ORIGINS` Production → **⋯ Remove** 整条 → 重新 **Add** 同名 entry 写入完整新 value（一次性写全所有条目）→ **Save** → Redeploy 一次。

**仅 edit + 末尾 append + save 不可靠** — `vercel env pull` 能立刻看到新值，但 deployment 注入有同步延迟。

### 当前 Production 状态（2026-05-20 已验）

- Latest deployment: `dpl_CSjGzTPKpUn6aSye3QetZnqLfe9q` (`auth-worldrouter-gsih04aqk-nubit.vercel.app`)
- `auth.worldrouter.ai` alias 已切到此 deployment
- `ALLOWED_REDIRECT_ORIGINS` 包含 13 项，其中：
  - `http://127.0.0.1:1456` ✓ ACCEPTED
  - `http://localhost:1456` ✓ ACCEPTED（兜底）
  - 其他 11 项 ✓

### 验证（无需 cookie）

```bash
curl -sS -o /dev/null -w "%{http_code}\n" \
  'https://auth.worldrouter.ai/session/check?redirect_uri=http%3A%2F%2F127.0.0.1%3A1456%2Fcallback&client_state=p'
# 期望: 307（实测通过）
```

---

## 问题 2：Auth Station JWT 不含 `default_team_id`，但 control-api 路径需要 `team_id`

### 现象

后端给的 control-api 调用方式：

```
POST https://control-api-pre-7f819c.worldrouter.ai/platform/v1/teams/{team_id}/keys
Authorization: Bearer <JWT>
Body:  {"key_alias": "puffer-<uuid>"}
```

`{team_id}` 必须从 JWT 的 `default_team_id` claim 取。

但**实际从 Auth Station 拿到的 JWT 不带这个字段**。

### 证据

探针 B 拿到的 token JWT decode：

```json
{
  "sub": "user_01KRNMV53Y0WTZBPXA2RWNMC06",
  "email": "wangshun+3@tomo.inc",
  "name": "shun wang",
  "picture": null,
  "iss": "https://auth.worldrouter.ai",
  "aud": "worldclaw",
  "iat": 1779275875,
  "exp": 1779362275
}
```

对比之前后端给的 control-api curl 示例里那个 JWT（能跑通的那个）：

```json
{
  "iss": "infer-session",      ← 不一样
  "sub": "cf530f43-...",
  "user_id": "cf530f43-...",
  "email": "winterfell0614+7@gmail.com",
  "default_team_id": "6afdef35-...",   ← 关键字段
  "user_role": "internal_user",
  "idp": "workos",
  "idp_sub": "user_01KRZJFGVE90DCGPP1E16XBJXX",
  "jti": "...",
  "iat": 1779266768,
  "exp": 1779353167
}
```

→ 那个能跑通 control-api 的 JWT 不是 Auth Station 签的，而是 **infer-monorepo 内部签发的 session JWT**（HS256，`iss=infer-session`，多了 `default_team_id` / `user_role` / `user_id` 等业务字段）。

### 含义

桌面端只能拿到 Auth Station JWT（OIDC 标准、面向所有接入方）。**桌面端没法直接拿到 infer-session JWT**——那是 worldagent dashboard / infer BFF 自己签的，浏览器侧只有 cookie，没有原始 JWT。

所以 puffer 现在的实现里 `exchange_jwt_for_api_key()` 第一行 `decode_jwt_profile(jwt).default_team_id.ok_or_else(...)` 会立刻失败，整个流程在那一步炸。

### 修复方案（让后端拍板）

| 方案 | 改动方 | 备注 |
|---|---|---|
| **A. control-api 直接接受 Auth Station JWT**（推荐） | infer 后端 | 控制台拿 Authorization header → 用 `https://auth.worldrouter.ai/jwks` 验签（RS256, `aud=worldclaw`, `iss=https://auth.worldrouter.ai`）→ 从 JWT 的 `sub`（即 WorkOS user_id）反查 infer 数据库找用户默认 team。puffer 端代码 0 改动，URL 还是 `POST /platform/v1/teams/{default_team_id}/keys`，但 `{default_team_id}` 由后端从 user 反查得出。或者干脆改成 `POST /platform/v1/keys`（不要 team 路径段），后端自己挑用户默认 team。 |
| **B. 新增 「Auth Station JWT → Infer Session JWT」交换端点** | infer 后端 | puffer 多走一步 `POST /v1/auth/exchange { authStationToken }` → 拿到 infer-session JWT → 再 `POST /platform/v1/teams/{team_id}/keys`。puffer 代码改一行 `exchange_jwt_for_api_key` 内部多调一次 fetch。这跟 auth-docs §guides/backend.md 的"两层 JWT"模式吻合。 |
| C. body 里塞 team_id | 任一方 | 让前端 / puffer 自己选 team。但桌面端没法选——它只有一个登录态、不知道用户多 team 怎么处理。除非接口 idempotent "默认 team" 行为。Not great。 |

**推荐方案 A**，因为 puffer 是桌面端、没 BFF，多一跳 latency 没必要；并且 Auth Station JWT 已经是 RS256 + JWKS 签发的标准 OIDC token，外部服务做 JWT 验签是常态。

### 验证修复

修完后 puffer 端 e2e 跑通的判据：

```bash
cd /Users/shun/Data/Code/tomo/agentenv/puffer
PUFFER_WORLDAGENT_AUTH_URL=https://auth.worldrouter.ai \
PUFFER_WORLDAGENT_CONTROL_URL=https://control-api-pre-7f819c.worldrouter.ai \
  cargo run -p puffer-cli -- auth login worldagent
```

浏览器打开 puffer 打印的 URL → 用一个 Production 已有的账号登录 → puffer 终端打印：

```
stored oauth credentials for worldagent       # 或类似 "stored api key for worldagent"
```

然后立刻验证 api_key 真能用：

```bash
cat ~/Library/Application\ Support/com.tomo.puffer/auth.json \
  | jq '.providers.worldagent'

# 期望看到 { "kind": "api_key", "key": "sk-worldrouter-..." }

curl -sS https://inference-api.worldrouter.ai/v1/models \
  -H "Authorization: Bearer <sk-worldrouter-...>"

# 期望返回模型列表 JSON，HTTP 200
```

---

## 附录 A：相关文件 / commit / endpoint 速查

| 项目 | 位置 |
|---|---|
| puffer 实现 | `feat/worldagent-provider` 分支，head `1132c94`（spec → impl → cleanup） |
| `exchange_jwt_for_api_key` 实现 | `crates/puffer-provider-worldagent/src/auth.rs:279`（带 TODO 说明 API 会调整） |
| 设计文档 | `docs/superpowers/specs/2026-05-20-worldagent-provider-design.md` |
| 实现 plan | `docs/superpowers/plans/2026-05-20-worldagent-provider.md` |
| Auth Station 源码 | `/Users/shun/Data/Code/tomo/worldclaw/infer-monorepo/auth/` |
| 白名单校验逻辑 | `auth/src/lib/sanitize.ts::validateRedirectUri` |
| 白名单 env 读取 | `auth/src/lib/config.ts::allowedRedirectOrigins` |
| Auth Station Sandbox | `https://auth-worldrouter.vercel.app`（puffer 默认） |
| Auth Station Production | `https://auth.worldrouter.ai`（联调走这个） |
| Auth Station JWKS | `https://auth.worldrouter.ai/jwks` — 用于验 puffer 拿到的 token |
| Inference API（OpenAI 兼容） | `https://inference-api.worldrouter.ai/v1/...` |
| Control API（创建 key 用） | `https://control-api-pre-7f819c.worldrouter.ai`（预览，会变） |
| Vercel project | `nubit/auth-worldrouter`，prj id `prj_4Mi7OqkeMQ5bOaNzzMOiLHsPRDGl` |
| puffer 端 env override | `PUFFER_WORLDAGENT_AUTH_URL`、`PUFFER_WORLDAGENT_CONTROL_URL` |

## 附录 B：puffer 端固定 callback

桌面端写死 loopback callback：`http://127.0.0.1:1456/callback`。

- 必须在 Auth Station `ALLOWED_REDIRECT_ORIGINS` 白名单里（origin 严格匹配，protocol+host+port，路径不参与）
- 既要在 Production 加（`auth.worldrouter.ai`），也要在 Sandbox 加（`auth-worldrouter.vercel.app`）
- 加 env 之后**必须 redeploy**，Vercel 不会自动 reload

## 附录 C：一个完整的 Auth Station JWT 真实样本（已脱敏？）

来自探针 B 响应，便于后端测试 JWKS 验签：

```
header:  {"alg":"RS256","kid":"prod-1","typ":"JWT"}
payload: {
  "sub": "user_01KRNMV53Y0WTZBPXA2RWNMC06",
  "email": "wangshun+3@tomo.inc",
  "name": "shun wang",
  "picture": null,
  "iss": "https://auth.worldrouter.ai",
  "aud": "worldclaw",
  "iat": 1779275875,
  "exp": 1779362275
}
signature: <RS256 sig, verifiable via /jwks kid=prod-1>
```

注：这个 token 是 wangshun+3@tomo.inc 在 2026-05-20 11:17 UTC 登录拿到的，有效期 24h（到 2026-05-21 11:17 UTC）。等过期之后会失效；这里只用于说明 JWT 的形状。
