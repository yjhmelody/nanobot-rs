# CLI 命令参考

本文档描述 `nanobot` 当前命令行接口（以源码实现为准）。

## 1. 总览

```bash
nanobot <subcommand>
```

子命令：

- `onboard`
- `agent`
- `gateway`
- `status`
- `provider`

## 2. onboard

初始化或刷新配置与工作区模板。

```bash
nanobot onboard
nanobot onboard --overwrite
```

行为：

- 创建或刷新 `~/.nanobot/config.json`
- 初始化 workspace（默认 `~/.nanobot/workspace`）
- 同步模板文件

`--overwrite` 会重置为默认配置；不加该参数时会保留已有字段并重写为规范格式。

## 3. agent

运行 Agent（单轮或交互模式）。

### 单轮模式

```bash
nanobot agent -m "你好"
nanobot agent -s cli:project-a -m "继续上次任务"
```

参数：

- `-m, --message <TEXT>`：单轮输入，执行后退出
- `-s, --session <CHANNEL:CHAT_ID>`：会话键，默认 `cli:direct`

### 交互模式

```bash
nanobot agent
nanobot agent -s cli:direct
```

交互退出命令：

- `exit`
- `quit`
- `/exit`
- `/quit`
- `:q`

## 4. gateway

启动长驻运行模式（agent loop + channels + cron + heartbeat）。

```bash
nanobot gateway
```

说明：当前实现会启动长驻流程，但并未暴露独立 HTTP API 监听逻辑。`--port` 作为保留参数存在，当前不生效。

## 5. status

输出本地状态摘要：

```bash
nanobot status
```

包含：

- config 路径与存在性
- workspace 路径与存在性
- 当前默认模型
- 推断出的 provider 名称

## 6. provider

当前仅用于 `github_copilot` 的登录与状态检查。

### login

```bash
nanobot provider login github_copilot
nanobot provider login github_copilot --host <HOST> --config-dir <DIR>
```

### status

```bash
nanobot provider status github_copilot
nanobot provider status github_copilot --config-dir <DIR>
```

说明：

- 对非 `github_copilot` 的 provider，`provider status` 不做同等状态探测。
- 实际调用命令名可由 `acp.agents.copilot.command` 覆盖，默认是 `copilot`。

## 7. 聊天内建命令（通过消息内容触发）

这些命令不是 CLI 子命令，而是聊天消息中的控制指令：

- `/help` — 显示可用命令列表
- `/cancel` — 优雅打断当前正在执行的 ReAct 循环，清空消息队列，已完成的 tool call 回合会保存到对话历史
- `/stop` — 暴力中止当前会话的所有任务，不保存中间状态
- `/new` — 清空当前会话历史，开始新对话
- `/compact` — 手动触发会话压缩，合并早期消息为摘要

它们由 `InboundCommand` 解析并在 `AgentLoop` 内部处理。
