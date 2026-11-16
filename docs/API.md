# API 参考

Base URL 示例：`http://127.0.0.1:8080`。生产环境由 Nginx 暴露 HTTPS / WSS。

## 0. 约定

### 认证

- HTTP：`Authorization: Bearer <token>`
- WS：`GET /ws?access_token=<token>`（唯一 WS 认证方式）
- 没有登录端点；token 即账号。
- 除 `/health`、`/register`、`/ws`、`/admin/...` 外，所有端点都需要 Bearer token。
- 未认证/失效 token 返回 HTTP 401，响应体也是 OB11 失败包：

```json
{"status":"failed","retcode":1401,"data":null}
```

### ID 类型

协议输入中的账号/群 ID 可以是 JSON 数字或数字字符串；推荐用数字。数据库内部账号 ID 为 BIGINT，ChatEvent 中对应字符串。

### OB11 动作响应

所有动作统一返回三字段（成功/失败均 HTTP 200，认证失败除外）：

```json
{"status":"ok","retcode":0,"data":{}}
```

```json
{"status":"failed","retcode":1404,"data":null}
```

失败不会返回 HTTP 500。`data` 在成功时为动作定义的 JSON，失败时为 `null`。

### 消息段

只接受消息段数组，**不接受 CQ 字符串**：

```json
{"message":[{"type":"text","data":{"text":"hello"}}]}
```

传字符串会得到 `retcode:1400`。CQ 编解码是 TODO。

### 自定义端点错误

自定义关系端点和注册端点使用：

```json
{"error":{"code":"conflict","message":"a pending friend request already exists"}}
```

HTTP 状态码表达语义：400 参数错误、401 未认证、403 禁止、404 不存在、409 冲突、500 内部错误。

### CORS

本原型对 API 启用了 permissive CORS，以支持直接通过 `file://` 打开单文件客户端。API 仅使用 Bearer token、不使用 cookie，所以 CORS 本身不授予访问权。生产部署建议在 Nginx 层收紧来源或仅允许同源 `/client`。

---

## 1. 公共端点

### `GET /health`

```json
{
  "ok": true,
  "database": "up",
  "active_connections": 2,
  "slow_disconnects": 0
}
```

数据库不可用时返回 503。

### `GET /client`

返回内嵌的单文件 HTML 聊天客户端（`client.html`，`text/html`）。同名文件也可直接从磁盘打开；若使用 `file://`，请确认服务端地址填写正确且允许 CORS。

### `GET /favicon.ico`

返回 204，避免浏览器 favicon 请求落入动态受保护路由。

### `POST /register`

请求：

```json
{
  "username": "alice",
  "labels": ["human", "e2e"],
  "register_key": "your-register-key"
}
```

- `username` 必填，去除首尾空白后转小写；3–32 字符，首字符字母或数字，其余允许 ASCII 字母数字和 `_-.`。
- `labels` 可选，最多 32 个，每个最长 64 字符，去重。
- `register_key`：服务端设置了 `REGISTER_KEY` 时必填且必须匹配；未设置时整个注册端点关闭。
- 成功：HTTP 200。
- 重复 username（大小写不敏感）：HTTP 409。
- 缺少/错误注册 key：HTTP 403。
- 服务未配置 `REGISTER_KEY`：HTTP 403，`code: registration_disabled`。

成功响应：

```json
{
  "account_id": 1,
  "username": "alice",
  "token": "仅这一次返回的明文 token",
  "token_fingerprint": "4d7c31806bf4",
  "http_base_url": "http://127.0.0.1:8080",
  "ws_url": "ws://127.0.0.1:8080/ws",
  "ws_url_with_token": "ws://127.0.0.1:8080/ws?access_token=...",
  "capabilities": {
    "protocol": "OneBot",
    "version": "11",
    "message_format": "array",
    "ws_role": "event_push_only",
    "ws_auth": "query parameter access_token",
    "http_auth": "Authorization: Bearer <token>",
    "actions": [
      "send_private_msg",
      "send_group_msg",
      "send_msg",
      "get_login_info",
      "get_group_list",
      "get_group_member_list",
      "get_group_member_info",
      "get_friend_list",
      "get_group_msg_history",
      "get_friend_msg_history"
    ],
    "rate_limit_per_second": 20
  },
  "notice": "The token is the account. It is shown once and cannot be recovered; a server operator can reset it. Keep it secret."
}
```

数据库只存 SHA-256 哈希（64 hex）；token 指纹是哈希前 12 位。

### `GET /ws?access_token=<token>`

升级 WebSocket。握手失败返回 HTTP 401（JSON）。

- 仅推送事件；客户端发送的动作 JSON 会被忽略。
- 连接建立后不会立即有 OB11 初始化事件；OB12 适配器未来会在这里先发 `meta.connect`。
- 服务端 30 秒发一次 Ping；每 60 秒复核 token。
- 慢消费者会被断开，不阻塞其他连接。

### `POST /admin/accounts/{account_id}/token/reset`

请求头：

```http
X-Admin-Key: <ADMIN_KEY>
X-Admin-Actor: operator-name   # 可选，审计用；默认 server-admin
```

请求体可省略（`{}`）。返回：

```json
{
  "account_id": 5,
  "username": "dave",
  "token": "新明文 token",
  "token_fingerprint": "……",
  "ws_url_with_token": "ws://.../ws?access_token=...",
  "revoked_connections": 2,
  "actor": "operator-name",
  "notice": "The previous token is invalid immediately. This token is shown once."
}
```

`ADMIN_KEY` 未配置时返回 403，端点关闭。

---

## 2. 动作调用入口

以下均需要 Bearer token。

| 入口 | 请求格式 |
|---|---|
| `POST /` | `{"action":"...","params":{...}}` |
| `POST /api` | 同 `/` |
| `POST /action` | 同 `/` |
| `POST /api/{action}` | params 直接放 body；如果 body 含 `action` 字段则按完整 envelope 处理 |
| `POST /{action}` | 同 `/api/{action}` |

`echo` 字段当前不解析也不回显（原型限制）。未知字段忽略。

### 示例

```bash
curl -sS http://127.0.0.1:8080/ \
  -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{
    "action":"send_private_msg",
    "params":{
      "user_id":2,
      "message":[{"type":"text","data":{"text":"hello"}}]
    }
  }'
```

```bash
curl -sS http://127.0.0.1:8080/api/get_group_list \
  -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' -d '{}'
```

---

## 3. 动作：消息发送

### `send_private_msg`

参数：

| 字段 | 类型 | 必需 | 说明 |
|---|---|---:|---|
| `user_id` | number/string | 是 | 对端账号 ID；账号必须存在；不能发给自己 |
| `message` | segment[] | 是 | 至少一个消息段 |

私聊不要求好友关系。

成功：

```json
{"status":"ok","retcode":0,"data":{"message_id":12,"message_seq":3}}
```

事件会推给发送方（回显）和接收方，双方都带同一个 `message_id`，客户端应去重展示。

### `send_group_msg`

| 字段 | 类型 | 必需 | 说明 |
|---|---|---:|---|
| `group_id` | number/string | 是 | 群 ID；发送者必须是群成员 |
| `message` | segment[] | 是 | 至少一个消息段 |

成功响应同 `send_private_msg`。

### `send_msg`

按 `message_type` 或目标字段分发。

| 字段 | 类型 | 必需 | 说明 |
|---|---|---:|---|
| `message_type` | `private`/`group` | 否 | 显式类型；与对应目标字段一起使用 |
| `user_id` | number/string | 条件 | private 目标 |
| `group_id` | number/string | 条件 | group 目标 |
| `message` | segment[] | 是 | 至少一个消息段 |

- 有 `message_type` 时必须提供对应目标。
- 无 `message_type` 时按唯一出现的 `user_id`/`group_id` 推断；同时出现两者则报参数错误。

---

## 4. 动作：信息查询

### `get_login_info`

无参数。返回：

```json
{"user_id":1,"nickname":"alice"}
```

### `get_group_list`

无参数。返回该账号加入的群数组：

```json
[
  {
    "group_id":1,
    "group_name":"Group name",
    "member_count":3,
    "max_member_count":200,
    "owner_id":1,
    "role":"owner"
  }
]
```

`max_member_count` 为展示值，未实际限制人数。`role`/`owner_id` 是额外字段。

### `get_group_member_list`

参数 `{"group_id":1}`。请求者必须是群成员。返回：

```json
[
  {
    "group_id":1,
    "user_id":1,
    "nickname":"alice",
    "card":"",
    "role":"owner",
    "join_time":1763300000
  }
]
```

### `get_group_member_info`

参数 `{"group_id":1,"user_id":2}`。请求者必须是群成员；目标也必须是成员。返回单对象，字段同上。

### `get_friend_list`

无参数。返回：

```json
[{"user_id":2,"nickname":"bob","remark":""}]
```

按 nickname 排序。好友关系为有序对存储，不影响展示。

---

## 5. 动作：历史消息

### `get_group_msg_history`

| 字段 | 类型 | 必需 | 说明 |
|---|---|---:|---|
| `group_id` | number/string | 是 | 请求者必须是群成员 |
| `before_seq` | integer | 否 | 只取 `message_seq < before_seq` |
| `after_seq` | integer | 否 | 只取 `message_seq > after_seq` |
| `limit` | integer | 否 | 默认 20，最大 100 |

返回：

```json
{
  "messages": [
    {
      "post_type":"message",
      "message_type":"group",
      "sub_type":"normal",
      "time":1763300000,
      "self_id":1,
      "message_id":12,
      "message_seq":3,
      "group_id":1,
      "user_id":1,
      "message":[{"type":"text","data":{"text":"hi"}}],
      "raw_message":"hi",
      "font":0,
      "sender":{"user_id":1,"nickname":"alice","card":"","role":"owner"}
    }
  ]
}
```

- 无论传 before 还是 after，结果都按 `message_seq` **升序**。
- 不传游标时取最新 `limit` 条，再按升序返回。
- 游标语义是半开区间：`after_seq` 不含该 seq，`before_seq` 不含该 seq。
- 只支持 `message_seq` 游标；不支持 offset/message_id 分页。
- 私聊消息事件额外带 `peer_id`（对端账号）扩展，便于客户端把发送方回显路由到正确会话。它是兼容扩展，OB11 客户端可忽略。

### `get_friend_msg_history`

| 字段 | 类型 | 必需 | 说明 |
|---|---|---:|---|
| `user_id` | number/string | 是 | 会话对端；请求者与该 user 组成私聊会话 |
| `before_seq` | integer | 否 | 同上 |
| `after_seq` | integer | 否 | 同上 |
| `limit` | integer | 否 | 同上 |

请求者只可能是该私聊会话一方（canonical key 由请求者 ID + 对端 ID 计算）。返回结构与群历史一致，`message_type:"private"`，无 `group_id`。

### 客户端补拉示例

```python
seen = set()
after = 0
while True:
    data = call("get_group_msg_history", {"group_id": 1, "after_seq": after, "limit": 100})
    batch = data["messages"]
    if not batch:
        break
    for message in batch:
        if message["message_id"] not in seen:
            seen.add(message["message_id"])
            process(message)
    after = batch[-1]["message_seq"]
```

---

## 6. 自定义关系 API

所有端点均需 Bearer token。

### `POST /friends/request`

```json
{"target_id":2}
```

成功（HTTP 200）：

```json
{
  "request_id":1,
  "status":"pending",
  "flag":"1",
  "from_id":1,
  "to_id":2
}
```

效果：给 target 的在线 WS 推：

```json
{
  "post_type":"request",
  "request_type":"friend",
  "time":1763300000,
  "self_id":2,
  "user_id":1,
  "comment":"alice requested to add you as a friend",
  "flag":"1"
}
```

冲突：已好友、已有 pending、目标不存在、目标为自己。

### `POST /friends/approve`

```json
{"request_id":1,"approve":true}
```

也接受 `flag` 代替 `request_id`：

```json
{"flag":"1","approve":true}
```

只有申请目标可以审批。同意后写 `friends` 有序对。响应包含 `request_id`、`status`、`from_id`、`to_id`。

### `POST /groups`

```json
{"name":"My Group"}
```

创建者自动为 owner，返回：

```json
{"group_id":1,"name":"My Group","owner_id":1,"role":"owner"}
```

### `POST /groups/{group_id}/join`

空 body `{}`。创建 pending 入群申请，返回：

```json
{
  "request_id":1,
  "flag":"1",
  "group_id":1,
  "account_id":2,
  "status":"pending"
}
```

效果：向群所有 owner/admin 推：

```json
{
  "post_type":"request",
  "request_type":"group",
  "sub_type":"add",
  "time":1763300000,
  "self_id":1,
  "group_id":1,
  "user_id":2,
  "comment":"bob requested to join My Group",
  "flag":"1"
}
```

已是成员、已有 pending、群不存在会报错。

### `POST /groups/{group_id}/members/approve`

```json
{"request_id":1,"approve":true}
```

或 `{"flag":"1","approve":true}`。只有 owner/admin 可以审批，且 request 必须属于该 group。

同意后：插入成员、失效群成员缓存、向**当前所有群成员**推：

```json
{
  "post_type":"notice",
  "notice_type":"group_increase",
  "sub_type":"approve",
  "time":1763300000,
  "self_id":1,
  "group_id":1,
  "operator_id":1,
  "user_id":2
}
```

### `POST /groups/{group_id}/leave`

空 body `{}`。owner 退群返回 HTTP 403。普通成员退群后：

- 删除成员，失效缓存；
- 向剩余成员推 `group_decrease`（`sub_type:"leave"`，operator/user 均为退群者）；
- 退群者本人**不会**收到该 notice；
- 响应 `{"group_id":1,"left":true}`。

### `GET /requests`

返回当前账号可处理的 pending 申请。用于客户端刷新后恢复申请列表（WS 实时 request 事件之外）。

```json
{
  "friend_requests": [
    {
      "request_id": 1,
      "flag": "1",
      "from_id": 2,
      "from_username": "bob",
      "created_at": "2026-11-16T05:02:54.697838+00:00"
    }
  ],
  "group_join_requests": [
    {
      "request_id": 7,
      "flag": "7",
      "group_id": 3,
      "group_name": "My Group",
      "account_id": 4,
      "account_username": "carol",
      "created_at": "2026-11-16T05:02:54.734781+00:00"
    }
  ]
}
```

- `friend_requests` 只包含发给当前账号的 pending 好友申请。
- `group_join_requests` 只包含当前账号作为 owner/admin 的群中的 pending 入群申请。
- 审批仍使用 `POST /friends/approve` 与 `POST /groups/{group_id}/members/approve`。

### `POST /me/token/reset`

无 body 或 `{}`。需要当前有效 Bearer token。返回：

```json
{
  "account_id":5,
  "token":"新 token",
  "token_fingerprint":"……",
  "ws_url_with_token":"ws://.../ws?access_token=...",
  "revoked_connections":1,
  "notice":"The previous token is invalid immediately. This token is shown once."
}
```

旧 token 立即失效；该账号所有 WS 被踢。审计 actor 为用户名。

---

## 7. 错误码

| retcode | 含义 | 典型场景 |
|---:|---|---|
| `0` | 成功 | 所有成功动作 |
| `1200` | 限流 | 超过账号的每秒动作配额（原型扩展码） |
| `1400` | 参数错误 | 非法 JSON、缺失字段、CQ 字符串、非法 message_type |
| `1401` | 未认证 | HTTP 401 / 无效 token |
| `1403` | 禁止 | 非群成员查历史、非成员发群消息、发私聊给自己 |
| `1404` | 未找到 / 不支持 | 账号/群不存在、未实现动作（如 `set_group_kick`） |
| `1500` | 内部错误 | 数据库/序列化异常；仍返回标准 envelope |

未实现动作示例：

```http
POST / HTTP/1.1
Authorization: Bearer <token>
Content-Type: application/json

{"action":"set_group_kick","params":{"group_id":1,"user_id":2}}
```

```http
HTTP/1.1 200 OK
Content-Type: application/json

{"status":"failed","retcode":1404,"data":null}
```

---

## 8. WebSocket 客户端示例

```python
import asyncio, json, websockets

async def main():
    uri = f"ws://127.0.0.1:8080/ws?access_token={TOKEN}"
    async with websockets.connect(uri) as ws:
        async for raw in ws:
            event = json.loads(raw)
            print(event["post_type"], event.get("message_id"))

asyncio.run(main())
```

注意：token 在 URL query 中，确保链路 TLS 且 Nginx 日志不记录 query。

---

## 9. 能力声明

`/register` 的 `capabilities.actions` 严格等于已实现动作：

- `send_private_msg`
- `send_group_msg`
- `send_msg`
- `get_login_info`
- `get_group_list`
- `get_group_member_list`
- `get_group_member_info`
- `get_friend_list`
- `get_group_msg_history`
- `get_friend_msg_history`

其他动作统一返回 1404。OB11 请求审批动作（`set_friend_add_request`、`set_group_add_request`）不在其中；审批使用本文第 6 节的自定义端点。
