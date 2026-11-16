# 架构说明

## 1. 目标与非目标

### 目标

1. 一个可运行的 OneBot 11 原型：账号注册、token 认证、HTTP 动作、WS 事件、私聊/群聊闭环、历史补拉、好友/加群审批。
2. 内部模型与协议解耦，后续增加 OneBot 12 只需新增转换适配器与 WS 首帧握手逻辑。
3. 扇出永不阻塞；慢 WS 连接不得拖慢其他账号。
4. 所有消息/通知/申请按接收账号独立填充 `self`（内部字段 `account_id`），避免推错人。
5. HTTP 动作失败也返回 OB11 标准结构，未实现动作不是 500。
6. 提供一个零依赖单文件浏览器客户端，作为账号的普通 WebSocket/HTTP 连接。

### 非目标

- 生产级 IM 可靠性、跨实例一致性、消息删除/保留策略。
- 反向 WS、CQ 码解析、文件上传、群管理动作、生产级人类 UI、SDK。
- 服务端 TLS；TLS 由 Nginx 终止。

---

## 2. 数据流

### 2.1 HTTP 动作流

```
Client
  │ POST /  {"action": "...", "params": {...}}
  │ Authorization: Bearer <token>
  ▼
axum Router
  │
  ├─ auth middleware (src/http.rs)
  │    └─ authenticate_token(pool, token)  ← HTTP / WS 共用
  │         └─ SHA-256(token) → accounts.token_hash → Account
  │
  ├─ rate limiter (src/state.rs, per-account token bucket)
  │
  ├─ ProtocolAdapter::deserialize_action  (src/protocol/ob11.rs)
  │    └─ ActionRequest { action: InternalAction }
  │
  ├─ actions::execute (src/actions.rs)
  │    ├─ permissions / validation
  │    ├─ Store (PostgreSQL, neutral fields)
  │    └─ fanout (src/fanout.rs)
  │
  └─ ProtocolAdapter::serialize_action_response
       └─ {"status","retcode","data"} (HTTP 200)
```

### 2.2 事件扇出流

```
Domain event (message/notice/request)
  │
  ├─ determine audience:
  │    group message    → all group members (cache)
  │    private message  → both participants (cache)
  │    group notice     → current group members; leaver excluded
  │    friend request   → target account
  │    group request    → all owner/admin
  │
  ├─ build neutral ChatEvent (account_id = "")
  │
  ├─ for each recipient:
  │    clone ChatEvent
  │    set account_id = recipient
  │    (private only) set session_id = peer
  │    ProtocolAdapter::serialize_event(event, recipient_account)
  │    ConnectionRegistry::push(recipient, payload)   ← try_send, never await
  │
  └─ per-connection bounded mpsc (WS_QUEUE_CAPACITY)
       └─ WS writer task → TCP
```

### 2.3 WS 连接生命周期

```
GET /ws?access_token=...
  │
  ├─ authenticate_token (same function as HTTP)
  ├─ WebSocketUpgrade
  ├─ split socket
  ├─ create bounded mpsc(capacity)
  ├─ register in memory: account_id → connection_id → handle
  ├─ initial_events(account)        ← OB11 [], OB12 meta.connect future
  └─ select loop:
       ├─ cancellation (token reset / slow consumer) → close
       ├─ outbound queue → write with 10s timeout
       ├─ inbound frames → ignore action JSON; handle ping/close
       ├─ every 30s send WS Ping
       └─ every 60s revalidate token_hash against DB
```

连接断开时从注册表移除。注册表只存在内存，不支持多实例共享。

### 2.4 单文件浏览器客户端

`client.html` 由 Rust 通过 `GET /client` 内嵌返回，也可直接以 `file://` 打开。

- 只使用账号 token，与机器人/SDK 使用同一认证路径；没有特殊账号类型。
- HTTP：`fetch` + `Authorization: Bearer`；WS：`new WebSocket(...?access_token=...)`。
- 本地状态：token 与 base URL 存 `localStorage`；会话消息只在内存中。
- 每次打开会话时通过历史动作加载最近 50 条，并用 `message_id` 与实时事件去重。
- `GET /requests` 恢复刷新页面后的待审批申请。
- 私聊事件带 `peer_id` 扩展：发送方回显的 `user_id` 是自身，浏览器需要该字段把消息路由到对端会话。
- 原型为支持 `file://` 启用了 permissive CORS；没有 cookie 凭证，token 仍是唯一访问凭证。生产建议在 Nginx 收紧。
- 移动端采用单列布局：会话列表 ↔ 聊天详情；桌面为侧栏 + 主聊天区。

---

## 3. 协议抽象层

### 3.1 内部事件

`ChatEvent` 字段：

| 字段 | 用途 |
|---|---|
| `event_id` | 内部事件唯一标识 |
| `event_type` | `meta / message / notice / request` |
| `detail_type` | 具体子类型，如 `private / group / group_increase / friend` |
| `account_id` | 接收者账号；适配器输出为 OB11 `self_id` / OB12 `self.user_id` |
| `session_type` | `group / private / none` |
| `session_id` | 群号；私聊事件中为对端账号；无会话为 None |
| `sender_id` | 发送方/动作发起方（notice 中为 operator 的邻近语义） |
| `content` | `Vec<MessageSegment>`，不是 CQ 字符串 |
| `timestamp` | Unix 秒 |
| `raw` | 原始协议 JSON，仅调试；内部构造事件为 None |
| `message_id` | 原型扩展：全局消息 ID，用于去重与历史 |
| `message_seq` | 原型扩展：会话内序号，用于游标 |
| `extras` | 协议中立的附加事实：昵称、角色、群名、operator、target、request flag/comment/sub_type |

`EventExtras` 是为协议差异预留的“安全扩展点”：适配器需要的字段尽量归一化到这里，而不是污染 `ChatEvent` 的顶层命名。

### 3.2 Adapter trait

```rust
pub trait ProtocolAdapter: Send + Sync {
    fn deserialize_event(&self, raw: &str) -> Result<ChatEvent, AdapterError>;
    fn serialize_event(&self, event: &ChatEvent, self_account: &Account) -> Result<String, AdapterError>;
    fn deserialize_action(&self, raw: &str) -> Result<ActionRequest, AdapterError>;
    fn serialize_action_response(&self, resp: &ActionResult) -> Result<String, AdapterError>;
    fn action_mapping(&self) -> ActionMapping;
    fn initial_events(&self, self_account: &Account) -> Result<Vec<String>, AdapterError>;
}
```

- 唯一实现：`Ob11Adapter`。
- 消息段编解码独立在 `src/protocol/segments.rs`：OB11 输入/输出的 `{"type","data"}` 只在该文件和适配器内出现；CQ 字符串转换 TODO。
- `Ob12Adapter`：所有方法返回 `UnsupportedProtocol`，并写明 TODO。
- `initial_events` 是额外增加的首帧钩子，默认返回空，以满足“WS 建立后首先推送 OB12 meta.connect”而不影响 OB11。
- `deserialize_event` 虽然原型不使用（目前只有正向 WS 推送），但完整实现，方便未来反向 WS 或测试。

### 3.3 内部动作

```rust
pub enum InternalAction {
    SendPrivateMsg { .. },
    SendGroupMsg { .. },
    SendMsg { .. },
    GetLoginInfo,
    GetGroupList,
    GetGroupMemberList { .. },
    GetGroupMemberInfo { .. },
    GetFriendList,
    GetGroupMsgHistory { .. },
    GetFriendMsgHistory { .. },
    Unsupported { name: String },  // 仍返回标准 1404
}
```

动作字段使用内部名词（如 `user_id` 仅作为动作参数名），而存储/事件中以 `account_id`、`session_id` 为主。适配器负责 OB11 参数名与内部枚举的映射。

### 3.4 OB12 迁移步骤

未来接入 OB12 时：

1. 在 `src/protocol/ob12.rs` 实现 trait：
   - 事件：`type/detail_type/sub_type` → `EventType/detail_type/EventExtras`
   - 发送者/自身：`self.user_id` → `account_id`
   - 动作与响应：OB12 envelope
2. 在 `initial_events` 返回首帧：
   ```json
   {"type":"meta","detail_type":"connect","sub_type":"","self":{"user_id":123},"version":{"implementation":"...","version":"12"}}
   ```
3. 在启动时选择适配器（按配置或按路径/端口）。
4. WS 层无需改动：首帧在连接注册后、事件循环前发送。
5. 存储层无需迁移：`messages` 只保存 `session_type/session_id/sender_id/message_seq/content`。

---

## 4. 会话与存储设计

### 4.1 私聊 key

选择有序对方案：

```
session_type = 'private'
session_id   = f"{min(a,b)}:{max(a,b)}"
```

- 对账号 ID 做数值排序后拼接；确定性、无需额外 session 表。
- 由于账号 ID 是 BIGINT，`"1:2"` 与 `"12"` 不会冲突。
- 历史查询 `get_friend_msg_history(user_id=peer)` 由请求者 ID 和 peer 计算 canonical key，因此请求者天然只能是会话一方，不需要额外的 ACL。
- 事件层 `ChatEvent.session_id` 对私聊填“接收者的对端账号 ID”（发送者收到时是目标，目标收到时是发送者），符合字段语义；这个值不会出现在 OB11 输出中。

### 4.2 群消息 key

```
session_type = 'group'
session_id   = group_id 的十进制字符串
```

群历史权限：必须存在 `group_members(group_id, requester)`。

### 4.3 message_seq

`message_sequences(session_type, session_id, next_seq)` 作为会话级序列：

1. 事务开始。
2. `INSERT ... ON CONFLICT DO UPDATE next_seq = next_seq + 1 RETURNING next_seq`。
3. 同会话的并发消息在此行锁上串行化。
4. 用返回序号插入 `messages`；唯一约束是最后防线。
5. 提交后再扇出。

不使用时间戳；序号空洞（事务失败）可接受。历史查询只按 seq 过滤/排序，不依赖连续性。

### 4.4 扇出缓存

`SessionCache`：

- 群成员列表：`group_id → Vec<GroupMemberView>`，TTL 30 秒。
- 私聊双方账号：canonical key → 两个 `Account`，TTL 30 秒。
- 任何成员变更（入群审批、退群）立即 `invalidate_group`，不等待 TTL。
- 缓存只用于 fanout 观众查询，不用于权限判定；动作权限直接查数据库（例如发群消息、查历史、审批），避免缓存陈旧造成越权。

---

## 5. 背压隔离

关键实现（`src/registry.rs`）：

- 每连接一个 `tokio::sync::mpsc::Sender<String>`，容量 `WS_QUEUE_CAPACITY`。
- fanout 使用 `try_send`；从不 `.await` 队列或网络。
- 队列满：
  - 标记 `slow = true`；
  - 取消该连接的 `CancellationToken`；
  - 从 registry 移除；
  - 增加 `slow_disconnects` 指标（`/health` 可见）。
- WS 写任务：
  - 每次网络写有 10 秒超时，防止永久卡死；
  - 取消分支优先级最高（`biased` select）。
- 同一账号多连接时逐连接独立队列；某个设备慢不会影响同账号其他设备。

验证：`scripts/e2e.py` 的第 9 部分用一个从不读取的原始 WebSocket 连接 + 400 条 16KB 群消息，验证快连接收到全部消息、慢连接被记录并断开、扇出不等待。

---

## 6. token 生命周期与 WS 吊销

- 注册：32 字节 CSPRNG → base64url；DB 仅 `SHA-256` hex。
- token 指纹：哈希前 12 个字符，供用户识别。
- 每个 HTTP 请求：查 `accounts.token_hash` 并校验 `disabled`。
- WS 握手：同一 `authenticate_token` 函数，仅从 query 取 token。
- WS 连接保存握手时的 token_hash。
- 重置：
  1. 事务中生成新 token、更新 `token_hash`、写审计；
  2. 提交后 `registry.kick_account(account_id)`；
  3. 旧 HTTP 下一次请求 401；旧 WS 收到取消并关闭。
- 周期复核：每 60 秒查一次当前 hash/disabled，作为主动踢失效后的兜底。
- 审计：`token_reset_audit(account_id, actor, action, reason, created_at)`；actor 为用户名或 `admin:<X-Admin-Actor>`。

---

## 7. 安全与边界

- 数据库不存明文 token。
- `REGISTER_KEY` 未设置时注册关闭（默认安全）。
- 注册 key 和管理 key 使用常量时间比较。
- username 限定 ASCII 字母数字和 `_-.`，统一小写，避免大小写碰撞。
- HTTP body 限制 2MB；WS 入站消息/帧限制 64KB（WS 本来只推事件）。
- Nginx 样例不记录 query，避免 token 进入访问日志。
- CORS 为 permissive（为了单文件客户端 `file://` 可用）；API 无 cookie，token 仍是唯一凭证。生产应限制允许来源。
- 没有 CSRF 风险面（不使用 cookie 会话）；不建议在不可信页面粘贴 token。
- token 在 query 中传输的 WS 方式依赖 TLS（Nginx）保护；不建议明文公网部署。

---

## 8. 并发与容量

- PostgreSQL 连接池 max 20。
- 每会话消息发送通过序列行锁串行化，不同会话可并发。
- 每 WS 连接独立任务和队列；慢消费者隔离。
- 进程内缓存与连接表意味着当前**不能水平多实例共享**；如有需要，后续替换 registry/cache 为 Redis/Postgres 通知。
- 历史永久保留，未实现清理任务；大表需要后续保留窗口策略。

---

## 9. 关键不变量

1. `messages.message_seq` 在同一 `(session_type, session_id)` 内单调唯一。
2. 每个出站消息事件的 `account_id` 等于接收账号。
3. 私聊受众严格等于有序对中的两个账号；群受众严格来自成员表（缓存失效后）。
4. 扇出路径不存在等待网络的操作。
5. `src/protocol/` 之外没有 `post_type` / `self_id` 等 OB11 事件字段（模型注释除外）。
6. 存储 schema 的 `messages` 与游标列不引用协议版本字段。
7. token 重置后旧哈希立即无法认证；审计与更新同事务。
