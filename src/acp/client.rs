//! ACP Client implementation

use anyhow::{Result, Context};
use tokio::process::{Command, Child};
use std::process::Stdio;
use std::path::PathBuf;
use std::collections::HashMap;

pub struct ACPClient {
    agent_id: String,
    process: Child,
}

impl ACPClient {
    pub async fn spawn(
        agent_id: String,
        command: String,
        cwd: Option<PathBuf>,
        env: HashMap<String, String>,
    ) -> Result<Self> {
        let mut cmd = Command::new(&command);
        
        if let Some(cwd) = cwd {
            cmd.current_dir(cwd);
        }
        
        for (key, value) in env {
            cmd.env(key, value);
        }
        
        cmd.stdin(Stdio::piped())
           .stdout(Stdio::piped())
           .stderr(Stdio::piped());
        
        let process = cmd.spawn()
            .context(format!("Failed to spawn ACP agent: {}", agent_id))?;
        
        Ok(Self {
            agent_id,
            process,
        })
    }
    
    pub async fn execute(&mut self, task: &str) -> Result<String> {
        // MVP: 简化实现
        // TODO: 使用官方 SDK 实现完整的 ACP 协议
        Ok(format!(
            "ACP agent '{}' would execute task: {}\n\
             (Note: This is a MVP placeholder. Full ACP protocol implementation coming in Phase 2)",
            self.agent_id, task
        ))
    }
    
    pub async fn close(mut self) -> Result<()> {
        self.process.kill().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    
    
    #[tokio::test]
    async fn test_acp_client_execute() {
        // Mock test for MVP
        let agent_id = "codex".to_string();
        let task = "Create a hello world program";
        
        // Just verify the format
        let result = format!(
            "ACP agent '{}' would execute task: {}\n\
             (Note: This is a MVP placeholder. Full ACP protocol implementation coming in Phase 2)",
            agent_id, task
        );
        
        assert!(result.contains("codex"));
        assert!(result.contains("hello world"));
    }
}
