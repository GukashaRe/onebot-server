# Changelog

本项目遵循 [Semantic Versioning](https://semver.org/)；提交与发布流程见 [CONTRIBUTING.md](CONTRIBUTING.md)。

## [Unreleased]

## [0.2.0] - 2026-11-16

### Added

- 4 个 OB11 群管动作：`set_group_kick`、`set_group_ban`、`set_group_admin`、`set_group_card`。
- 基于群成员角色的服务端权限矩阵；owner/admin/member 越权统一返回 retcode 1403。
- 持久化禁言表 `group_bans`，支持时长、解除、重启后仍然生效，并在群消息发送路径强制拦截。
- 群内名片持久化：`group_members.card`，成员信息、消息 sender 与通知均可反映。
- `audit_logs` 群管审计：操作者、动作、目标、群、detail JSONB、时间；业务变更与审计同事务。
- 新 notice：`group_decrease(kick/kick_me)`、`group_ban`、`group_admin`、`group_card`，全部 per-account 填充 `self_id`。
- 14 个 Rust 单元测试与 149 项端到端检查，覆盖权限矩阵、禁言持久化、审计与 per-account 通知。

### Changed

- `get_group_member_list` / `get_group_member_info` 返回真实群名片。
- 群消息与历史 sender 使用 `group_members.card`，成员角色/名片变更后缓存立即失效。
- 群成员退群/被踢时清理对应禁言记录。
- `set_group_kick` 的 `reject_add_request` 参数会被解析并记录到审计 detail；持久化“拒绝再次加群”仍为 TODO。

### Security

- 群管权限不再由 HTTP 层信任客户端参数，全部在数据库事务内重新查询角色并加行锁。
- 禁言状态只存 PostgreSQL，避免内存状态在重启后丢失。
- 每次成功群管操作均写审计；权限失败不写审计。

## [0.1.0] - 2026-11-16

### Added

- OneBot 11 原型服务端（Rust + PostgreSQL）：账号注册、token 认证、HTTP 动作、正向 WebSocket 事件。
- 协议中立的 `ChatEvent` / `ActionRequest` / `ActionResult` 模型与 `ProtocolAdapter` 抽象。
- OB11 适配器；OB12 适配器骨架与 `meta.connect` 首帧钩子。
- 私聊、群聊、消息回显、群成员角色、好友/入群申请与审批。
- 基于 `message_seq` 的群/私聊历史游标与权限校验。
- per-account WS 事件扇出、有界队列、慢消费者隔离与 token 周期复核。
- 自助/服管 token 重置、审计日志、token 哈希存储。
- 零依赖单文件 HTML 客户端（`client.html`，由 `GET /client` 提供）。
- 待审批申请汇总接口 `GET /requests`。
- Nginx 部署示例、API/架构/测试文档与 86 项端到端测试脚本。

### Security

- 数据库只存 token 的 SHA-256 哈希；`.env` 与真实凭据不入库。
- 注册默认关闭，需配置 `REGISTER_KEY`。

[Unreleased]: https://github.com/GukashaRe/onebot-server/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/GukashaRe/onebot-server/releases/tag/v0.2.0
[0.1.0]: https://github.com/GukashaRe/onebot-server/releases/tag/v0.1.0
