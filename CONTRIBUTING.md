# 贡献与版本流程

本项目使用 **Semantic Versioning 2.0.0** 管理版本，使用 **Conventional Commits** 管理提交信息与发布记录。

## 1. 分支与提交

- 主分支：`main`，始终可构建、可测试。
- 功能/修复使用短生命周期分支，合并回 `main`。
- 提交前至少运行：

```bash
cargo fmt --all -- --check
cargo test
cargo clippy --all-targets -- -D warnings
```

### 提交信息格式

```text
<type>(<scope>): <subject>

[optional body]

[optional footer(s)]
```

- `type` 使用小写；`subject` 使用祈使语气、简短描述，不加句号，尽量不超过 72 字符。
- `scope` 可选，建议使用模块名，例如 `auth`、`ws`、`protocol`、`store`、`client`、`docs`。
- 破坏性变更：type 后加 `!`，并在 footer 写 `BREAKING CHANGE: ...`。

允许的 type：

| type | 用途 | 版本影响 |
|---|---|---|
| `feat` | 新功能 | MINOR |
| `fix` | 修复 bug | PATCH |
| `perf` | 性能优化 | PATCH |
| `refactor` | 不改变行为的重构 | 不单独发版 |
| `docs` | 文档 | 不单独发版 |
| `test` | 测试 | 不单独发版 |
| `build` | 构建/依赖 | 不单独发版 |
| `ci` | CI 配置 | 不单独发版 |
| `chore` | 其他维护 | 不单独发版 |
| `revert` | 回滚 | 视回滚内容 |

示例：

```text
feat(protocol): add OB12 adapter skeleton
fix(ws): close slow consumers without blocking fanout
docs(api): document peer_id extension
refactor(store)!: replace message cursor

BREAKING CHANGE: message cursors no longer accept timestamps.
```

## 2. 版本号

版本号格式：`MAJOR.MINOR.PATCH`，唯一来源是 `Cargo.toml` 的 `package.version`。

- `MAJOR`：不兼容的 API/数据/协议变更。
- `MINOR`：向后兼容的新功能。
- `PATCH`：向后兼容的修复。
- 预发布版本使用 `X.Y.Z-alpha.N` / `X.Y.Z-rc.N`。
- 1.0.0 之前（当前 0.x）处于初始开发期：破坏性变更提升 MINOR（例如 `0.1.0 → 0.2.0`）；1.0.0 之后严格按 SemVer 提升 MAJOR。
- 已发布的版本号和 tag 不覆盖、不复用。

## 3. 发布流程

1. 确认 `main` 干净且 CI/本地检查通过。
2. 根据自上次 tag 以来的提交，决定下一个版本号：
   - 有 `BREAKING CHANGE` / `!` → 按上文规则提升 MAJOR 或 MINOR；
   - 有 `feat` → MINOR；
   - 只有 `fix` / `perf` → PATCH。
3. 更新 `Cargo.toml` 的 `version`；运行 `cargo check` 使 `Cargo.lock` 同步。
4. 更新 `CHANGELOG.md`：
   - 把 `[Unreleased]` 内容整理到新的 `[X.Y.Z] - YYYY-MM-DD`；
   - 保留新的空 `[Unreleased]`；
   - 按 Added / Changed / Fixed / Removed / Security 分类。
5. 提交发布 commit：

```bash
git add Cargo.toml Cargo.lock CHANGELOG.md
git commit -m "chore(release): vX.Y.Z"
```

6. 创建**附注 tag**：

```bash
git tag -a vX.Y.Z -m "vX.Y.Z"
```

7. 推送提交和 tag：

```bash
git push origin main
git push origin vX.Y.Z
```

8. 在 GitHub Release 中引用 `CHANGELOG.md` 对应段落。

## 4. 安全要求

- 永远不提交 `.env`、数据库密码、token、注册/管理密钥、代理凭据或任何个人凭证。
- 提交前确认 `git status` 与暂存区；`.env` 必须在 `.gitignore` 中。
- 文档与测试默认值使用占位符，不写入真实凭据。
- 如误提交敏感信息：立即吊销/轮换对应凭证，再清理历史；仅删除文件不够。
