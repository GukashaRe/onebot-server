# 测试与验收

验证环境（本机）：

- Windows 11
- Rust 1.99.1 / Cargo 1.99.1（edition 2024）
- PostgreSQL 18.7（`onebot_dev`）
- Python 3.15.2 + `websockets` 17.4
- UI 测试：Playwright 1.65.2 + Chromium（缓存的 headless Chromium）
- 服务监听 `127.0.0.1:8080`（明文 HTTP/WS）
- 测试日期：2026-11-16
- 依赖：见 `Cargo.lock`，直接依赖均为当前最新 stable 版本（axum 0.8.9、sqlx 0.9.0、tokio 1.54.1、tower-http 0.7.1、serde 1.0.229、serde_json 1.0.154、sha2 0.11.1、base64 0.23.1、getrandom 0.4.3、thiserror 2.0.21 等）

## 1. 运行自动化验收

前置：

```bash
createdb -h 127.0.0.1 -U postgres onebot_dev
cp .env.example .env
# 保证 REGISTER_KEY / ADMIN_KEY 与测试环境一致
cargo run --release
```

另开终端：

```bash
python -m pip install -r scripts/requirements.txt
export REGISTER_KEY='<与 .env 一致>'
export ADMIN_KEY='<与 .env 一致>'
python scripts/e2e.py
```

脚本会创建随机后缀的隔离账号，不依赖已有数据；可重复运行。慢消费者测试发送 400 条 16KB 消息，通常需要十几秒。由于该测试会快速发送大量动作，服务端测试配置建议使用较高的 `RATE_LIMIT_DEFAULT`（例如 1000）；生产/常规开发可保持默认 20。

环境变量：

- `BASE_URL`，默认 `http://127.0.0.1:8080`
- `REGISTER_KEY`：必填，需与服务端配置一致
- `ADMIN_KEY`：必填，需与服务端配置一致
- `RUN_SLOW_TEST=0` 可跳过慢消费者测试
- `E2E_STATE_FILE`：可选；把最后一组群管测试的账号/群/token 写入指定文件，用于重启后禁言验证（该文件含 token，勿提交）

## 2. 最近一次运行结果

```
[1] registration, 409, token hash storage, auth          5 PASS
[2] private message loop and sender echo                 7 PASS
[3] friend request/approve loop                         11 PASS
[4] group create/join/approve loop and notices          15 PASS
[5] group message fanout / send_msg / member info       11 PASS
[6] history cursor, pagination, offline catch-up        17 PASS
[7] unsupported action follows OB11 error shape          3 PASS
[8] token reset revokes HTTP and kicks WS                6 PASS
[9] slow WS does not block a fast WS                     4 PASS
[10] group leave and permission checks                   7 PASS
[11] group management actions, permissions, notices      63 PASS

ALL 149 CHECKS PASSED
```

编译质量：

```bash
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
```

本机结果：`cargo test` 共 14 个单元测试全部通过；`cargo clippy --all-targets -- -D warnings` 无 warning。

## 3. 单文件客户端 UI 验证

用 Playwright + Chromium 额外验证了：

- 注册新账号后自动建立 WS，`self_id`/账号信息正确显示
- 两个浏览器上下文完成好友申请 → 申请中心审批 → 双方好友列表
- 创建群 → 对方申请 → owner WS 收到申请 → 审批 → 群成员收到 `group_increase`
- 群聊消息双向实时显示；私聊消息通过 `peer_id` 正确路由到对端会话
- 页面刷新后用 localStorage token 自动重连，会话与消息去重正常
- 设置面板、token 指纹、复制 token、token 重置 → 旧 token 401 → 新 token 自动重连
- 桌面端与 390×844 移动端布局：移动端会话列表/聊天页切换、返回按钮
- 直接打开 `file://.../client.html` 也能注册并连接（服务端 CORS 生效）
- 控制台无 pageerror / 非预期 console error；favicon 修复后不再出现 401

UI 测试使用临时账号，不依赖仓库内测试数据。

## 4. 验收标准逐项映射

| # | 验收标准 | 自动化检查 | 结果 |
|---:|---|---|---|
| 1 | 服务启动，连通 PG，监听明文端口 | 启动日志 + `GET /health`（database up） | 通过 |
| 2 | 调注册端点拿 token | e2e [1] 注册返回 token/指纹/URL/能力 | 通过 |
| 3 | 用 token 连 WS 能收事件 | e2e 三个账号 WS 连接，后续事件均通过 WS 收到 | 通过 |
| 4 | 私聊闭环：双方收到 private，self_id 各自账号 | e2e [2]：sender/receiver 事件 `self_id` 分别断言 | 通过 |
| 5 | 群聊闭环：A/B/C 同群，A 发消息，B/C 收 group + 各自 self_id | e2e [5]：三端事件同时断言 | 通过 |
| 6 | 群/私聊历史补拉，按 message_id 去重后无重复无遗漏 | e2e [6]：after_seq 分页逐页拉取、ID 唯一、seq 升序、与全量一致；断线期间群/私聊消息补拉 | 通过 |
| 7 | 未实现动作返回标准“不支持”错误，不是 500 | e2e [7]：`set_group_leave` HTTP 200 / failed / 1404 / data=null / 仅三字段 | 通过 |
| 8 | 重置 token 后旧 token HTTP 401，旧 WS 被踢 | e2e [8]：自助重置 + 管理员重置，旧 token 401、旧 WS closed | 通过 |
| 9 | 重复注册同一 username 返回 409 | e2e [1]：用大写用户名重复注册，返回 409 | 通过 |
| 10 | 慢 WS 不阻塞其他连接推送 | e2e [9]：原始 WS 完成握手后不读；400 条 16KB 群消息；快连接全收；服务端记录 slow_disconnect；耗时 < 60s | 通过 |
| 11 | 私聊回显，发送方按 message_id 去重后不重复 | e2e [2]：发送方和接收方同一 message_id；回显事件 | 通过 |
| 12 | 加好友闭环 | e2e [3]：A 申请 → B WS 收 friend request（flag）→ B approve → 双方 friend list | 通过 |
| 13 | 加群闭环 | e2e [4]：A 建群，B/C 申请 → A WS 收两条 group request（sub_type=add）→ 审批 → 成员收到 group_increase → get_group_list | 通过 |
| 14 | 建群后创建者 owner，get_group_list 可查 | e2e [4]：`owner_id`、role=owner、get_group_list | 通过 |
| 15 | 协议抽象验证 | 静态 grep：`src/` 中（除 `src/protocol/` 与注释）没有 `post_type/self_id/notice_type/request_type`；迁移中 messages/cursor 无协议字段 | 通过 |
| 16 | 踢人权限矩阵、group_decrease(kick)/kick_me、被踢后不再收群消息 | e2e [11]：owner 踢 member；admin 踢 member；admin 踢 admin/owner → 1403；member → 1403；kick_me self_id；被踢后新群消息不再投递 | 通过 |
| 17 | 禁言持久化：60 秒禁言阻止发送；重启仍生效；过期恢复 | e2e [11] 验证 admin 禁言 member 60s、send_group_msg/send_msg 1403、消息未落库；重启服务后再次发送仍 1403；手工把 `banned_until` 改到过去后发送成功并写入历史 | 通过 |
| 18 | 禁言权限矩阵 | e2e [11]：owner 禁言 admin 成功；admin 禁言 owner → 1403；member 禁言任何人 → 1403；duration=0 解除并推 lift_ban | 通过 |
| 19 | 设管理员与撤管理员、权限变化 | e2e [11]：owner 设置 member 为 admin → 通知 set；该账号成功踢 member；owner 撤销 → 通知 unset；其再调 set_group_admin → 1403 | 通过 |
| 20 | 名片权限与 group_card | e2e [11]：owner 改 admin/自己成功；admin 改 member 成功；admin 改 owner → 1403；member → 1403；通知含 card/old_card；成员信息持久化 | 通过 |
| 21 | 群管审计完整 | 数据库核对 `audit_logs`：kick/ban/admin/card 成功操作均有 actor_id/action/target_id/group_id/detail/created_at，缺失字段数为 0 | 通过 |
| 22 | 其他未实现动作仍返回标准错误 | e2e [11]：`set_group_leave`、`delete_msg` → 200/failed/1404/data=null | 通过 |
| 23 | 群管 notice per-account self_id | e2e [11]：group_admin/group_ban/group_card/group_decrease 在多个接收者上分别断言 self_id 等于自身账号 ID | 通过 |

## 5. 额外覆盖

e2e 还验证了：

- 未设置 `register_key` 注册返回 403；
- `send_msg` 按 private/group 分发；
- `get_group_member_info` 返回成员角色；
- owner 退群被 403 拒绝；
- 普通成员退群成功，剩余成员收到 `group_decrease`，退群者收不到；
- 退群后 `get_group_list` 不再包含该群，历史查询返回 1403；
- 群成员列表反映退群；
- `get_group_member_list` 包含所有审批通过成员；
- token 重置返回 `revoked_connections`；
- 认证失败为标准 OB11 失败包；
- `GET /requests` 返回待审批好友/入群申请及申请者用户名；
- 私聊事件包含 `peer_id`，发送方回显可正确定位对端会话；
- CORS 预检返回允许来源/方法/头，`file://` 客户端可正常注册并连接 WS；
- 慢消费者记录在 `/health` 的 `slow_disconnects` 中；
- 群管动作的成功审计行数：group_kick=3、group_ban=3（含一次 60s、一次 30s、一次 0s 解除）、group_admin=4、group_card=3；
- 被禁言成员调用 `send_group_msg` 与 `send_msg` 均在落库前返回 1403；
- 被踢成员不再收到群消息；
- duration=0 的 `group_ban` 推 `lift_ban` 且 duration=0。

### 禁言重启验证

e2e 可通过 `E2E_STATE_FILE` 输出最后一组测试的群/账号/token 状态（文件含 token，请放在仓库外并及时删除）：

```bash
export E2E_STATE_FILE="$TEMP/onebot-e2e-state.json"
python scripts/e2e.py
```

然后：

1. 停掉并重启 Rust 服务；
2. 使用 state 里的 `m3_token` 调 `send_group_msg`，应仍返回 1403；
3. 执行：
   ```sql
   UPDATE group_bans
   SET banned_until = now() - interval '1 second'
   WHERE group_id = <state.group_id> AND account_id = <state.m3_account_id>;
   ```
4. 再次发送，应成功并出现在历史中。

本机已按此步骤验证通过。

## 6. 数据库核对

运行下列查询可复核 token 哈希、审计与序号：

```sql
-- 只存 64 位 SHA-256 hex，不存明文
SELECT count(*) accounts,
       bool_and(length(token_hash) = 64) all_sha256_hex,
       bool_and(token_hash ~ '^[0-9a-f]{64}$') valid_shape
FROM accounts;

-- token 重置审计
SELECT account_id, actor, action, reason, created_at
FROM token_reset_audit
ORDER BY id;

-- 群管审计
SELECT actor_id, action, target_type, target_id, group_id, detail, created_at
FROM audit_logs
ORDER BY id;

-- 审计完整性
SELECT count(*) FILTER (
         WHERE actor_id IS NULL OR action IS NULL OR target_id IS NULL
            OR group_id IS NULL OR created_at IS NULL
       ) AS incomplete,
       count(*) AS total
FROM audit_logs;

-- 持久化禁言
SELECT group_id, account_id, banned_until, operator_id, created_at
FROM group_bans;

-- 会话内 message_seq 单调且无重复
SELECT session_type, session_id,
       count(*) AS messages,
       min(message_seq), max(message_seq),
       count(DISTINCT message_seq) AS distinct_seq
FROM messages
GROUP BY 1, 2;
```

本机结果示例：

```text
所有 accounts 行：all_sha256_hex=t, valid_shape=t
audit_logs 中 group 11: group_kick=3, group_ban=3, group_admin=4, group_card=3
审计缺失字段：incomplete=0, total=13
group/<id>: 403 条消息，min_seq=1，max_seq=403，distinct_seq=403
private/<a>:<b>: 3 条消息，min_seq=1，max_seq=3，distinct_seq=3
```

## 7. 协议抽象静态检查

PowerShell：

```powershell
Get-ChildItem src -Recurse -Filter *.rs |
  Where-Object { $_.FullName -notmatch '\\protocol\\' } |
  Select-String -Pattern 'post_type|self_id|notice_type|request_type' -CaseSensitive
```

结果只有 `src/model/event.rs` 中说明“这些字段只属于适配器”的注释；没有代码使用这些 OB11 字段。存储 schema 的 `messages` 和游标均使用 `session_type/session_id/message_seq`。

## 8. 手工验证建议

- `curl /health`
- 用 `psql` 检查 `accounts.token_hash` 不是 token 明文
- 修改 `.env` 的 `WS_QUEUE_CAPACITY`（例如 8）重跑 e2e，慢消费者部分会更早触发
- 设置 `RUST_LOG=onebot_server=debug` 观察 WS 连接/断开、慢消费者与 token 复核
- 将 `WS_REVALIDATE_SECS` 调小（如 5），在数据库直接更换 token_hash，观察周期复核断开

## 9. 测试脚本说明

`scripts/e2e.py` 使用：

- 标准库 `urllib` 发 HTTP；
- `websockets` 客户端接收 WS 事件；
- 原始 TCP 手工完成 WebSocket 握手，用于模拟不读取数据的慢消费者；
- `asyncio.gather` 并发发送 400 条大消息，验证背压隔离。

测试不依赖外部 SaaS 或网络，仅访问本机服务。
