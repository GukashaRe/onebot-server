# Changelog

本项目遵循 [Semantic Versioning](https://semver.org/)；提交与发布流程见 [CONTRIBUTING.md](CONTRIBUTING.md)。

## [Unreleased]

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

[Unreleased]: https://github.com/GukashaRe/onebot-server/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/GukashaRe/onebot-server/releases/tag/v0.1.0
