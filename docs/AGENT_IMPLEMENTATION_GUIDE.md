# Agent 架构改进实施指南

**文档版本**: 1.0  
**创建日期**: 2026-03-07  
**目标读者**: 开发者

---

## 目录

1. [Phase 1: ReAct 模式实现](#phase-1-react-模式实现)
2. [Phase 2: 工具并行执行](#phase-2-工具并行执行)
3. [Phase 3: 语义记忆](#phase-3-语义记忆)
4. [Phase 4: Plan-and-Execute](#phase-4-plan-and-execute)
5. [Phase 5: Reflexion](#phase-5-reflexion)
6. [Phase 6: 多 Agent 协作](#phase-6-多-agent-协作)

---

## Phase 1: ReAct 模式实现

### 1.1 目标

添加显式推理步骤，提升可解释性。

### 1.2 架构设计

```rust
// src/agent/mode.rs
pub enum AgentMode {
    /// 简单循环模式（当前默认）
    Simple,
    /// ReAct 模式：Reasoning + Acting
    ReAct,
    /// Plan-and-Execute 模式（未来）
    PlanExecute,
}

// src/agent/react.rs
pub struct ReActAgent {
    inner: Arc<AgentLoop>,
    parser: ReActParser,
}

pub struct ReActParser {
    thought_pattern: Regex,
    action_pattern: Regex,
    answer_pattern: Regex,
}

pub struct ReActStep {
    pub thought: Option<String>,
    pub action: Option<ToolCall>,
    pub observation: Option<String>,
    pub answer: Option<String>,
}
```

### 1.3 提示模板

```rust
// src/agent/react.rs
const REACT_SYSTEM_PROMPT: &str = r#"
You are a helpful AI assistant that uses tools to accomplish tasks.

You should follow this format:

Thought: [your reasoning about what to do next]
Action: tool_name
Action Input: {"arg": "value"}
Observation: [tool result will be inserted here]
... (repeat Thought/Action/Observation as needed)
Thought: I now have enough information to answer
Final Answer: [your response to the user]

Available tools:
{tools}

Important:
- Always start with a Thought
- Use Action and Action Input together
- Wait for Observation before next Thought
- End with Final Answer when ready
"#;
```

### 1.4 核心实现

```rust
// src/agent/react.rs
impl ReActAgent {
    pub async fn run(
        &self,
        messages: Vec<ChatMessage>,
        tool_context: &ToolContext,
    ) -> Result<(Option<String>, Vec<ChatMessage>)> {
        let mut messages = messages;
        let mut steps = Vec::new();
        
        for iteration in 0..self.inner.max_iterations {
            // 调用 LLM
            let response = self.inner.provider.chat(ChatRequest {
                messages: messages.clone(),
                tools: None, // ReAct 不使用 function calling
                model: Some(self.inner.model.clone()),
                max_tokens: self.inner.max_tokens,
                temperature: self.inner.temperature,
                reasoning_effort: self.inner.reasoning_effort.clone(),
            }).await;
            
            // 解析 ReAct 步骤
            let step = self.parser.parse(&response.content.unwrap_or_default())?;
            
            // 记录 Thought
            if let Some(thought) = &step.thought {
                info!(target: TARGET_AGENT, "Thought: {}", thought);
                messages.push(ChatMessage::assistant_text(
                    format!("Thought: {}", thought)
                ));
            }
            
            // 执行 Action
            if let Some(action) = &step.action {
                info!(target: TARGET_AGENT, "Action: {}({})", 
                      action.name, action.arguments_json);
                
                messages.push(ChatMessage::assistant_text(
                    format!("Action: {}\nAction Input: {}", 
                            action.name, action.arguments_json)
                ));
                
                // 执行工具
                let result = self.inner.tools
                    .execute(&action.name, &action.arguments_json, tool_context)
                    .await
                    .unwrap_or_else(|e| format_tool_error(&e));
                
                // 添加 Observation
                messages.push(ChatMessage::user_text(
                    format!("Observation: {}", result)
                ));
                
                steps.push(step);
                continue;
            }
            
            // 返回 Final Answer
            if let Some(answer) = step.answer {
                return Ok((Some(answer), messages));
            }
            
            // 如果既没有 Action 也没有 Answer，说明格式错误
            warn!(target: TARGET_AGENT, "Invalid ReAct format, retrying...");
            messages.push(ChatMessage::user_text(
                "Please follow the format: Thought → Action → Observation → ... → Final Answer"
            ));
        }
        
        // 达到最大迭代次数
        Ok((Some(format!(
            "Reached maximum iterations ({}). Steps completed: {}",
            self.inner.max_iterations,
            steps.len()
        )), messages))
    }
}

impl ReActParser {
    pub fn new() -> Self {
        Self {
            thought_pattern: Regex::new(r"Thought:\s*(.+?)(?:\n|$)").unwrap(),
            action_pattern: Regex::new(r"Action:\s*(\w+)").unwrap(),
            answer_pattern: Regex::new(r"Final Answer:\s*(.+)").unwrap(),
        }
    }
    
    pub fn parse(&self, text: &str) -> Result<ReActStep> {
        let thought = self.thought_pattern
            .captures(text)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().trim().to_string());
        
        let action = if let Some(action_match) = self.action_pattern.captures(text) {
            let name = action_match.get(1).unwrap().as_str().to_string();
            
            // 提取 Action Input
            let input_pattern = Regex::new(r"Action Input:\s*(\{.+?\})").unwrap();
            let arguments_json = input_pattern
                .captures(text)
                .and_then(|c| c.get(1))
                .map(|m| m.as_str().to_string())
                .unwrap_or_else(|| "{}".to_string());
            
            Some(ToolCall {
                id: uuid::Uuid::new_v4().to_string(),
                name: name.into(),
                arguments_json,
            })
        } else {
            None
        };
        
        let answer = self.answer_pattern
            .captures(text)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().trim().to_string());
        
        Ok(ReActStep {
            thought,
            action,
            observation: None,
            answer,
        })
    }
}
```

### 1.5 集成到 AgentLoop

```rust
// src/agent/loop_core.rs
impl AgentLoop {
    async fn run_agent_loop(
        &self,
        messages: Vec<ChatMessage>,
        tool_context: &ToolContext,
    ) -> (Option<String>, Vec<ChatMessage>) {
        match self.mode {
            AgentMode::Simple => self.run_simple_loop(messages, tool_context).await,
            AgentMode::ReAct => {
                let react = ReActAgent::new(Arc::new(self.clone()));
                react.run(messages, tool_context).await
                    .unwrap_or_else(|e| {
                        (Some(format!("ReAct error: {}", e)), messages)
                    })
            }
            AgentMode::PlanExecute => {
                // 未来实现
                self.run_simple_loop(messages, tool_context).await
            }
        }
    }
    
    async fn run_simple_loop(
        &self,
        mut messages: Vec<ChatMessage>,
        tool_context: &ToolContext,
    ) -> (Option<String>, Vec<ChatMessage>) {
        // 当前实现保持不变
        // ...
    }
}
```

### 1.6 配置

```rust
// src/types/config.rs
pub struct AgentDefaults {
    // 现有字段...
    
    /// Agent 模式：simple, react, plan_execute
    #[serde(default = "default_agent_mode")]
    pub mode: String,
}

fn default_agent_mode() -> String {
    "simple".to_string()
}
```

### 1.7 测试

```rust
// src/agent/react.rs
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn parse_react_step_with_thought_and_action() {
        let parser = ReActParser::new();
        let text = r#"
Thought: I need to read the file first
Action: read_file
Action Input: {"path": "test.txt"}
"#;
        
        let step = parser.parse(text).unwrap();
        assert!(step.thought.is_some());
        assert!(step.action.is_some());
        assert_eq!(step.action.unwrap().name.as_str(), "read_file");
    }
    
    #[test]
    fn parse_react_step_with_final_answer() {
        let parser = ReActParser::new();
        let text = r#"
Thought: I now have all the information
Final Answer: The file contains "Hello World"
"#;
        
        let step = parser.parse(text).unwrap();
        assert!(step.thought.is_some());
        assert!(step.answer.is_some());
        assert!(step.answer.unwrap().contains("Hello World"));
    }
    
    #[tokio::test]
    async fn react_agent_executes_tool_and_returns_answer() {
        // Mock provider that returns ReAct format
        let provider = Arc::new(MockReActProvider::new());
        let agent = create_test_react_agent(provider).await;
        
        let (answer, _) = agent.run(vec![], &test_context()).await.unwrap();
        assert!(answer.is_some());
    }
}
```

### 1.8 文档

创建 `docs/REACT_MODE.md`：
- ReAct 模式介绍
- 使用方法
- 配置选项
- 示例
- 最佳实践

---

## Phase 2: 工具并行执行

### 2.1 目标

并行执行独立的工具调用，提升性能。

### 2.2 依赖分析

```rust
// src/tools/dependency.rs
pub struct ToolDependencyAnalyzer {
    conflict_rules: HashMap<String, Vec<String>>,
}

impl ToolDependencyAnalyzer {
    pub fn new() -> Self {
        let mut conflict_rules = HashMap::new();
        
        // 文件写入冲突
        conflict_rules.insert("write_file".to_string(), vec![
            "write_file".to_string(),
            "edit_file".to_string(),
        ]);
        
        // 命令执行冲突（可能修改文件系统）
        conflict_rules.insert("execute_command".to_string(), vec![
            "write_file".to_string(),
            "edit_file".to_string(),
            "execute_command".to_string(),
        ]);
        
        Self { conflict_rules }
    }
    
    pub fn can_run_parallel(&self, calls: &[ToolCallRequest]) -> Vec<Vec<usize>> {
        // 返回可以并行执行的工具组
        let mut groups = Vec::new();
        let mut remaining: Vec<usize> = (0..calls.len()).collect();
        
        while !remaining.is_empty() {
            let mut group = vec![remaining[0]];
            remaining.remove(0);
            
            let mut i = 0;
            while i < remaining.len() {
                let idx = remaining[i];
                if self.is_compatible_with_group(&calls[idx], &group, calls) {
                    group.push(idx);
                    remaining.remove(i);
                } else {
                    i += 1;
                }
            }
            
            groups.push(group);
        }
        
        groups
    }
    
    fn is_compatible_with_group(
        &self,
        call: &ToolCallRequest,
        group: &[usize],
        all_calls: &[ToolCallRequest],
    ) -> bool {
        for &idx in group {
            if self.has_conflict(&call.name, &all_calls[idx].name) {
                return false;
            }
        }
        true
    }
    
    fn has_conflict(&self, tool1: &str, tool2: &str) -> bool {
        if let Some(conflicts) = self.conflict_rules.get(tool1) {
            if conflicts.contains(&tool2.to_string()) {
                return true;
            }
        }
        if let Some(conflicts) = self.conflict_rules.get(tool2) {
            if conflicts.contains(&tool1.to_string()) {
                return true;
            }
        }
        false
    }
}
```

### 2.3 并行执行

```rust
// src/agent/loop_core.rs
impl AgentLoop {
    async fn execute_tool_calls_parallel(
        &self,
        calls: Vec<ToolCallRequest>,
        tool_context: &ToolContext,
    ) -> Vec<(String, String)> {
        let analyzer = ToolDependencyAnalyzer::new();
        let groups = analyzer.can_run_parallel(&calls);
        
        let mut results = Vec::new();
        
        for group in groups {
            // 并行执行同一组内的工具
            let futures = group.iter().map(|&idx| {
                let call = &calls[idx];
                let tools = self.tools.clone();
                let ctx = tool_context.clone();
                let call_id = call.id.clone();
                let name = call.name.clone();
                let args = call.arguments_json.clone();
                
                async move {
                    let result = tools.execute(name.as_str(), &args, &ctx).await
                        .unwrap_or_else(|e| format_tool_error(&e));
                    (call_id, result)
                }
            });
            
            let group_results = futures::future::join_all(futures).await;
            results.extend(group_results);
        }
        
        results
    }
}
```

### 2.4 配置

```rust
// src/types/config.rs
pub struct AgentDefaults {
    // 现有字段...
    
    /// 是否启用工具并行执行
    #[serde(default = "default_parallel_tools")]
    pub parallel_tools: bool,
}

fn default_parallel_tools() -> bool {
    true
}
```

### 2.5 测试

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn dependency_analyzer_groups_independent_tools() {
        let analyzer = ToolDependencyAnalyzer::new();
        let calls = vec![
            ToolCallRequest { name: "read_file".into(), ... },
            ToolCallRequest { name: "read_file".into(), ... },
            ToolCallRequest { name: "web_search".into(), ... },
        ];
        
        let groups = analyzer.can_run_parallel(&calls);
        assert_eq!(groups.len(), 1); // 全部可以并行
        assert_eq!(groups[0].len(), 3);
    }
    
    #[test]
    fn dependency_analyzer_separates_conflicting_tools() {
        let analyzer = ToolDependencyAnalyzer::new();
        let calls = vec![
            ToolCallRequest { name: "write_file".into(), ... },
            ToolCallRequest { name: "write_file".into(), ... },
        ];
        
        let groups = analyzer.can_run_parallel(&calls);
        assert_eq!(groups.len(), 2); // 必须串行
    }
    
    #[tokio::test]
    async fn parallel_execution_faster_than_serial() {
        let agent = create_test_agent().await;
        
        let calls = vec![
            slow_tool_call("tool1"),
            slow_tool_call("tool2"),
            slow_tool_call("tool3"),
        ];
        
        let start = Instant::now();
        agent.execute_tool_calls_parallel(calls, &ctx).await;
        let parallel_time = start.elapsed();
        
        // 并行执行应该接近单个工具的时间
        assert!(parallel_time < Duration::from_secs(2));
    }
}
```

---

## Phase 3: 语义记忆

### 3.1 目标

添加语义检索能力，改进长对话记忆。

### 3.2 架构设计

```rust
// src/agent/memory_semantic.rs
pub struct SemanticMemoryStore {
    base: MemoryStore,
    embeddings: Arc<dyn EmbeddingProvider>,
    vector_store: Arc<dyn VectorStore>,
}

#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    async fn embed(&self, text: &str) -> Result<Vec<f32>>;
    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
}

#[async_trait]
pub trait VectorStore: Send + Sync {
    async fn insert(&self, id: &str, vector: Vec<f32>, metadata: serde_json::Value) -> Result<()>;
    async fn search(&self, query_vector: Vec<f32>, limit: usize) -> Result<Vec<SearchResult>>;
    async fn delete(&self, id: &str) -> Result<()>;
}

pub struct SearchResult {
    pub id: String,
    pub score: f32,
    pub metadata: serde_json::Value,
}
```

### 3.3 实现

```rust
impl SemanticMemoryStore {
    pub async fn add_memory(&self, text: &str, metadata: serde_json::Value) -> Result<()> {
        let id = uuid::Uuid::new_v4().to_string();
        let vector = self.embeddings.embed(text).await?;
        self.vector_store.insert(&id, vector, metadata).await?;
        Ok(())
    }
    
    pub async fn search_relevant(&self, query: &str, limit: usize) -> Result<Vec<MemoryChunk>> {
        let query_vector = self.embeddings.embed(query).await?;
        let results = self.vector_store.search(query_vector, limit).await?;
        
        let chunks = results.into_iter().map(|r| MemoryChunk {
            text: r.metadata["text"].as_str().unwrap_or("").to_string(),
            timestamp: r.metadata["timestamp"].as_str().unwrap_or("").to_string(),
            score: r.score,
        }).collect();
        
        Ok(chunks)
    }
}
```

### 3.4 集成

```rust
// src/agent/context.rs
impl ContextBuilder {
    pub async fn build_system_prompt(&self) -> String {
        let mut parts = vec![self.identity_section()];
        
        // 添加语义检索的相关记忆
        if let Some(semantic) = &self.semantic_memory {
            let relevant = semantic.search_relevant("current context", 5).await
                .unwrap_or_default();
            if !relevant.is_empty() {
                let memory_text = relevant.iter()
                    .map(|c| c.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n\n");
                parts.push(format!("# Relevant Memory\n\n{}", memory_text));
            }
        }
        
        // 其他部分...
        parts.join("\n\n---\n\n")
    }
}
```

---

## Phase 4-6: 后续阶段

详细实施文档将在 Phase 1-3 完成后补充。

---

## 通用开发指南

### 代码风格

- 遵循 Rust 2024 edition 规范
- 使用 `cargo fmt` 格式化
- 使用 `cargo clippy` 检查
- 所有公共 API 必须有文档注释

### 测试要求

- 单元测试覆盖率 > 80%
- 关键路径必须有集成测试
- 使用 `mockall` 模拟外部依赖
- 性能测试使用 `criterion`

### 错误处理

- 使用 `Result<T>` 返回可能失败的操作
- 使用 `anyhow::Context` 添加错误上下文
- 不要在库代码中 panic
- 记录错误日志使用 `tracing::error!`

### 文档要求

- 每个 Phase 完成后更新 `docs/`
- 添加使用示例
- 更新 `CHANGELOG.md`
- 更新 `README.md` 如果有新功能

---

**维护**: 本文档随实施进度更新。
