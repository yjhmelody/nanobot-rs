# Agent 多模式架构设计

**文档版本**: 1.0  
**创建日期**: 2026-03-07  
**作者**: nanobot-rs 开发团队

---

## 目录

1. [设计理念](#1-设计理念)
2. [核心架构](#2-核心架构)
3. [模式系统](#3-模式系统)
4. [实施计划](#4-实施计划)

---

## 1. 设计理念

### 1.1 问题

当前实现的问题：
- 单一模式硬编码在 `AgentLoop` 中
- 添加新模式需要修改核心代码
- 无法动态切换模式
- 无法组合多种模式

### 1.2 目标

设计一个**可扩展的多模式架构**：
- ✅ 支持多种执行模式（ReAct, Plan-Execute, Reflexion 等）
- ✅ 模式可插拔，易于扩展
- ✅ 支持运行时切换模式
- ✅ 支持模式组合（如 ReAct + Reflexion）
- ✅ 向后兼容现有代码

### 1.3 设计原则

**1. 策略模式 (Strategy Pattern)**
- 每种模式是一个独立的策略
- 通过统一接口调用

**2. 组合优于继承**
- 模式可以组合使用
- 避免复杂的继承层次

**3. 配置驱动**
- 通过配置选择模式
- 支持运行时切换

**4. 渐进式增强**
- 保持 Simple 模式作为默认
- 新模式作为可选增强

---

## 2. 核心架构

### 2.1 整体设计

```rust
// src/agent/executor.rs

/// Agent 执行器 - 核心抽象
#[async_trait]
pub trait AgentExecutor: Send + Sync {
    /// 执行一轮对话
    async fn execute(
        &self,
        context: ExecutionContext,
    ) -> Result<ExecutionResult>;
    
    /// 执行器名称
    fn name(&self) -> &str;
    
    /// 是否支持流式输出
    fn supports_streaming(&self) -> bool {
        false
    }
}

/// 执行上下文
pub struct ExecutionContext {
    /// 消息历史
    pub messages: Vec<ChatMessage>,
    /// 工具上下文
    pub tool_context: ToolContext,
    /// LLM Provider
    pub provider: Arc<dyn LLMProvider>,
    /// 工具注册表
    pub tools: Arc<ToolRegistry>,
    /// 配置
    pub config: ExecutorConfig,
}

/// 执行结果
pub struct ExecutionResult {
    /// 最终响应
    pub response: Option<String>,
    /// 更新后的消息历史
    pub messages: Vec<ChatMessage>,
    /// 执行元数据
    pub metadata: ExecutionMetadata,
}

/// 执行元数据
pub struct ExecutionMetadata {
    /// 迭代次数
    pub iterations: usize,
    /// 工具调用次数
    pub tool_calls: usize,
    /// 执行时间
    pub duration: Duration,
    /// 模式特定数据
    pub mode_data: serde_json::Value,
}

/// 执行器配置
pub struct ExecutorConfig {
    pub max_iterations: usize,
    pub temperature: f32,
    pub max_tokens: i32,
    pub model: String,
    pub reasoning_effort: Option<String>,
    /// 模式特定配置
    pub mode_config: serde_json::Value,
}
```

### 2.2 模式注册表

```rust
// src/agent/registry.rs

/// 执行器注册表
pub struct ExecutorRegistry {
    executors: HashMap<String, Arc<dyn AgentExecutor>>,
    default: String,
}

impl ExecutorRegistry {
    pub fn new() -> Self {
        let mut registry = Self {
            executors: HashMap::new(),
            default: "simple".to_string(),
        };
        
        // 注册内置执行器
        registry.register(Arc::new(SimpleExecutor::new()));
        registry.register(Arc::new(ReActExecutor::new()));
        // 未来添加更多...
        
        registry
    }
    
    pub fn register(&mut self, executor: Arc<dyn AgentExecutor>) {
        self.executors.insert(executor.name().to_string(), executor);
    }
    
    pub fn get(&self, name: &str) -> Option<Arc<dyn AgentExecutor>> {
        self.executors.get(name).cloned()
    }
    
    pub fn get_default(&self) -> Arc<dyn AgentExecutor> {
        self.get(&self.default).expect("default executor must exist")
    }
    
    pub fn list(&self) -> Vec<String> {
        self.executors.keys().cloned().collect()
    }
}
```

### 2.3 AgentLoop 重构

```rust
// src/agent/loop_core.rs

pub struct AgentLoop {
    // 现有字段...
    pub provider: Arc<dyn LLMProvider>,
    pub tools: Arc<ToolRegistry>,
    pub sessions: Arc<SessionManager>,
    
    // 新增：执行器注册表
    pub executors: Arc<ExecutorRegistry>,
    
    // 新增：默认执行器名称
    pub default_executor: String,
}

impl AgentLoop {
    async fn run_agent_loop(
        &self,
        messages: Vec<ChatMessage>,
        tool_context: &ToolContext,
    ) -> (Option<String>, Vec<ChatMessage>) {
        // 从配置或会话中获取执行器名称
        let executor_name = self.get_executor_name(&tool_context.session_key);
        
        // 获取执行器
        let executor = self.executors
            .get(&executor_name)
            .unwrap_or_else(|| self.executors.get_default());
        
        // 构建执行上下文
        let context = ExecutionContext {
            messages,
            tool_context: tool_context.clone(),
            provider: self.provider.clone(),
            tools: self.tools.clone(),
            config: ExecutorConfig {
                max_iterations: self.max_iterations,
                temperature: self.temperature,
                max_tokens: self.max_tokens,
                model: self.model.clone(),
                reasoning_effort: self.reasoning_effort.clone(),
                mode_config: self.get_mode_config(&executor_name),
            },
        };
        
        // 执行
        match executor.execute(context).await {
            Ok(result) => (result.response, result.messages),
            Err(e) => {
                error!(target: TARGET_AGENT, "Executor error: {}", e);
                (Some(format!("Execution error: {}", e)), messages)
            }
        }
    }
    
    fn get_executor_name(&self, session_key: &str) -> String {
        // 1. 检查会话级配置
        if let Some(name) = self.sessions.get_executor(session_key) {
            return name;
        }
        
        // 2. 使用默认配置
        self.default_executor.clone()
    }
    
    fn get_mode_config(&self, executor_name: &str) -> serde_json::Value {
        // 从配置中读取模式特定配置
        serde_json::Value::Null
    }
}
```

---

## 3. 模式系统

### 3.1 Simple 模式（当前）

```rust
// src/agent/executors/simple.rs

pub struct SimpleExecutor;

#[async_trait]
impl AgentExecutor for SimpleExecutor {
    async fn execute(&self, ctx: ExecutionContext) -> Result<ExecutionResult> {
        let mut messages = ctx.messages;
        let mut iterations = 0;
        let mut tool_calls = 0;
        let start = Instant::now();
        
        for _ in 0..ctx.config.max_iterations {
            iterations += 1;
            
            let response = ctx.provider.chat(ChatRequest {
                messages: messages.clone(),
                tools: Some(ctx.tools.definitions()),
                model: Some(ctx.config.model.clone()),
                max_tokens: ctx.config.max_tokens,
                temperature: ctx.config.temperature,
                reasoning_effort: ctx.config.reasoning_effort.clone(),
            }).await;
            
            if response.has_tool_calls() {
                tool_calls += response.tool_calls.len();
                
                // 添加助手消息
                messages.push(ChatMessage::assistant_with_tools(
                    response.content,
                    response.tool_calls.clone(),
                ));
                
                // 执行工具
                for call in response.tool_calls {
                    let result = ctx.tools
                        .execute(&call.name, &call.arguments_json, &ctx.tool_context)
                        .await
                        .unwrap_or_else(|e| format_tool_error(&e));
                    
                    messages.push(ChatMessage::tool_result(
                        &call.id,
                        &call.name,
                        &result,
                    ));
                }
                continue;
            }
            
            // 无工具调用，返回
            messages.push(ChatMessage::assistant_text(response.content.clone()));
            
            return Ok(ExecutionResult {
                response: response.content,
                messages,
                metadata: ExecutionMetadata {
                    iterations,
                    tool_calls,
                    duration: start.elapsed(),
                    mode_data: serde_json::json!({
                        "mode": "simple"
                    }),
                },
            });
        }
        
        // 达到最大迭代
        Ok(ExecutionResult {
            response: Some(format!(
                "Reached max iterations ({})",
                ctx.config.max_iterations
            )),
            messages,
            metadata: ExecutionMetadata {
                iterations,
                tool_calls,
                duration: start.elapsed(),
                mode_data: serde_json::json!({
                    "mode": "simple",
                    "max_iterations_reached": true
                }),
            },
        })
    }
    
    fn name(&self) -> &str {
        "simple"
    }
}
```

### 3.2 ReAct 模式

```rust
// src/agent/executors/react.rs

pub struct ReActExecutor {
    parser: ReActParser,
}

#[async_trait]
impl AgentExecutor for ReActExecutor {
    async fn execute(&self, ctx: ExecutionContext) -> Result<ExecutionResult> {
        let mut messages = ctx.messages;
        let mut iterations = 0;
        let mut tool_calls = 0;
        let start = Instant::now();
        
        // 添加 ReAct 系统提示
        messages.insert(0, ChatMessage::system_text(
            self.build_react_prompt(&ctx.tools)
        ));
        
        for _ in 0..ctx.config.max_iterations {
            iterations += 1;
            
            let response = ctx.provider.chat(ChatRequest {
                messages: messages.clone(),
                tools: None, // ReAct 不使用 function calling
                model: Some(ctx.config.model.clone()),
                max_tokens: ctx.config.max_tokens,
                temperature: ctx.config.temperature,
                reasoning_effort: ctx.config.reasoning_effort.clone(),
            }).await;
            
            let step = self.parser.parse(&response.content.unwrap_or_default())?;
            
            // 记录 Thought
            if let Some(thought) = &step.thought {
                info!(target: TARGET_AGENT, "💭 Thought: {}", thought);
            }
            
            // 执行 Action
            if let Some(action) = &step.action {
                tool_calls += 1;
                info!(target: TARGET_AGENT, "🔧 Action: {}({})", 
                      action.name, action.arguments_json);
                
                let result = ctx.tools
                    .execute(&action.name, &action.arguments_json, &ctx.tool_context)
                    .await
                    .unwrap_or_else(|e| format_tool_error(&e));
                
                messages.push(ChatMessage::assistant_text(
                    format!("Thought: {}\nAction: {}\nAction Input: {}",
                            step.thought.unwrap_or_default(),
                            action.name,
                            action.arguments_json)
                ));
                
                messages.push(ChatMessage::user_text(
                    format!("Observation: {}", result)
                ));
                
                continue;
            }
            
            // Final Answer
            if let Some(answer) = step.answer {
                return Ok(ExecutionResult {
                    response: Some(answer),
                    messages,
                    metadata: ExecutionMetadata {
                        iterations,
                        tool_calls,
                        duration: start.elapsed(),
                        mode_data: serde_json::json!({
                            "mode": "react"
                        }),
                    },
                });
            }
            
            // 格式错误，提示重试
            messages.push(ChatMessage::user_text(
                "Please follow the format: Thought → Action → Observation → ... → Final Answer"
            ));
        }
        
        Ok(ExecutionResult {
            response: Some(format!("Max iterations reached")),
            messages,
            metadata: ExecutionMetadata {
                iterations,
                tool_calls,
                duration: start.elapsed(),
                mode_data: serde_json::json!({
                    "mode": "react",
                    "max_iterations_reached": true
                }),
            },
        })
    }
    
    fn name(&self) -> &str {
        "react"
    }
}
```

### 3.3 Plan-Execute 模式

```rust
// src/agent/executors/plan_execute.rs

pub struct PlanExecuteExecutor {
    planner: Arc<dyn Planner>,
    executor: Arc<SimpleExecutor>,
}

#[async_trait]
impl AgentExecutor for PlanExecuteExecutor {
    async fn execute(&self, ctx: ExecutionContext) -> Result<ExecutionResult> {
        let start = Instant::now();
        
        // 1. Planning Phase
        let plan = self.planner.plan(&ctx).await?;
        info!(target: TARGET_AGENT, "📋 Plan: {} steps", plan.steps.len());
        
        let mut messages = ctx.messages;
        let mut total_tool_calls = 0;
        
        // 2. Execution Phase
        for (idx, step) in plan.steps.iter().enumerate() {
            info!(target: TARGET_AGENT, "▶️  Step {}/{}: {}", 
                  idx + 1, plan.steps.len(), step.description);
            
            // 为每个步骤创建子上下文
            let step_ctx = ExecutionContext {
                messages: messages.clone(),
                tool_context: ctx.tool_context.clone(),
                provider: ctx.provider.clone(),
                tools: ctx.tools.clone(),
                config: ctx.config.clone(),
            };
            
            // 执行步骤
            let result = self.executor.execute(step_ctx).await?;
            messages = result.messages;
            total_tool_calls += result.metadata.tool_calls;
            
            // 检查是否需要重新规划
            if self.should_replan(&result) {
                info!(target: TARGET_AGENT, "🔄 Replanning...");
                // TODO: 实现重新规划逻辑
            }
        }
        
        Ok(ExecutionResult {
            response: messages.last()
                .and_then(|m| m.content.as_ref())
                .and_then(|c| c.as_text())
                .map(|s| s.to_string()),
            messages,
            metadata: ExecutionMetadata {
                iterations: plan.steps.len(),
                tool_calls: total_tool_calls,
                duration: start.elapsed(),
                mode_data: serde_json::json!({
                    "mode": "plan_execute",
                    "plan": plan
                }),
            },
        })
    }
    
    fn name(&self) -> &str {
        "plan_execute"
    }
}
```

### 3.4 Reflexion 模式

```rust
// src/agent/executors/reflexion.rs

pub struct ReflexionExecutor {
    actor: Arc<SimpleExecutor>,
    evaluator: Arc<dyn Evaluator>,
    max_retries: usize,
}

#[async_trait]
impl AgentExecutor for ReflexionExecutor {
    async fn execute(&self, ctx: ExecutionContext) -> Result<ExecutionResult> {
        let start = Instant::now();
        let mut messages = ctx.messages;
        let mut reflections = Vec::new();
        let mut total_tool_calls = 0;
        
        for attempt in 0..self.max_retries {
            info!(target: TARGET_AGENT, "🎯 Attempt {}/{}", attempt + 1, self.max_retries);
            
            // 添加之前的反思
            if !reflections.is_empty() {
                let reflection_text = reflections.join("\n\n");
                messages.push(ChatMessage::user_text(
                    format!("Previous attempts and reflections:\n{}", reflection_text)
                ));
            }
            
            // 执行任务
            let result = self.actor.execute(ExecutionContext {
                messages: messages.clone(),
                ..ctx.clone()
            }).await?;
            
            total_tool_calls += result.metadata.tool_calls;
            
            // 评估结果
            let evaluation = self.evaluator.evaluate(&ctx, &result).await?;
            
            if evaluation.success {
                info!(target: TARGET_AGENT, "✅ Success! Score: {}", evaluation.score);
                return Ok(ExecutionResult {
                    response: result.response,
                    messages: result.messages,
                    metadata: ExecutionMetadata {
                        iterations: attempt + 1,
                        tool_calls: total_tool_calls,
                        duration: start.elapsed(),
                        mode_data: serde_json::json!({
                            "mode": "reflexion",
                            "attempts": attempt + 1,
                            "score": evaluation.score,
                            "reflections": reflections
                        }),
                    },
                });
            }
            
            // 生成反思
            info!(target: TARGET_AGENT, "❌ Failed. Score: {}. Reflecting...", evaluation.score);
            let reflection = self.generate_reflection(&evaluation).await?;
            reflections.push(reflection);
            
            messages = result.messages;
        }
        
        Ok(ExecutionResult {
            response: Some(format!(
                "Failed after {} attempts. Last feedback: {}",
                self.max_retries,
                reflections.last().unwrap_or(&String::new())
            )),
            messages,
            metadata: ExecutionMetadata {
                iterations: self.max_retries,
                tool_calls: total_tool_calls,
                duration: start.elapsed(),
                mode_data: serde_json::json!({
                    "mode": "reflexion",
                    "max_retries_reached": true,
                    "reflections": reflections
                }),
            },
        })
    }
    
    fn name(&self) -> &str {
        "reflexion"
    }
}
```

### 3.5 组合模式

```rust
// src/agent/executors/composite.rs

/// 组合执行器 - 支持模式组合
pub struct CompositeExecutor {
    executors: Vec<Arc<dyn AgentExecutor>>,
    strategy: CompositionStrategy,
}

pub enum CompositionStrategy {
    /// 顺序执行：A → B → C
    Sequential,
    /// 条件执行：根据结果选择下一个
    Conditional(Box<dyn Fn(&ExecutionResult) -> Option<usize>>),
    /// 并行执行：同时执行多个，合并结果
    Parallel,
}

#[async_trait]
impl AgentExecutor for CompositeExecutor {
    async fn execute(&self, ctx: ExecutionContext) -> Result<ExecutionResult> {
        match &self.strategy {
            CompositionStrategy::Sequential => {
                self.execute_sequential(ctx).await
            }
            CompositionStrategy::Conditional(selector) => {
                self.execute_conditional(ctx, selector).await
            }
            CompositionStrategy::Parallel => {
                self.execute_parallel(ctx).await
            }
        }
    }
    
    fn name(&self) -> &str {
        "composite"
    }
}

// 示例：ReAct + Reflexion 组合
pub fn create_react_reflexion_executor() -> Arc<dyn AgentExecutor> {
    Arc::new(CompositeExecutor {
        executors: vec![
            Arc::new(ReActExecutor::new()),
            Arc::new(ReflexionExecutor::new()),
        ],
        strategy: CompositionStrategy::Sequential,
    })
}
```

---

## 4. 实施计划

### 4.1 Phase 1: 核心架构（1 周）

**目标**: 建立多模式基础架构

**任务**:
1. ✅ 定义 `AgentExecutor` trait
2. ✅ 实现 `ExecutorRegistry`
3. ✅ 重构 `AgentLoop` 使用执行器
4. ✅ 迁移现有代码到 `SimpleExecutor`
5. ✅ 添加配置支持
6. ✅ 编写测试

**交付物**:
- `src/agent/executor.rs` - 核心 trait
- `src/agent/registry.rs` - 注册表
- `src/agent/executors/simple.rs` - Simple 模式
- 测试覆盖率 > 80%

### 4.2 Phase 2: ReAct 模式（1 周）

**目标**: 实现第一个新模式

**任务**:
1. ✅ 实现 `ReActExecutor`
2. ✅ 实现 `ReActParser`
3. ✅ 添加提示模板
4. ✅ 注册到 `ExecutorRegistry`
5. ✅ 编写测试
6. ✅ 更新文档

**交付物**:
- `src/agent/executors/react.rs`
- `docs/REACT_MODE.md`

### 4.3 Phase 3: 工具并行执行（1 周）

**目标**: 优化性能

**任务**:
1. ✅ 实现依赖分析
2. ✅ 修改 `SimpleExecutor` 支持并行
3. ✅ 添加配置开关
4. ✅ 性能测试

**交付物**:
- `src/tools/dependency.rs`
- 性能提升 30-50%

### 4.4 Phase 4: Plan-Execute 模式（2 周）

**目标**: 支持复杂任务

**任务**:
1. ✅ 定义 `Planner` trait
2. ✅ 实现 `LLMPlanner`
3. ✅ 实现 `PlanExecuteExecutor`
4. ✅ 编写测试

**交付物**:
- `src/agent/executors/plan_execute.rs`
- `src/agent/planner.rs`

### 4.5 Phase 5: Reflexion 模式（2 周）

**目标**: 自我反思能力

**任务**:
1. ✅ 定义 `Evaluator` trait
2. ✅ 实现 `LLMEvaluator`
3. ✅ 实现 `ReflexionExecutor`
4. ✅ 编写测试

**交付物**:
- `src/agent/executors/reflexion.rs`
- `src/agent/evaluator.rs`

### 4.6 Phase 6: 组合模式（1 周）

**目标**: 支持模式组合

**任务**:
1. ✅ 实现 `CompositeExecutor`
2. ✅ 添加组合策略
3. ✅ 创建预设组合
4. ✅ 编写测试

**交付物**:
- `src/agent/executors/composite.rs`
- 预设组合（ReAct+Reflexion 等）

---

## 5. 配置示例

### 5.1 基础配置

```json
{
  "agents": {
    "defaults": {
      "executor": "simple",
      "maxToolIterations": 40,
      "executors": {
        "simple": {
          "parallel_tools": true
        },
        "react": {
          "show_thoughts": true
        },
        "plan_execute": {
          "max_plan_steps": 10,
          "allow_replan": true
        },
        "reflexion": {
          "max_retries": 3,
          "min_score": 0.7
        }
      }
    }
  }
}
```

### 5.2 会话级配置

```bash
# 使用 ReAct 模式
nanobot-rs agent -m "分析代码" --executor react

# 使用 Plan-Execute 模式
nanobot-rs agent -m "创建 Web 服务" --executor plan_execute

# 使用组合模式
nanobot-rs agent -m "复杂任务" --executor react+reflexion
```

### 5.3 动态切换

```rust
// 在对话中切换模式
session.set_executor("react");

// 为特定任务临时切换
let result = agent.execute_with_mode("plan_execute", task).await;
```

---

## 6. 优势总结

### 6.1 可扩展性

- ✅ 添加新模式只需实现 `AgentExecutor` trait
- ✅ 无需修改核心代码
- ✅ 支持第三方模式插件

### 6.2 灵活性

- ✅ 运行时切换模式
- ✅ 会话级配置
- ✅ 模式组合

### 6.3 向后兼容

- ✅ 默认使用 Simple 模式
- ✅ 现有代码无需修改
- ✅ 渐进式迁移

### 6.4 可测试性

- ✅ 每个模式独立测试
- ✅ Mock 执行器
- ✅ 集成测试简单

---

**下一步**: 开始实施 Phase 1 - 核心架构
