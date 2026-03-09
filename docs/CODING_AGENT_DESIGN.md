# Coding Agent 桌面客户端设计

**文档版本**: 1.0  
**创建日期**: 2026-03-07  
**目标**: Tauri 桌面客户端，专注代码编写

---

## 目录

1. [主流 Coding Agent 分析](#1-主流-coding-agent-分析)
2. [当前设计的不足](#2-当前设计的不足)
3. [改进方案](#3-改进方案)
4. [架构设计](#4-架构设计)

---

## 1. 主流 Coding Agent 分析

### 1.1 Cursor

**核心特性**:
- ✅ **编辑器集成**: 基于 VSCode fork
- ✅ **Diff View**: 实时预览代码变更
- ✅ **多文件编辑**: 同时修改多个文件
- ✅ **上下文感知**: 自动包含相关文件
- ✅ **Inline Chat**: 在代码中直接对话
- ✅ **Composer**: 多文件任务规划
- ✅ **Terminal 集成**: 执行命令并查看输出
- ✅ **Git 集成**: 自动提交、查看 diff

**架构特点**:
```
VSCode Extension
  ↓
Language Server Protocol (LSP)
  ↓
AI Backend (Claude/GPT)
  ↓
File System Watcher
```

### 1.2 Windsurf (Codeium)

**核心特性**:
- ✅ **Cascade**: 多步骤任务流
- ✅ **Supercomplete**: 智能补全
- ✅ **Flow Mode**: 持续编辑模式
- ✅ **Context Engine**: 智能上下文选择
- ✅ **Diff Preview**: 变更预览
- ✅ **Multi-file Edit**: 批量编辑

**架构特点**:
```
独立 IDE
  ↓
自研 LSP + Context Engine
  ↓
AI Backend
  ↓
Real-time Collaboration
```

### 1.3 Cline (VSCode Extension)

**核心特性**:
- ✅ **Task-based**: 任务驱动
- ✅ **Approval Flow**: 每步需确认
- ✅ **File Tree**: 可视化文件结构
- ✅ **Command Execution**: 安全执行命令
- ✅ **Browser Integration**: 网页搜索
- ✅ **Memory**: 跨会话记忆

**架构特点**:
```
VSCode Extension API
  ↓
Task Queue + Approval System
  ↓
Tool Execution (sandboxed)
  ↓
AI Backend
```

### 1.4 Aider

**核心特性**:
- ✅ **CLI-first**: 命令行优先
- ✅ **Git-aware**: 深度 Git 集成
- ✅ **Diff-based**: 基于 diff 编辑
- ✅ **Multi-model**: 支持多种模型
- ✅ **Context Management**: 智能上下文
- ✅ **Undo/Redo**: 完整的撤销系统

**架构特点**:
```
CLI Interface
  ↓
Git Integration (diff, commit)
  ↓
Tree-sitter (code parsing)
  ↓
AI Backend
```

### 1.5 GitHub Copilot Workspace

**核心特性**:
- ✅ **Issue-to-Code**: 从 issue 生成代码
- ✅ **Plan Generation**: 自动生成实施计划
- ✅ **Multi-file Changes**: 批量修改
- ✅ **PR Integration**: 自动创建 PR
- ✅ **Code Review**: AI 代码审查
- ✅ **Test Generation**: 自动生成测试

**架构特点**:
```
Web-based IDE
  ↓
GitHub API Integration
  ↓
Plan → Code → Test → PR
  ↓
Copilot AI
```

### 1.6 共同特点总结

| 特性 | Cursor | Windsurf | Cline | Aider | Copilot WS |
|------|--------|----------|-------|-------|------------|
| Diff View | ✅ | ✅ | ✅ | ✅ | ✅ |
| 多文件编辑 | ✅ | ✅ | ✅ | ✅ | ✅ |
| LSP 集成 | ✅ | ✅ | ✅ | ❌ | ✅ |
| Git 集成 | ✅ | ✅ | ⚠️ | ✅✅ | ✅✅ |
| 上下文感知 | ✅ | ✅✅ | ✅ | ✅ | ✅ |
| 任务规划 | ✅ | ✅ | ✅ | ❌ | ✅✅ |
| 实时预览 | ✅ | ✅ | ⚠️ | ❌ | ✅ |
| 撤销/重做 | ✅ | ✅ | ⚠️ | ✅✅ | ✅ |

---

## 2. 当前设计的不足

### 2.1 缺少编辑器集成

**问题**:
- 当前只有 `write_file` 和 `edit_file` 工具
- 没有 LSP 集成
- 没有语法高亮、代码补全
- 没有实时错误检查

**影响**:
- 无法提供 IDE 级别的体验
- 代码质量难以保证
- 开发效率低

**主流方案**:
```rust
// LSP 集成
pub struct LSPClient {
    pub language_server: Arc<LanguageServer>,
}

impl LSPClient {
    pub async fn get_diagnostics(&self, file: &Path) -> Vec<Diagnostic>;
    pub async fn get_completions(&self, file: &Path, pos: Position) -> Vec<Completion>;
    pub async fn get_hover(&self, file: &Path, pos: Position) -> Option<Hover>;
    pub async fn goto_definition(&self, file: &Path, pos: Position) -> Option<Location>;
}
```

### 2.2 缺少 Diff 预览

**问题**:
- 直接修改文件，无法预览
- 用户无法审查变更
- 错误修改难以撤销

**影响**:
- 用户体验差
- 容易破坏代码
- 缺少信任感

**主流方案**:
```rust
// Diff 系统
pub struct DiffView {
    pub original: String,
    pub modified: String,
    pub hunks: Vec<DiffHunk>,
}

pub struct DiffHunk {
    pub old_start: usize,
    pub old_lines: usize,
    pub new_start: usize,
    pub new_lines: usize,
    pub lines: Vec<DiffLine>,
}

pub enum DiffLine {
    Context(String),
    Added(String),
    Removed(String),
}
```

### 2.3 缺少多文件协调

**问题**:
- 工具调用是独立的
- 没有事务性
- 无法原子性地修改多个文件

**影响**:
- 修改一半失败，代码不一致
- 无法回滚
- 重构困难

**主流方案**:
```rust
// 文件事务
pub struct FileTransaction {
    changes: Vec<FileChange>,
    state: TransactionState,
}

impl FileTransaction {
    pub fn add_change(&mut self, change: FileChange);
    pub async fn preview(&self) -> Vec<DiffView>;
    pub async fn commit(&mut self) -> Result<()>;
    pub async fn rollback(&mut self) -> Result<()>;
}
```

### 2.4 缺少上下文管理

**问题**:
- 依赖 LLM 自己选择文件
- 没有智能上下文选择
- 容易遗漏相关文件

**影响**:
- 上下文不完整
- 修改可能不一致
- token 浪费

**主流方案**:
```rust
// 上下文引擎
pub struct ContextEngine {
    pub graph: CodeGraph,
    pub embeddings: EmbeddingStore,
}

impl ContextEngine {
    // 基于依赖关系查找相关文件
    pub async fn find_related_files(&self, file: &Path) -> Vec<PathBuf>;
    
    // 基于语义查找相关代码
    pub async fn semantic_search(&self, query: &str) -> Vec<CodeChunk>;
    
    // 智能选择上下文
    pub async fn select_context(&self, task: &str, max_tokens: usize) -> Context;
}
```

### 2.5 缺少代码理解

**问题**:
- 没有 AST 解析
- 没有符号索引
- 没有依赖分析

**影响**:
- 无法理解代码结构
- 重构困难
- 容易破坏代码

**主流方案**:
```rust
// 代码解析
pub struct CodeParser {
    pub tree_sitter: TreeSitter,
}

impl CodeParser {
    pub fn parse(&self, code: &str, lang: Language) -> SyntaxTree;
    pub fn extract_symbols(&self, tree: &SyntaxTree) -> Vec<Symbol>;
    pub fn find_references(&self, symbol: &Symbol) -> Vec<Location>;
}

// 符号索引
pub struct SymbolIndex {
    pub symbols: HashMap<String, Vec<Symbol>>,
}

impl SymbolIndex {
    pub fn find_definition(&self, name: &str) -> Option<Symbol>;
    pub fn find_references(&self, name: &str) -> Vec<Symbol>;
}
```

### 2.6 缺少实时反馈

**问题**:
- 工具执行是黑盒
- 没有进度显示
- 没有中间结果

**影响**:
- 用户不知道发生了什么
- 长时间等待焦虑
- 无法及时干预

**主流方案**:
```rust
// 流式输出
pub trait StreamingExecutor {
    async fn execute_streaming(
        &self,
        ctx: ExecutionContext,
    ) -> impl Stream<Item = ExecutionEvent>;
}

pub enum ExecutionEvent {
    Thinking(String),
    ToolCall { name: String, args: String },
    ToolResult { name: String, result: String },
    FileChange { path: PathBuf, diff: DiffView },
    Progress { current: usize, total: usize },
    Complete(ExecutionResult),
}
```

### 2.7 缺少安全机制

**问题**:
- 工具执行没有沙箱
- 没有权限控制
- 没有审批流程

**影响**:
- 可能破坏代码
- 可能执行危险命令
- 用户缺少控制

**主流方案**:
```rust
// 审批系统
pub struct ApprovalSystem {
    pub pending: Vec<PendingAction>,
}

pub struct PendingAction {
    pub action: Action,
    pub risk_level: RiskLevel,
    pub preview: ActionPreview,
}

pub enum RiskLevel {
    Safe,      // 自动执行
    Low,       // 通知用户
    Medium,    // 需要确认
    High,      // 需要审查
    Critical,  // 需要明确批准
}
```

### 2.8 缺少项目感知

**问题**:
- 不理解项目结构
- 不知道构建系统
- 不知道测试框架

**影响**:
- 无法正确构建
- 无法运行测试
- 无法理解项目约定

**主流方案**:
```rust
// 项目检测
pub struct ProjectDetector {
    pub detectors: Vec<Box<dyn ProjectTypeDetector>>,
}

pub trait ProjectTypeDetector {
    fn detect(&self, root: &Path) -> Option<ProjectInfo>;
}

pub struct ProjectInfo {
    pub project_type: ProjectType,
    pub build_system: BuildSystem,
    pub test_framework: Option<TestFramework>,
    pub dependencies: Vec<Dependency>,
}

pub enum ProjectType {
    Rust { edition: String },
    Node { package_manager: String },
    Python { version: String },
    // ...
}
```

---

## 3. 改进方案

### 3.1 短期改进（2-4 周）

#### 3.1.1 Diff 预览系统

**优先级**: 🔴 最高

**实现**:
```rust
// src/editor/diff.rs
pub struct DiffEngine {
    pub differ: Arc<dyn Differ>,
}

impl DiffEngine {
    pub fn compute_diff(&self, original: &str, modified: &str) -> DiffView;
    pub fn apply_diff(&self, original: &str, diff: &DiffView) -> String;
    pub fn merge_diffs(&self, diffs: Vec<DiffView>) -> Result<DiffView>;
}

// Tauri 命令
#[tauri::command]
async fn preview_changes(
    file: String,
    new_content: String,
) -> Result<DiffView> {
    let original = fs::read_to_string(&file)?;
    let diff = diff_engine.compute_diff(&original, &new_content);
    Ok(diff)
}

#[tauri::command]
async fn apply_changes(
    file: String,
    diff: DiffView,
    approved: bool,
) -> Result<()> {
    if !approved {
        return Err("User rejected changes".into());
    }
    // 应用变更
    Ok(())
}
```

**UI 设计**:
```typescript
// Tauri 前端
interface DiffViewProps {
  original: string;
  modified: string;
  hunks: DiffHunk[];
  onApprove: () => void;
  onReject: () => void;
}

// 类似 GitHub PR diff view
<DiffView
  original={original}
  modified={modified}
  hunks={hunks}
  onApprove={handleApprove}
  onReject={handleReject}
/>
```

#### 3.1.2 文件事务系统

**优先级**: 🔴 最高

**实现**:
```rust
// src/editor/transaction.rs
pub struct FileTransaction {
    id: Uuid,
    changes: Vec<FileChange>,
    state: TransactionState,
    created_at: DateTime<Utc>,
}

pub struct FileChange {
    pub path: PathBuf,
    pub operation: FileOperation,
    pub original_content: Option<String>,
    pub new_content: Option<String>,
}

pub enum FileOperation {
    Create,
    Modify,
    Delete,
    Rename { to: PathBuf },
}

impl FileTransaction {
    pub fn new() -> Self;
    
    pub fn add_change(&mut self, change: FileChange);
    
    pub async fn preview(&self) -> Vec<DiffView>;
    
    pub async fn commit(&mut self) -> Result<()> {
        // 1. 验证所有变更
        self.validate()?;
        
        // 2. 创建备份
        self.create_backup()?;
        
        // 3. 应用变更
        for change in &self.changes {
            self.apply_change(change).await?;
        }
        
        // 4. 更新状态
        self.state = TransactionState::Committed;
        
        Ok(())
    }
    
    pub async fn rollback(&mut self) -> Result<()> {
        // 从备份恢复
        self.restore_backup()?;
        self.state = TransactionState::RolledBack;
        Ok(())
    }
}
```

#### 3.1.3 LSP 集成

**优先级**: 🟡 高

**实现**:
```rust
// src/editor/lsp.rs
pub struct LSPManager {
    clients: HashMap<Language, Arc<LSPClient>>,
}

impl LSPManager {
    pub async fn start_server(&mut self, lang: Language) -> Result<()>;
    
    pub async fn get_diagnostics(&self, file: &Path) -> Result<Vec<Diagnostic>>;
    
    pub async fn format_document(&self, file: &Path) -> Result<String>;
    
    pub async fn get_completions(
        &self,
        file: &Path,
        position: Position,
    ) -> Result<Vec<Completion>>;
}

// 工具集成
pub struct LSPTool {
    lsp: Arc<LSPManager>,
}

#[async_trait]
impl Tool for LSPTool {
    async fn execute(&self, args: &str) -> Result<String> {
        let req: LSPRequest = serde_json::from_str(args)?;
        match req.action {
            LSPAction::Diagnostics => {
                let diags = self.lsp.get_diagnostics(&req.file).await?;
                Ok(serde_json::to_string(&diags)?)
            }
            LSPAction::Format => {
                let formatted = self.lsp.format_document(&req.file).await?;
                Ok(formatted)
            }
            // ...
        }
    }
}
```

### 3.2 中期改进（1-2 个月）

#### 3.2.1 上下文引擎

**实现**:
```rust
// src/editor/context.rs
pub struct ContextEngine {
    graph: CodeGraph,
    embeddings: Arc<EmbeddingStore>,
    index: Arc<SymbolIndex>,
}

impl ContextEngine {
    pub async fn build_context(
        &self,
        task: &str,
        current_file: Option<&Path>,
        max_tokens: usize,
    ) -> Result<Context> {
        let mut context = Context::new();
        
        // 1. 当前文件
        if let Some(file) = current_file {
            context.add_file(file, Priority::High);
        }
        
        // 2. 语义搜索相关代码
        let relevant = self.embeddings
            .search(task, 10)
            .await?;
        for chunk in relevant {
            context.add_chunk(chunk, Priority::Medium);
        }
        
        // 3. 依赖关系
        if let Some(file) = current_file {
            let deps = self.graph.get_dependencies(file)?;
            for dep in deps {
                context.add_file(&dep, Priority::Low);
            }
        }
        
        // 4. 裁剪到 token 限制
        context.trim_to_tokens(max_tokens);
        
        Ok(context)
    }
}
```

#### 3.2.2 代码解析

**实现**:
```rust
// src/editor/parser.rs
pub struct CodeParser {
    tree_sitter: TreeSitter,
    parsers: HashMap<Language, Parser>,
}

impl CodeParser {
    pub fn parse(&self, code: &str, lang: Language) -> Result<SyntaxTree>;
    
    pub fn extract_symbols(&self, tree: &SyntaxTree) -> Vec<Symbol>;
    
    pub fn find_node_at(&self, tree: &SyntaxTree, pos: Position) -> Option<Node>;
    
    pub fn get_scope(&self, tree: &SyntaxTree, pos: Position) -> Option<Scope>;
}

// 符号提取
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub location: Location,
    pub scope: String,
}

pub enum SymbolKind {
    Function,
    Class,
    Variable,
    Constant,
    Type,
    // ...
}
```

#### 3.2.3 项目感知

**实现**:
```rust
// src/editor/project.rs
pub struct ProjectManager {
    root: PathBuf,
    info: ProjectInfo,
    watcher: FileWatcher,
}

impl ProjectManager {
    pub async fn detect(root: &Path) -> Result<Self>;
    
    pub async fn build(&self) -> Result<BuildOutput>;
    
    pub async fn test(&self) -> Result<TestOutput>;
    
    pub async fn run(&self, target: Option<&str>) -> Result<RunOutput>;
    
    pub fn get_dependencies(&self) -> &[Dependency];
    
    pub fn get_entry_points(&self) -> Vec<PathBuf>;
}
```

### 3.3 长期改进（3-6 个月）

#### 3.3.1 实时协作

**实现**:
```rust
// src/editor/collaboration.rs
pub struct CollaborationEngine {
    crdt: Arc<CRDT>,
    sync: Arc<SyncEngine>,
}

// 支持多用户同时编辑
// 类似 Windsurf 的实时协作
```

#### 3.3.2 智能补全

**实现**:
```rust
// src/editor/completion.rs
pub struct SmartCompletion {
    lsp: Arc<LSPManager>,
    ai: Arc<dyn LLMProvider>,
}

// 结合 LSP 和 AI 的智能补全
// 类似 Cursor 的 Tab 补全
```

---

## 4. 架构设计

### 4.1 整体架构

```
┌─────────────────────────────────────────┐
│         Tauri Frontend (React/Vue)      │
│  ┌──────────┐  ┌──────────┐  ┌────────┐│
│  │ Editor   │  │ Diff View│  │ Chat   ││
│  │ (Monaco) │  │          │  │ Panel  ││
│  └──────────┘  └──────────┘  └────────┘│
└─────────────────────────────────────────┘
              ↕ Tauri IPC
┌─────────────────────────────────────────┐
│         Rust Backend (nanobot-rs)       │
│  ┌──────────────────────────────────┐   │
│  │      Editor Module (新增)        │   │
│  │  ┌────────┐  ┌────────┐         │   │
│  │  │ LSP    │  │ Diff   │         │   │
│  │  │ Client │  │ Engine │         │   │
│  │  └────────┘  └────────┘         │   │
│  │  ┌────────┐  ┌────────┐         │   │
│  │  │Context │  │ Parser │         │   │
│  │  │Engine  │  │        │         │   │
│  │  └────────┘  └────────┘         │   │
│  └──────────────────────────────────┘   │
│  ┌──────────────────────────────────┐   │
│  │      Agent Module (现有)         │   │
│  │  ┌────────┐  ┌────────┐         │   │
│  │  │Executor│  │ Tools  │         │   │
│  │  │Registry│  │Registry│         │   │
│  │  └────────┘  └────────┘         │   │
│  └──────────────────────────────────┘   │
└─────────────────────────────────────────┘
```

### 4.2 模块划分

```rust
// src/editor/mod.rs
pub mod diff;          // Diff 引擎
pub mod transaction;   // 文件事务
pub mod lsp;          // LSP 集成
pub mod context;      // 上下文引擎
pub mod parser;       // 代码解析
pub mod project;      // 项目管理
pub mod completion;   // 智能补全
pub mod collaboration;// 协作（未来）

// src/tauri/mod.rs
pub mod commands;     // Tauri 命令
pub mod events;       // 事件系统
pub mod state;        // 应用状态
```

### 4.3 Tauri 集成

```rust
// src/tauri/commands.rs

#[tauri::command]
async fn execute_agent_task(
    task: String,
    files: Vec<String>,
    state: State<'_, AppState>,
) -> Result<TaskResult> {
    // 1. 构建上下文
    let context = state.context_engine
        .build_context(&task, files.first().map(Path::new), 8000)
        .await?;
    
    // 2. 创建事务
    let mut transaction = FileTransaction::new();
    
    // 3. 执行 agent
    let result = state.agent
        .execute_with_transaction(&task, context, &mut transaction)
        .await?;
    
    // 4. 预览变更
    let previews = transaction.preview().await?;
    
    Ok(TaskResult {
        response: result.response,
        changes: previews,
        transaction_id: transaction.id,
    })
}

#[tauri::command]
async fn approve_changes(
    transaction_id: Uuid,
    state: State<'_, AppState>,
) -> Result<()> {
    let mut transaction = state.transactions
        .get_mut(&transaction_id)
        .ok_or("Transaction not found")?;
    
    transaction.commit().await?;
    
    Ok(())
}

#[tauri::command]
async fn get_diagnostics(
    file: String,
    state: State<'_, AppState>,
) -> Result<Vec<Diagnostic>> {
    let diags = state.lsp_manager
        .get_diagnostics(Path::new(&file))
        .await?;
    
    Ok(diags)
}
```

### 4.4 前端设计

```typescript
// src-tauri/bindings.ts (自动生成)
export interface DiffView {
  original: string;
  modified: string;
  hunks: DiffHunk[];
}

export interface TaskResult {
  response: string;
  changes: DiffView[];
  transaction_id: string;
}

// src/components/AgentChat.tsx
function AgentChat() {
  const [task, setTask] = useState('');
  const [result, setResult] = useState<TaskResult | null>(null);
  
  const handleSubmit = async () => {
    const result = await invoke<TaskResult>('execute_agent_task', {
      task,
      files: getCurrentFiles(),
    });
    setResult(result);
  };
  
  const handleApprove = async () => {
    await invoke('approve_changes', {
      transactionId: result.transaction_id,
    });
    // 刷新编辑器
  };
  
  return (
    <div>
      <ChatInput value={task} onChange={setTask} onSubmit={handleSubmit} />
      {result && (
        <DiffPreview
          changes={result.changes}
          onApprove={handleApprove}
          onReject={() => setResult(null)}
        />
      )}
    </div>
  );
}
```

---

## 5. 实施优先级

### 5.1 P0 - 必须有（2 周）

1. **Diff 预览系统**
   - 用户必须能看到变更
   - 必须能批准/拒绝

2. **文件事务系统**
   - 多文件修改必须原子性
   - 必须能回滚

3. **基础 Tauri 集成**
   - 前后端通信
   - 状态管理

### 5.2 P1 - 应该有（4 周）

1. **LSP 集成**
   - 语法检查
   - 代码格式化

2. **上下文引擎**
   - 智能选择相关文件
   - 减少 token 浪费

3. **项目感知**
   - 检测项目类型
   - 运行构建/测试

### 5.3 P2 - 可以有（8 周）

1. **代码解析**
   - AST 分析
   - 符号索引

2. **智能补全**
   - AI 辅助补全

3. **实时协作**
   - 多用户编辑

---

## 6. 总结

### 6.1 关键差距

当前 nanobot-rs 作为 IM bot 设计，缺少：
1. ❌ 编辑器集成（LSP）
2. ❌ Diff 预览
3. ❌ 文件事务
4. ❌ 上下文引擎
5. ❌ 代码理解
6. ❌ 项目感知
7. ❌ 实时反馈
8. ❌ 安全审批

### 6.2 改进路线

**Phase 1 (2 周)**: Diff + Transaction + Tauri 基础
**Phase 2 (4 周)**: LSP + Context + Project
**Phase 3 (8 周)**: Parser + Completion + Collaboration

### 6.3 参考实现

- **Diff**: `similar` crate
- **LSP**: `tower-lsp` crate
- **Parser**: `tree-sitter` crate
- **Tauri**: `tauri` v2
- **前端编辑器**: Monaco Editor

---

**下一步**: 开始实施 Phase 1 - Diff 预览系统
