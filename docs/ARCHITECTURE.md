# nanobot-rs 架构设计

本文描述 **当前实现** 的稳定结构，而不是历史重构过程。

## 目标

`nanobot-rs` 的目标是提供一个本地优先的 Agent 运行时，支持：

- CLI 直接使用
- Gateway 长驻运行
- 可替换的 LLM provider
- 可扩展的工具体系
- 本地会话、记忆、Cron、心跳与渠道分发

## 系统总览

```text
CLI / Gateway
    │
    ▼
Config + Workspace Bootstrap
    │
    ▼
RuntimeBundle
 ├─ MessageBus
 ├─ Provider
 ├─ AgentLoop
 ├─ CronService
 └─ HeartbeatService
    │
    ▼
AgentLoop
 ├─ ContextBuilder
 ├─ SessionManager
 ├─ Memory / Skills
 ├─ ReActExecutor
 └─ ToolRegistry
```

## 运行入口

当前二进制提供 5 个入口命令：

- `onboard`：初始化 `~/.nanobot/config.json` 和工作区模板
- `agent`：运行单轮或交互式 Agent
- `gateway`：启动长驻运行时、渠道、Cron 和心跳
- `status`：查看当前配置与运行状态摘要
- `provider`：执行 provider 相关辅助命令，当前仅支持 `github_copilot` 登录与状态检查

## 运行时分层

### 1. 配置层

配置文件位于 `~/.nanobot/config.json`，核心分为：

- `agents`：默认模型、provider、tokens、温度、迭代次数、记忆窗口
- `providers`：LLM provider 配置
- `tools`：Web、Shell、MCP、工作区限制
- `channels`：CLI / Telegram / 占位渠道
- `gateway`：监听地址与心跳
- `acp`：ACP 外部 coding agent 配置

### 2. RuntimeBundle

`build_runtime()` 将配置装配成统一运行时：

- `MessageBus`：系统内部消息总线
- `Provider`：模型调用抽象
- `AgentLoop`：主执行循环
- `CronService`：定时任务服务
- `HeartbeatService`：周期性健康/提醒逻辑

这层负责把“配置”变成“可运行组件”，但不关心业务具体执行细节。

### 3. AgentLoop

`AgentLoop` 是核心编排器，负责：

- 构建系统提示词和上下文
- 读取与保存会话
- 调用 ReAct 执行器
- 处理工具调用结果
- 维护子代理与渠道交互

`AgentLoop` 自身不直接实现每个工具或 provider 的细节，而是通过抽象边界组合其他模块。

## ReAct 执行模型

当前 Agent 执行采用 **ReAct** 循环：

1. 根据消息历史与工具定义查询模型
2. 如果模型返回最终答案，则结束
3. 如果模型返回工具调用，则执行一个工具观察步骤
4. 将观察结果追加到消息历史
5. 继续下一轮，直到完成、取消或达到最大迭代次数

ReAct 相关模块拆分在 `src/agent/react/`：

- `planner`：负责向模型发起规划请求
- `tool_runner`：执行工具并构造 observation
- `state`：执行状态与退出原因
- `executor`：协调完整循环

这种拆分让模型调用、工具执行和状态机各自独立，便于维护和扩展。

## Provider 设计

当前 provider 层只维护 **当前有效协议**：

### 1. Anthropic

- 原生使用 `Messages API`
- 端点语义：`/v1/messages`
- 支持文本与工具调用映射

### 2. OpenAI

- 原生使用 `Responses API`
- 端点语义：`/v1/responses`
- 用于 OpenAI 官方与兼容 Responses 协议的服务

### 3. Custom

- 复用 `OpenAICompatProvider`
- 要求目标服务兼容 `Responses API`
- 不再兼容旧的 `chat/completions` 路径

### 4. GitHub Copilot

- 不作为主 LLM provider 注入 AgentLoop
- 当前仅支持：
  - `nanobot-rs provider login github_copilot`
  - `nanobot-rs provider status github_copilot`
- 若要在执行流中调用外部 coding agent，应通过 ACP 工具接入

## 工具系统

内建工具分为两类：

### 核心工具

- 文件系统：`read_file` `write_file` `edit_file` `list_dir`
- Shell：`exec`
- Web：`web_search` `web_fetch`
- 消息：`message`

### 可选工具

- `spawn`：创建子代理任务
- `cron`：管理定时任务

此外还支持两类外部扩展：

- **MCP**：通过 `tools.mcpServers` 注册远程或本地 MCP server
- **ACP**：通过 ACP agent 执行外部 coding 任务

工具注册统一由 `ToolRegistryBuilder` 构建，运行期通过 `ToolRegistry` 执行。

## 消息总线与渠道

`MessageBus` 是运行时内部的发布订阅通道，用于：

- 渠道输入 -> Agent
- Agent 输出 -> 渠道
- 心跳/网关等后台服务协作

当前渠道状态：

- `cli`：内置、可用
- `telegram`：已实现
- `discord`：占位实现
- `feishu`：占位实现

Gateway 模式会启动渠道管理器，并把 outbound message 分发到对应渠道。

## 持久化

### 工作区持久化

工作区默认位于 `~/.nanobot/workspace`，包含：

- `sessions/`：会话 JSONL
- `memory/`：长期记忆与历史
- 模板文件：`AGENTS.md`、`TOOLS.md`、`USER.md` 等

### 应用数据持久化

- `CronService` 使用独立数据目录保存任务定义

## 后台服务

### Heartbeat

- 按配置周期运行
- 与 Agent/Bus 协作完成定时提醒或自检逻辑

### Cron

- 提供周期性任务调度
- 可通过工具接口增删查任务

## 扩展点

当前稳定扩展点如下：

- 新增 LLM provider：实现 `LLMProvider`
- 新增工具：实现 `Tool` 并注册到 `ToolRegistryBuilder`
- 新增渠道：实现 `ChannelAdapter`
- 新增 MCP / ACP：通过配置接入，无需修改 AgentLoop 主流程

## 当前设计边界

为了降低维护成本，当前文档与实现遵循以下边界：

- 只记录当前可运行架构，不记录历史重构过程
- 只维护现行协议：`Anthropic Messages` 与 `OpenAI Responses`
- 不再维护逐文件实现说明与阶段性总结
- 详细代码细节以源码和测试为准
