# RFC-0002: CLI 与工作流升级

## 元信息

| 字段 | 值 |
| --- | --- |
| 文档状态 | `Proposed` |
| 实施状态 | `Not Started` |
| 提出顺序 | `2` |
| 优先级 | `P0` |
| 依赖 | `RFC-0001` |
| 最后更新 | `2026-04-01` |

## 摘要

把 `nanobot-rs` 从“可运行的 agent runtime”升级成“可持续使用的终端助手”。核心是补齐会话、恢复、规划、后台任务和诊断入口，并建立统一的状态模型，支撑后续更丰富的交互体验。

## 当前不足

### 命令面太薄

- 缺少 `sessions`、`resume`、`tasks`、`plan` 这类一眼能看懂的工作流命令
- `provider` 命令偏单一，不利于扩展不同 provider 的诊断能力

### 状态不可见

- `status` 只展示少量静态配置
- 看不到当前 session 列表、最近活动、后台任务、MCP 状态和 channel 健康状态

### 交互缺乏阶段感

- 交互模式仍以基础读入/打印为主
- 没有明确的“当前在计划、执行、等待审批、后台处理中”反馈

### 缺少恢复能力

- 底层有 session 持久化，但 CLI 没有把“恢复工作”做成一等能力

## 目标

- 用最少的新交互建立完整工作流心智
- 复用现有 session、message bus、agent loop
- 先做命令和状态模型，再决定是否引入 richer TUI

## 方案

## 1. 扩展命令体系

### `nanobot sessions`

- `sessions list`
- `sessions show <session-id>`
- `sessions delete <session-id>`
- `sessions compact <session-id>`

### `nanobot resume`

- `resume`
- `resume --session <id>`
- `resume --all`

### `nanobot plan`

- `plan start [topic]`
- `plan show`
- `plan approve`
- `plan discard`

规划模式下默认禁用写工具和高风险 shell。

### `nanobot tasks`

- `tasks list`
- `tasks show <task-id>`
- `tasks cancel <task-id>`

### `nanobot doctor`

- 检查配置、provider 凭证、workspace、MCP server、channel 配置

## 2. 统一状态模型

建议新增共享状态对象：

- `RuntimeStatus`
- `SessionSummary`
- `TaskSummary`
- `ProviderHealth`
- `ToolPolicyState`

这层只负责“可展示状态”，不直接持有复杂业务逻辑。

## 3. 交互式 CLI 升级

第一轮不做重型全屏界面，先补：

- 历史记录
- 多行输入
- slash commands 自动补全
- 系统事件的可读化展示
- 当前 intent / step / active task 提示

## 4. Plan Mode

### 行为约束

- 默认只允许只读工具
- shell 仅允许低风险读操作，或直接禁用
- 明确显示“当前为规划模式”
- 退出规划模式时要求显式确认

### 数据模型

- `SessionMode`
- `PlanState`
- `PlanApprovalState`

## 5. 事件与进度展示

建议统一以下事件：

- `IntentChanged`
- `ToolQueued`
- `ToolStarted`
- `ToolFinished`
- `TaskSpawned`
- `TaskCompleted`
- `ApprovalRequested`

CLI 层负责将这些事件渲染为人类可读输出。

## 实施阶段

| 阶段 | 范围 | 状态 | 说明 |
| --- | --- | --- | --- |
| 阶段 1 | `sessions list/show`、增强 `status`、增加 `resume` | `Planned` | 最小工作流闭环 |
| 阶段 2 | `tasks list/show/cancel`、事件标准化、输入体验升级 | `Planned` | 建立任务感和过程可见性 |
| 阶段 3 | `plan`、`doctor`、planning 模式约束 | `Planned` | 把规划与执行阶段分离 |

## 风险与取舍

- 过早做复杂 TUI 会拖慢核心 workflow 落地
- 只做命令不做状态模型，后续会重复造展示逻辑
- plan mode 过严会影响体验，过松又无法建立信任

## 验收标准

- 用户能列出、查看、恢复历史会话
- 用户能看到后台任务和当前执行阶段
- 用户能显式进入/退出 planning 模式
- `status` 和 `doctor` 足以支持常见排障场景
