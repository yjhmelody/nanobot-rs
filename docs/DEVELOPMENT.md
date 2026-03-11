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
just ci              # 本地 CI 流程
```

也可以直接使用 Cargo：

```bash
cargo test
cargo test --test e2e_local -- --nocapture
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
cargo test --test e2e_local codex_mcp_connect_smoke -- --ignored --nocapture
```

## 调试建议

### 开启日志

```bash
RUST_LOG=debug cargo run -- agent -m "hello"
RUST_LOG=nanobot_rs::agent=trace cargo run -- agent -m "hello"
```

### 常用排查点

- `~/.nanobot/config.json`：配置是否正确
- `~/.nanobot/workspace/`：模板、记忆、会话文件是否生成
- Provider API base 是否与协议匹配：
  - Anthropic -> `messages`
  - OpenAI / custom -> `responses`

## 文档维护原则

本仓库未来只维护三份核心文档：

- `docs/QUICK_START.md`
- `docs/ARCHITECTURE.md`
- `docs/DEVELOPMENT.md`

更新规则：

- 行为变化先改文档，再补代码
- 不再新增“实现总结 / 修复纪要 / 阶段报告”类文档
- 代码细节以源码与测试为准
