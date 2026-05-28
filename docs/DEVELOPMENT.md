# 开发与测试

本文面向贡献者，描述当前推荐的开发流程。

## 常用命令

仓库使用 `justfile` 管理常见流程：

```bash
just                 # 查看命令列表
just fmt             # 格式化
just fmt-check       # 检查格式
just lint            # clippy + taplo
just check           # cargo check
just test            # cargo test --all-targets --all-features
just e2e             # 离线端到端测试
just e2e-codex       # 离线端到端测试（含 codex MCP connect smoke）
just ci              # 本地 CI 流程
just build-release   # 本地构建 release 二进制
just changelog vX.Y.Z        # 为版本生成 CHANGELOG 条目
just changelog-preview vX.Y.Z # 预览 changelog 条目（不落盘）
```

也可以直接使用 Cargo：

```bash
cargo test
cargo test -p nanobot --test e2e_local -- --nocapture
cargo run -- agent -m "hi"
```

## 本地 E2E

离线 E2E 的目标是覆盖一条真实运行链路：

```text
CLI -> runtime -> provider -> tool execution -> session persistence
```

运行方式：

```bash
just e2e
```

当前离线 E2E 会验证：

- `onboard`
- `status`
- `agent -m ...`
- Provider 请求链路
- 工具执行
- 会话落盘

并覆盖两条协议路径：

- `custom` + `Responses API`
- `anthropic` + `Messages API`

## 可选测试

部分测试依赖本地二进制或真实认证信息，因此默认忽略：

- ACP 相关 smoke tests
- Codex MCP connect smoke test

需要时可以手动执行：

```bash
cargo test -- --ignored
```

或运行特定测试：

```bash
cargo test -p nanobot --test e2e_local codex_mcp_connect_smoke -- --ignored --nocapture
```

## 调试建议

### 开启日志

```bash
RUST_LOG=debug cargo run -- agent -m "hello"
RUST_LOG=nanobot::agent=trace cargo run -- agent -m "hello"
# 每个特点概念的组件都有对应的日志 target，如 agent、provider、tool、session 等，可以按需开启更细粒度的日志输出。
```

### 常用排查点

- `~/.nanobot/config.json`：配置是否正确
- `~/.nanobot/workspace/`：模板、记忆、会话文件是否生成
- Provider API base 是否与协议匹配：
  - Anthropic -> `messages`
  - OpenAI / custom -> `responses`（默认）或 `chat/completions`（`wireApi=chat_completions`）

## GitHub Actions

仓库当前包含两条 workflow：

- `CI`：在 `Linux / macOS / Windows` 上执行 `fmt + clippy + test`
- `Release`：在 `Linux / macOS / Windows` 上构建发行包，并在 tag push 时创建 GitHub Release

## 发版 changelog 维护

仓库提供发布 changelog 自动生成脚本与 skill：

- 脚本：`scripts/generate-changelog.sh`
- skill：`skills/release-changelog/SKILL.md`

推荐流程：

```bash
just changelog-preview v0.0.4
just changelog v0.0.4
git add CHANGELOG.md
git commit -m "Update changelog for v0.0.4"
git tag v0.0.4
git push origin main --tags
```

> 已固化约束：`pre-push` 会检查本次 push 的 tag（`refs/tags/*`）是否在 `CHANGELOG.md` 中存在对应条目（`## [vX.Y.Z] - ...`）。未命中时会直接拦截 push，并提示运行 `just changelog <tag>`。

建议在提交前至少本地执行：

```bash
just ci
just e2e
```

其中 `just e2e` 对应 workflow 里的 Linux 离线端到端校验，用于覆盖完整运行链路，而不是只做单元测试。

## 项目级 Git Hooks（适配 Codex / Claude / Copilot）

本仓库提供版本化 hooks（`.githooks/`）：

- `pre-commit`：调用 `scripts/quality-gate.sh pre-commit`
- `pre-push`：调用 `scripts/quality-gate.sh pre-push`

实际检查逻辑统一定义在：

- `scripts/quality-gate.sh`
- `scripts/verify-tag-changelog.sh`（tag 推送前 changelog 校验）
- `just hook-commit`
- `just hook-push`

启用方式（执行一次）：

```bash
bash scripts/setup-git-hooks.sh
```

说明：

- 这些是 **Git 项目级 hooks**，对通过 `git commit` / `git push` 的 Codex、Claude、Copilot 工作流都生效。
- 如需临时跳过（不建议常用）：`NANOBOT_SKIP_HOOKS=1 git commit ...`

## Claude 原生 Hooks（项目级）

仓库已提供 `.claude/settings.json`，在 Claude Code 中会在以下场景自动触发：

- `git commit ...` 前：调用 `scripts/quality-gate.sh pre-commit --claude`
- `git push ...` 前：调用 `scripts/quality-gate.sh pre-push --claude`

当检查失败时，Claude Hook 会拒绝该工具调用（阻止 commit/push）。

## Codex / Copilot 说明

- **Codex**：提供 `.codex/hooks/pre-commit.sh` 和 `.codex/hooks/pre-push.sh`，同样走 `scripts/quality-gate.sh`。
- **Copilot**：仓库已新增 `.github/copilot-instructions.md`，要求在 commit/push 前执行同等检查。

## 文档维护原则

本仓库未来只维护三份核心文档：

- `docs/QUICK_START.md`
- `docs/ARCHITECTURE.md`
- `docs/DEVELOPMENT.md`

更新规则：

- 行为变化先改文档，再补代码
- 不再新增“实现总结 / 修复纪要 / 阶段报告”类文档
- 代码细节以源码与测试为准
