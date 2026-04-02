# RFC-0005: 产品化、可观测性与发布能力补齐

## 元信息

| 字段 | 值 |
| --- | --- |
| 文档状态 | `Proposed` |
| 实施状态 | `Not Started` |
| 提出顺序 | `5` |
| 优先级 | `P2` |
| 依赖 | `RFC-0001` |
| 最后更新 | `2026-04-01` |

## 摘要

补齐 `nanobot-rs` 的诊断、自检、事件、指标和发布分发能力，让项目从“架构上完整”升级为“安装、定位问题、发布升级都更像正式产品”。

## 当前不足

### 可观测性弱

- 目前主要依赖日志
- 缺少指标、trace、健康状态、任务统计
- CLI 没有一个真正用于排障的入口

### 安装与发布链路不完整

- 二进制命名和 release workflow 存在不一致
- 分发目标过少
- 缺少统一安装说明、校验和和包管理支持

### 文档与诊断入口不足

- 缺少 config reference
- 缺少“为什么当前不可用”的系统级解释
- 缺少对 provider、MCP、channel 的统一诊断视图

## 目标

- 让用户快速判断系统是否可用、哪里出错、该怎么修
- 让维护者看到关键运行指标
- 让安装和升级路径更清晰、更一致

## 方案

## 1. 统一运行事件模型

将以下行为统一建模为事件：

- provider 请求开始 / 结束 / 失败
- tool 调用开始 / 结束 / 失败
- session compact 开始 / 结束
- subagent 创建 / 完成 / 取消
- cron 触发 / 执行结果
- channel 消息收发

这些事件同时支撑：

- CLI 实时输出
- 指标统计
- trace 导出
- 任务和状态查询

## 2. 增加基础指标与健康视图

建议新增：

- provider 请求总数、失败率、平均耗时
- tool 调用次数、失败率、审批次数
- session 数量、compact 次数
- subagent 活跃数、成功率
- cron job 触发和失败统计

同时提供：

- `status --verbose`
- `doctor`
- `tasks inspect`

## 3. Provider / MCP / Channel 诊断

新增统一诊断框架：

- provider 凭证检查
- provider API reachability
- MCP server 配置和握手结果
- channel 配置是否完整
- workspace 权限和目录存在性

输出要求：

- 明确指出失败原因
- 明确指出建议操作
- 不使用静默降级掩盖问题

## 4. 发布与命名统一

建议先统一：

- 二进制名
- 文档中的调用示例
- release workflow 产物名
- `cargo install` 指引

之后再补：

- macOS / Linux / Windows 多平台构建
- checksums
- 安装脚本
- Homebrew / Scoop / Nix 等分发

## 5. 文档产品化

建议补齐：

- config reference
- provider capability matrix
- channel setup matrix
- troubleshooting guide

这些文档只描述当前真实能力和排障方法。

## 6. 用户信任机制

建议补齐最小版信任机制：

- 首次运行时说明工具可读取 / 写入 / 执行的范围
- 明确展示当前信任模式
- 对未批准的动态工具或危险配置给出高可见提示

## 实施阶段

| 阶段 | 范围 | 状态 | 说明 |
| --- | --- | --- | --- |
| 阶段 1 | 统一二进制与发布命名、增强 `status`、基础诊断 | `Planned` | 先修最影响信任的问题 |
| 阶段 2 | 事件统计、关键指标、`doctor`、结构化修复建议 | `Planned` | 打通排障闭环 |
| 阶段 3 | 多平台发布、安装脚本、包管理分发、补齐运维文档 | `Planned` | 提升产品化完成度 |

## 风险与取舍

- 不建议一开始接入重型 observability 平台，先打通本地事件和 CLI 诊断
- 多平台发布会带来 CI 复杂度，应在命名和打包规范统一后推进
- 诊断逻辑必须避免“假健康”

## 验收标准

- 用户能通过 `status` 或 `doctor` 快速定位常见配置问题
- 发布产物、文档和命令名保持一致
- 至少一套基础指标可用于判断 provider、tool、subagent 的运行质量
- 安装与升级路径对新用户清晰可执行
