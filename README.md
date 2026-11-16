# OneBot 11 原型服务端（OB12-ready）

一个用 **Rust（stable）+ PostgreSQL** 实现的自建聊天服务端原型。服务端原生适配 OneBot 协议，核心原则是：

> **不区分人类账号和机器人账号。服务端只认账号。**

每个注册账号都获得一个 token；凭 token 可以通过 HTTP 调用动作、通过 WebSocket 接收事件。服务端不关心背后是人、脚本还是机器人。

- 当前实现：**OneBot 11 正向 WebSocket 事件推送 + HTTP 动作**
- 架构预留：**OneBot 12 适配器接口与 `meta.connect` 首帧钩子**
- 存储层与扇出层：**不依赖 OB11/OB12 字段名**，协议差异集中在 `src/protocol/`
- 生产部署：Rust 只监听明文 HTTP/WS，由 Nginx 终止 TLS

> 这是原型，不是生产级 IM。详见 [限制与 TODO](#限制与-todo)。

---

## 目录

- [快速开始](#快速开始)
- [单文件 HTML 客户端](#单文件-html-客户端)
- [核心架构](#核心架构)
- [认证模型](#认证模型)
- [HTTP API 一览](#http-api-一览)
- [WebSocket 事件](#websocket-事件)
- [会话、存储与历史](#会话存储与历史)
- [协议抽象与 OB12 预留](#协议抽象与-ob12-预留)
- [Nginx 部署](#nginx-部署)
- [测试与验收](#测试与验收)
- [版本管理与发布](#版本管理与发布)
- [限制与 TODO](#限制与-todo)

更详细的文档：

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) —— 分层、存储、扇出、背压、OB12 迁移路径
- [docs/API.md](docs/API.md) —— 完整端点、动作参数、错误码、示例
- [docs/TESTING.md](docs/TESTING.md) —— 验收标准逐项映射与测试证据

---

## 快速开始

### 前置条件
- Rust stable（本项目在 1.99.1 上构建；`Cargo.toml` 使用 edition 2024）
- PostgreSQL（在 PostgreSQL 18 上验证）
- Python 3.10+（仅运行端到端测试脚本时需要 `websockets`）

### 1. 创建数据库

```bash
createdb -h 127.0.0.1 -U postgres onebot_dev
# 或：
psql -h 127.0.0.1 -U postgres -c 'CREATE DATABASE onebot_dev;'
```

本地开发数据库的连接串由 `DATABASE_URL` 注入，代码中没有任何硬编码数据库密码；请使用自己的 PostgreSQL 账号密码。

### 2. 配置环境变量

```bash
cp .env.example .env
# 编辑 .env，至少确认 DATABASE_URL、REGISTER_KEY、ADMIN_KEY
```

所有配置：

| 环境变量 | 必填 | 默认值 | 说明 |
|---|---:|---|---|
| `DATABASE_URL` | 是 | — | PostgreSQL 连接串 |
| `LISTEN_ADDR` | 否 | `127.0.0.1:8080` | Rust 明文 HTTP/WS 监听地址 |
| `REGISTER_KEY` | 否 | 未设置 | **未设置时注册端点关闭**；设置后请求必须匹配 |
| `ADMIN_KEY` | 否 | 未设置 | 服管重置 token 的 `X-Admin-Key`；未设置时管理员重置关闭（自助重置仍可用） |
| `WS_QUEUE_CAPACITY` | 否 | `256` | 每个 WS 连接的有界发送队列长度 |
| `RATE_LIMIT_DEFAULT` | 否 | `20` | 新账号默认动作限流（次/秒，突发=同值），落库到 `accounts.rate_limit` |
| `PUBLIC_BASE_URL` | 否 | 由请求推导 | 注册响应中的对外 URL，例如 `https://chat.example.com` |
| `SESSION_CACHE_TTL_SECS` | 否 | `30` | 群成员/私聊账号缓存 TTL |
| `WS_REVALIDATE_SECS` | 否 | `60` | WS token 周期复核间隔 |
| `HISTORY_DEFAULT_LIMIT` | 否 | `20` | 历史查询默认条数 |
| `HISTORY_MAX_LIMIT` | 否 | `100` | 历史查询最大条数 |

### 3. 启动

```bash
cargo run --release
# 或开发期：
RUST_LOG=onebot_server=debug cargo run
```

启动时自动运行 `migrations/`。服务只监听明文 HTTP/WS，不配置 TLS。

```bash
curl http://127.0.0.1:8080/health
# {"ok":true,"database":"up","active_connections":0,"slow_disconnects":0}
```

浏览器打开 **`http://127.0.0.1:8080/client`** 即可使用单文件聊天客户端；下面是 API 手工调用方式。

### 4. 注册账号

```bash
curl -sS http://127.0.0.1:8080/register \
  -H 'Content-Type: application/json' \
  -d '{"username":"alice","labels":["human"],"register_key":"replace-with-a-strong-register-key"}'
```

响应（节选）：

```json
{
  "account_id": 1,
  "username": "alice",
  "token": "……仅此一次返回……",
  "token_fingerprint": "4d7c31806bf4",
  "http_base_url": "http://127.0.0.1:8080",
  "ws_url": "ws://127.0.0.1:8080/ws",
  "ws_url_with_token": "ws://127.0.0.1:8080/ws?access_token=……",
  "capabilities": {
    "protocol": "OneBot",
    "version": "11",
    "message_format": "array",
    "actions": ["send_private_msg", "send_group_msg", "…"],
    "rate_limit_per_second": 20
  },
  "notice": "The token is the account. It is shown once and cannot be recovered; a server operator can reset it. Keep it secret."
}
```

重复注册同一 username（大小写不敏感）返回 **409**。

### 5. 连接 WebSocket 并调用动作

WS 握手认证**只使用 query 参数**：`ws://host/ws?access_token=<token>`。

```bash
# HTTP 动作（Authorization: Bearer）
curl -sS http://127.0.0.1:8080/ \
  -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"action":"send_private_msg","params":{"user_id":2,"message":[{"type":"text","data":{"text":"hi"}}]}}'
```

```json
{"status":"ok","retcode":0,"data":{"message_id":1,"message_seq":1}}
```

动作也可以走 `/api/{action}` 或 `/{action}`，参数直接放 body：

```bash
curl -sS http://127.0.0.1:8080/api/get_login_info \
  -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' -d '{}'
```

---

## 单文件 HTML 客户端

仓库根目录的 [`client.html`](client.html) 是一个**零依赖、单文件**的浏览器客户端。Rust 服务通过 `GET /client` 直接提供它：

```
http://127.0.0.1:8080/client
```

也可以直接打开本地文件（服务端为原型开启了 CORS）：

```
file:///.../onebot-server/client.html
```

客户端能力：

- 注册新账号 / 使用已有 token 登录；token 保存在本机 `localStorage`
- 连接正向 WS，实时接收私聊、群聊、notice 与 request 事件
- 会话列表、私聊/群聊历史、发送文本消息、消息去重、未读数
- 创建群、申请加群、退群；加好友、待处理申请中心（好友/入群审批）
- 设置面板：账号信息、WS 状态、token 指纹、复制 token、重置 token
- 移动端响应式布局（会话列表 ↔ 聊天页切换）

认证方式与 API 完全一致：HTTP 使用 `Authorization: Bearer`，WS 使用 `?access_token=`。客户端不包含任何第三方 CDN、字体或框架资源。

> 为了让 `file://` 直接打开可用，服务端是**原型级 permissive CORS**（只放行 API，仍需 token，不使用 cookie）。生产部署建议在 Nginx 层限制 CORS 来源，或仅允许同源 `/client`。

客户端会通过 `GET /requests` 拉取当前可处理的申请，因此刷新页面后仍能看到待审批事项。私聊事件额外带 `peer_id` 扩展，便于客户端在发送方回显时定位对端会话。

---

## 核心架构

```
                         ┌──────────────────────────────────────────┐
 HTTP POST /action ─────▶│ HTTP 路由层 (http.rs)                    │
 Authorization: Bearer   │  ├─ 统一 token 中间件 (auth.rs)          │
                         │  └─ OB11 适配器解析动作 (protocol/ob11)  │
                         └───────────────┬──────────────────────────┘
                                         ▼
                         ┌──────────────────────────────────────────┐
                         │ 内部动作处理层 (actions.rs)              │
                         │  ├─ 发送消息                              │
                         │  ├─ 查询信息                              │
                         │  └─ message_seq 历史游标                  │
                         └───────┬───────────────────┬──────────────┘
                                 │                   │
                                 ▼                   ▼
                    ┌────────────────────┐   ┌──────────────────────┐
                    │ 存储层 (store.rs)  │   │ 扇出层 (fanout.rs)   │
                    │ PostgreSQL         │   │ 只操作 ChatEvent     │
                    │ 协议中立           │   │ + 内存连接表          │
                    └────────────────────┘   └──────────┬───────────┘
                                                        ▼
                                             ┌─────────────────────────┐
                                             │ 协议适配器 (ProtocolAdapter) │
                                             │ OB11 实现 / OB12 TODO   │
                                             └────────────┬────────────┘
                                                          ▼
                                             ┌─────────────────────────┐
                                             │ WS 连接 (ws.rs)         │
                                             │ 每连接有界队列 + 超时   │
                                             └─────────────────────────┘
```

### 模块分层

| 层 | 位置 | 职责 |
|---|---|---|
| HTTP 路由 | `src/http.rs` | 注册、动作入口、自定义关系端点、token 重置、`/client`、`/requests` |
| 认证 | `src/auth.rs` | token 生成/哈希、HTTP+WS 共用验证、周期复核 |
| 协议适配 | `src/protocol/` | OB11 实现、OB12 占位、`ProtocolAdapter` trait |
| 消息段 | `src/protocol/segments.rs` | OB11 消息段数组编解码；CQ 字符串 TODO |
| 内部动作 | `src/actions.rs` | 动作分发、权限、历史、发送 |
| 关系建立 | `src/relations.rs` | 好友/群申请与审批业务、待处理申请汇总 |
| 扇出 | `src/fanout.rs` | 群/私聊/notice/request 推送，per-account 填充 |
| WS 连接 | `src/ws.rs`, `src/registry.rs` | 握手、连接生命周期、背压、踢连接、复核 |
| 浏览器客户端 | `client.html` | 单文件零依赖聊天客户端，由 `/client` 提供 |
| 缓存 | `src/cache.rs` | 群成员、私聊账号缓存 + 失效 + TTL |
| 存储 | `src/store.rs` | PostgreSQL 查询；无协议字段 |
| 模型 | `src/model/` | `ChatEvent`、`ActionRequest`、`MessageSegment` 等中性结构 |

### 核心内部模型

`ChatEvent`（`src/model/event.rs`）不包含 `post_type`、`self_id`：

```rust
pub struct ChatEvent {
    pub event_id: String,
    pub event_type: EventType,          // Meta | Message | Notice | Request
    pub detail_type: String,            // private/group/friend/group_increase...
    pub account_id: String,             // 接收者账号
    pub session_type: SessionType,      // Group | Private | None
    pub session_id: Option<String>,
    pub sender_id: Option<String>,
    pub content: Vec<MessageSegment>,
    pub timestamp: f64,
    pub raw: Option<serde_json::Value>,
    pub message_id: Option<i64>,        // 原型扩展
    pub message_seq: Option<i64>,       // 原型扩展，用于历史游标
    pub extras: EventExtras,
}
```

协议适配 trait：

```rust
pub trait ProtocolAdapter: Send + Sync {
    fn deserialize_event(&self, raw: &str) -> Result<ChatEvent, AdapterError>;
    fn serialize_event(&self, event: &ChatEvent, self_account: &Account) -> Result<String, AdapterError>;
    fn deserialize_action(&self, raw: &str) -> Result<ActionRequest, AdapterError>;
    fn serialize_action_response(&self, resp: &ActionResult) -> Result<String, AdapterError>;
    fn action_mapping(&self) -> ActionMapping;

    // OB12 的 meta.connect 首帧钩子；OB11 默认返回空
    fn initial_events(&self, self_account: &Account) -> Result<Vec<String>, AdapterError> {
        Ok(Vec::new())
    }
}
```

- **OB11 适配器**：`src/protocol/ob11.rs`，原型唯一实现。
- **OB12 适配器**：`src/protocol/ob12.rs`，标记 TODO；WS 建立后先取 `initial_events`，未来可直接返回 `meta.connect` 首帧。
- **扇出层**只构造/克隆 `ChatEvent`，再调用适配器序列化；不会拼 OB11 JSON。
- **存储层**只存 `session_type`、`session_id`、`sender_id`、`message_seq`、`content` 等中性字段，不引用协议版本字段。

更详细说明见 [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)。

---

## 认证模型

- **token 是唯一凭证**：没有密码、没有登录端点、没有找回流程。
- SQL 只存 **token 的 SHA-256 十六进制哈希**；注册响应中的 token 只出现一次。`token_hash` 有 UNIQUE 约束。
- token 生成：32 字节 CSPRNG（`getrandom`）→ base64url（无 padding）→ 43 字符。
- **HTTP**：`Authorization: Bearer <token>`，每个受保护端点都经过统一中间件。
- **WS**：`GET /ws?access_token=<token>`；这是本原型选择的唯一 WS 认证方式。
- 注册保护：设置 `REGISTER_KEY` 后必须提交匹配的 `register_key`；**未设置 `REGISTER_KEY` 时注册端点关闭**（默认关闭开放注册）。
- username 统一去空白并转小写后存储，数据库另有 `username = lower(username)` 约束。
- `labels: text[]` 仅作为自由标签，服务端逻辑不依赖。
- 没有 `account_type` 字段，也没有人/机器人权限分支。

### token 重置

两种方式，均会写入 `token_reset_audit` 审计表：

1. **自助重置**（需要当前 token）
   ```http
   POST /me/token/reset
   Authorization: Bearer <current token>
   ```
2. **服管重置**（需要 `ADMIN_KEY`）
   ```http
   POST /admin/accounts/{account_id}/token/reset
   X-Admin-Key: <ADMIN_KEY>
   X-Admin-Actor: operator-name
   ```

效果：

- 旧 token 立即失效；下一个 HTTP 请求返回 **401**（OB11 风格错误体）。
- 立即踢掉该账号所有活跃 WS（内存账号→连接映射主动取消）。
- WS 连接每 60 秒还会重新校验 token 作为兜底。
- 审计记录谁（self 用户名 / `admin:<actor>`）、何时（`created_at`）、哪个账号。
- 新 token 只在响应中返回一次。

---

## HTTP API 一览

完整请求/响应见 [docs/API.md](docs/API.md)。

### 公共端点

| 方法/路径 | 说明 |
|---|---|
| `GET /health` | 健康检查（含数据库、活跃 WS 数、慢消费者计数） |
| `GET /client` | 单文件 HTML 聊天客户端 |
| `POST /register` | 注册并发 token；已禁用或无 key 返回 403；重复 409 |
| `GET /ws?access_token=...` | WebSocket 升级；只推事件 |
| `POST /admin/accounts/{account_id}/token/reset` | 服管重置（`X-Admin-Key`） |

### 需 Bearer token 的 OB11 动作入口

| 方式 | 说明 |
|---|---|
| `POST /` | 标准 OB11 body：`{"action":"...","params":{...}}` |
| `POST /api` | 同上别名 |
| `POST /action` | 同上别名 |
| `POST /api/{action}` | action 放路径，params 直接放 body |
| `POST /{action}` | 同上，方便兼容 |

响应严格为三字段：

```json
{"status":"ok","retcode":0,"data":{...}}
{"status":"failed","retcode":1404,"data":null}
```

未实现动作返回 HTTP **200** + `status:"failed"` + `retcode:1404`，不会 500。

### 自定义关系端点（需 Bearer token）

| 方法/路径 | body | 说明 |
|---|---|---|
| `POST /friends/request` | `{"target_id":2}` | 发起好友申请，推 friend request 事件 |
| `POST /friends/approve` | `{"request_id":1}` 或 `{"flag":"1"}` + `"approve":true` | 审批好友申请 |
| `POST /groups` | `{"name":"群名"}` | 建群，创建者自动 owner |
| `POST /groups/{group_id}/join` | 空 body `{}` | 申请入群，推给 owner/admin |
| `POST /groups/{group_id}/members/approve` | `{"request_id":1,"approve":true}` | 审批入群 |
| `POST /groups/{group_id}/leave` | 空 body | 退群；owner 退群返回 403 |
| `POST /me/token/reset` | 空 body | 自助重置 token |
| `GET /requests` | — | 当前账号可处理的好友申请与入群申请 |

关系审批**不走 OB11 动作**（不实现 `set_friend_add_request` / `set_group_add_request`）。

---

## WebSocket 事件

- WS 只推事件，不接受动作调用；客户端发来的文本会被忽略。
- 每个连接有独立的有界发送队列（`WS_QUEUE_CAPACITY`，默认 256）。
- 队列满时标记慢消费者、取消该连接；**扇出循环永不等待网络发送**。
- 同一账号允许多个 WS 连接。
- 每个接收者独立克隆事件、填 `account_id`（OB11 输出为 `self_id`）并独立序列化；不会复用同一份 JSON 群发。

示例：群消息事件（接收者视角）

```json
{
  "post_type": "message",
  "message_type": "group",
  "sub_type": "normal",
  "time": 1763300000,
  "self_id": 2,
  "message_id": 101,
  "message_seq": 7,
  "group_id": 1,
  "user_id": 1,
  "message": [{"type": "text", "data": {"text": "hello"}}],
  "raw_message": "hello",
  "font": 0,
  "sender": {"user_id": 1, "nickname": "alice", "card": "", "role": "owner"}
}
```

- `self_id` 是该接收者自己的账号 ID。
- 私聊给会话双方都推，包括发送方回显；客户端用 `message_id` 去重。
- 群成员变动推 `notice`（`group_increase` / `group_decrease`）；退群者不会收到自己的 `group_decrease`。
- 好友申请推 `request`（`request_type:"friend"`）；入群申请推 `request`（`request_type:"group"`, `sub_type:"add"`）给所有 owner/admin。
- `message_seq` 是为历史游标增加的 OB11 扩展字段；客户端可以忽略，也可用于补拉。
- 私聊消息事件额外带 `peer_id` 扩展（对端账号）。OB11 原始 `user_id` 在发送方回显时是发送者自身，浏览器客户端需要 `peer_id` 才能把回显路由到正确会话；这是当前原型的兼容扩展。

---

## 会话、存储与历史

### 私聊会话标识

采用 **有序对** 方案，文档明确：

```
private session_id = "<min(account_a, account_b)>:<max(account_a, account_b)>"
```

例如账号 1 和 2 → `"1:2"`。数据库 `messages.session_id` 使用该规范字符串。事件层 `ChatEvent.session_id` 对私聊填“接收者的对端账号 ID”（更符合中性事件语义）；OB11 序列化不暴露它。这样未来 OB12 的事件转换也能直接使用。

### 群聊

`session_type = "group"`，`session_id = group_id`。群成员表角色为 `owner/admin/member`；创建者自动成为 owner；owner 原型阶段禁止退群。

### message_seq 分配

- `messages.id` 是全局 `BIGSERIAL` message_id，仅用于唯一/去重，不要求有序。
- `messages.message_seq` 是**会话内**单调递增序号，由 `message_sequences` 表原子 UPSERT 分配：
  ```sql
  INSERT INTO message_sequences (...) VALUES (..., 1)
  ON CONFLICT (...) DO UPDATE SET next_seq = message_sequences.next_seq + 1
  RETURNING next_seq
  ```
  行锁保证同一会话并发发送串行分配；不使用时间戳。
- 唯一索引：`UNIQUE(session_type, session_id, message_seq)`。
- 插入失败可能造成序号空洞，但不会重复；分页按序号处理即可。

### 历史动作

`get_group_msg_history` / `get_friend_msg_history`：

- 请求参数只支持 `before_seq` / `after_seq` / `limit`，**不支持 offset / message_id 分页**。
- 返回 `data.messages`，按 `message_seq` 升序。
- 权限：群历史要求请求者是该群成员；私聊历史请求者必须是该有序对的一方（由会话 key 本身保证）。
- 返回对象结构与 WS 消息事件一致，`self_id` 为请求者账号。
- 消息永久保留；**保留窗口待定**。原型没有删除/清理策略。
- 游标示例：
  - 首次：`{"group_id":1,"limit":20}`（取最近 20 条，升序返回）
  - 补拉：`{"group_id":1,"after_seq":42,"limit":100}`
  - 向前翻：`{"group_id":1,"before_seq":43,"limit":20}`

### 数据表

| 表 | 关键字段/约束 |
|---|---|
| `accounts` | `id`, `username UNIQUE`（小写）, `token_hash UNIQUE`, `labels text[]`, `rate_limit`, `created_at`, `disabled` |
| `friends` | 有序对 `account_a < account_b`，PRIMARY KEY |
| `friend_requests` | pending/approved/rejected；同对仅一个 pending |
| `groups` | `id`, `name`, `owner_id`, `created_at` |
| `group_members` | `(group_id, account_id)` UNIQUE；role owner/admin/member；每群一个 owner |
| `group_join_requests` | 群+账号仅一个 pending |
| `messages` | `session_type`, `session_id`, `sender_id`, `message_seq`, JSONB `content`, `recalled` |
| `message_sequences` | 会话内下一位序号分配器 |
| `token_reset_audit` | 账号、actor、reason、时间 |

`connections` 只存在于内存，不在 PostgreSQL。

---

## 协议抽象与 OB12 预留

### OB11 → ChatEvent 映射（仅适配器内）

| OB11 | ChatEvent |
|---|---|
| `post_type` | `event_type` |
| `message_type` / `notice_type` / `request_type` | `detail_type` |
| `self_id` | 序列化时由接收者 `Account.id` 填充为 `account_id` |
| `group_id` / `user_id` | `session_id` / `sender_id` |
| `message` | `content: Vec<MessageSegment>` |
| `time` | `timestamp` |

其他细节（request flag、sub_type、sender 昵称/角色）放在 `EventExtras`，同样是协议中立字段。

### OB12 接入路径

1. 实现 `src/protocol/ob12.rs` 的 `ProtocolAdapter`。
2. 在 `initial_events` 返回 `meta.connect` 首帧；WS 层已在注册连接后、事件循环前发送它。
3. 在启动里把 `Arc<dyn ProtocolAdapter>` 换成 `Ob12Adapter`（或按请求做协议选择）。
4. **不需要修改** `store.rs`、`fanout.rs`、`registry.rs`、`cache.rs`、`actions.rs` 及数据库 schema。

OB12 的 `type/detail_type/sub_type` 扩展模型与当前 `event_type/detail_type/extras.request_sub_type` 对齐；`self.user_id` 对应 `account_id`。

---

## Nginx 部署

Rust 进程只监听明文 HTTP/WS。示例见 [nginx.conf.example](nginx.conf.example)：

- 终止 TLS（证书在 Nginx）
- WebSocket 必须透传 `Upgrade` / `Connection`
- 设置 `proxy_read_timeout`，避免空闲 WS 被切断
- 使用 `X-Forwarded-Proto` / `Host` 让 `/register` 推导正确 URL（或设置 `PUBLIC_BASE_URL`）
- 建议关闭对 WS 的代理缓冲
- 访问日志默认 `$request` 会包含 `?access_token=...`；示例里给了不记录 query 的日志格式

注册响应的 URL 推导规则：

1. 若设置 `PUBLIC_BASE_URL`，直接使用；
2. 否则用 `X-Forwarded-Proto`（http/https）+ `Host`；
3. 都没有时使用 `LISTEN_ADDR` 的本地地址。

---

## 测试与验收

已提供 `scripts/e2e.py`，覆盖 15 条验收标准中的全部功能链路，并额外覆盖退群、owner 退群禁止、send_msg 分发、慢消费者等。

```bash
python -m pip install -r scripts/requirements.txt
# 与服务端 .env 保持一致
export REGISTER_KEY='...'
export ADMIN_KEY='...'
python scripts/e2e.py
```

最近一次本机运行：**86 项端到端检查全部通过**；另有 **11 个 Rust 单元测试**通过，并用 Playwright/Chromium 验证了桌面、移动端、`file://` 直接打开、好友/入群审批、消息收发与 token 重置。详见 [docs/TESTING.md](docs/TESTING.md)。

---

## 版本管理与发布

本项目采用：

- **Semantic Versioning 2.0.0**：`MAJOR.MINOR.PATCH`，当前版本 `0.1.0`。
- **Conventional Commits**：`feat:` / `fix:` / `docs:` / `refactor:` / `chore:` 等，破坏性变更使用 `!` 与 `BREAKING CHANGE:` footer。
- 发布时必须更新 `CHANGELOG.md`、`Cargo.toml`，并创建 `vX.Y.Z` 附注 tag。
- 版本号唯一来源为 `Cargo.toml` 的 `package.version`。
- 发布流程与提交规范见 [CONTRIBUTING.md](CONTRIBUTING.md)，版本记录见 [CHANGELOG.md](CHANGELOG.md)。

---

## 限制与 TODO

### 明确不做（原型）

- **CQ 码字符串解析/编码**：只接受消息段数组；传 CQ 字符串会返回参数错误。TODO 标记在适配器注释中。
- **反向 WebSocket**：只做正向 WS（服务端推事件）。
- **OB12 适配器实现**：只留 trait 实现位置、TODO 和 `initial_events` 钩子。
- 动作清单之外的 OneBot 动作（群踢人/禁言/设管理、请求审批动作、文件/图片上传、频道等）。
- 完整生产级人类 UI、SDK（仓库提供的是原型单文件 HTML 客户端）。
- 分布式部署、多实例连接表共享。
- TLS（Nginx 负责）。
- 找回 token（只能重置）。
- 按账号类型的权限分支（不存在账号类型）。
- 消息清理/保留窗口策略（当前永久保留，保留窗口待定）。
- 好友删除、群解散、群主转让、管理员设置等管理功能。
- 通知重发、消息已读等扩展。
- HTTP 动作的 `echo` 回显（当前忽略未知字段；响应固定三字段）。

### 已知原型简化

- 限流为单进程内存 token bucket；多实例不共享。
- WS token 每 60 秒复核；重置会主动踢连接，复核只是兜底。
- 历史父级对象是完整消息事件（与 WS 事件一致），而非单独的 message 子结构。
- `raw_message` 由文本段拼接，非文本段用 `[type]` 占位；不是完整 CQ 编码。
- 群上限 200 只是能力字段中的展示值，未实际限制入群。
- CORS 为原型级 permissive，支持 `file://` 打开单文件客户端；生产应由 Nginx 收紧。
- 客户端只在打开会话时加载最近 50 条历史，未打开会话的“最后消息预览”可能为空；点击进入即可刷新。

---

## License

MIT（原型示例）。
