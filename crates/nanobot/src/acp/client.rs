//! ACP Client implementation based on the official Rust SDK.

use std::path::PathBuf;
use std::process::Stdio;
use std::thread::JoinHandle;
use std::time::Duration;

use agent_client_protocol::{
    Agent, ClientSideConnection, ContentBlock, Implementation, InitializeRequest,
    NewSessionRequest, PromptRequest, ProtocolVersion, SessionId, StopReason,
};
use anyhow::{Context, Result, anyhow};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use tracing::{info, warn};

use crate::acp::simple_client::SimpleClient;

const INIT_TIMEOUT: Duration = Duration::from_secs(20);
const EXECUTE_TIMEOUT: Duration = Duration::from_secs(1_200);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(20);

pub struct ACPClient {
    agent_id: String,
    command_tx: mpsc::UnboundedSender<ActorCommand>,
    actor_thread: Option<JoinHandle<()>>,
}

impl ACPClient {
    pub async fn spawn(agent_id: String, command: Command, session_cwd: PathBuf) -> Result<Self> {
        Self::spawn_with_client(
            agent_id,
            command,
            session_cwd.clone(),
            SimpleClient::new(session_cwd),
        )
        .await
    }

    #[allow(unused)]
    pub async fn spawn_prompt_only(
        agent_id: String,
        command: Command,
        session_cwd: PathBuf,
    ) -> Result<Self> {
        Self::spawn_with_client(
            agent_id,
            command,
            session_cwd.clone(),
            SimpleClient::prompt_only(session_cwd),
        )
        .await
    }

    async fn spawn_with_client(
        agent_id: String,
        command: Command,
        session_cwd: PathBuf,
        client: SimpleClient,
    ) -> Result<Self> {
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (ready_tx, ready_rx) = oneshot::channel();

        let thread_name = format!("acp-{}", sanitize_thread_label(&agent_id));
        let actor_thread = std::thread::Builder::new()
            .name(thread_name)
            .spawn(move || run_actor_thread(command, session_cwd, client, command_rx, ready_tx))
            .context("failed to spawn ACP actor thread")?;

        let mut actor_thread = Some(actor_thread);
        match tokio::time::timeout(INIT_TIMEOUT, ready_rx)
            .await
            .context("ACP client initialization timed out")?
        {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                join_actor_thread(actor_thread.take())
                    .await
                    .context("joining ACP actor thread after init failure")?;
                return Err(err.context("ACP client initialization failed"));
            }
            Err(err) => {
                join_actor_thread(actor_thread.take())
                    .await
                    .context("joining ACP actor thread after channel close")?;
                return Err(anyhow!("ACP actor startup channel closed: {}", err));
            }
        }

        Ok(Self {
            agent_id,
            command_tx,
            actor_thread,
        })
    }

    pub async fn execute(&mut self, task: &str) -> Result<String> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(ActorCommand::Execute {
                task: task.to_string(),
                reply: reply_tx,
            })
            .map_err(|_| anyhow!("ACP actor is not running for '{}'", self.agent_id))?;

        match tokio::time::timeout(EXECUTE_TIMEOUT, reply_rx)
            .await
            .context("ACP execute request timed out")?
        {
            Ok(result) => result,
            Err(err) => Err(anyhow!("ACP execute response channel closed: {}", err)),
        }
    }

    pub async fn close(mut self) -> Result<()> {
        info!("closing ACP client for '{}'", self.agent_id);
        let mut shutdown_result = Ok(());
        let (reply_tx, reply_rx) = oneshot::channel();
        if self
            .command_tx
            .send(ActorCommand::Shutdown { reply: reply_tx })
            .is_ok()
        {
            shutdown_result = match tokio::time::timeout(CLOSE_TIMEOUT, reply_rx).await {
                Ok(Ok(result)) => result,
                Ok(Err(err)) => Err(anyhow!("ACP shutdown channel closed: {}", err)),
                Err(_) => Err(anyhow!("ACP shutdown timed out")),
            };
        }

        let join_result = join_actor_thread(self.actor_thread.take()).await;
        match (shutdown_result, join_result) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(err), Ok(())) => Err(err),
            (Ok(()), Err(err)) => Err(err),
            (Err(shutdown_err), Err(join_err)) => Err(anyhow!(
                "ACP shutdown failed: {}; actor join failed: {}",
                shutdown_err,
                join_err
            )),
        }
    }
}

enum ActorCommand {
    Execute {
        task: String,
        reply: oneshot::Sender<Result<String>>,
    },
    Shutdown {
        reply: oneshot::Sender<Result<()>>,
    },
}

fn run_actor_thread(
    command: Command,
    session_cwd: PathBuf,
    client: SimpleClient,
    mut command_rx: mpsc::UnboundedReceiver<ActorCommand>,
    ready_tx: oneshot::Sender<Result<()>>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            let _ = ready_tx.send(Err(anyhow!("failed to build ACP runtime: {}", err)));
            return;
        }
    };

    runtime.block_on(async move {
        let local_set = tokio::task::LocalSet::new();
        local_set
            .run_until(async move {
                let mut actor = match ACPActor::initialize(command, session_cwd, client).await {
                    Ok(actor) => {
                        let _ = ready_tx.send(Ok(()));
                        actor
                    }
                    Err(err) => {
                        let _ = ready_tx.send(Err(err));
                        return;
                    }
                };

                actor.run_loop(&mut command_rx).await;
            })
            .await;
    });
}

struct ACPActor {
    process: Child,
    connection: ClientSideConnection,
    session_id: SessionId,
    client: SimpleClient,
}

impl ACPActor {
    async fn initialize(
        mut command: Command,
        session_cwd: PathBuf,
        client: SimpleClient,
    ) -> Result<Self> {
        let mut process = command
            .spawn()
            .context("failed to spawn ACP agent process")?;
        let outgoing = process
            .stdin
            .take()
            .ok_or_else(|| anyhow!("ACP process stdin unavailable"))?;
        let incoming = process
            .stdout
            .take()
            .ok_or_else(|| anyhow!("ACP process stdout unavailable"))?;

        let capabilities = client.capabilities();

        let (connection, io_task) = ClientSideConnection::new(
            client.clone(),
            outgoing.compat_write(),
            incoming.compat(),
            |future| {
                tokio::task::spawn_local(future);
            },
        );

        tokio::task::spawn_local(async move {
            if let Err(err) = io_task.await {
                warn!("ACP transport loop exited with error: {}", err);
            }
        });

        let initialize_request = InitializeRequest::new(ProtocolVersion::LATEST)
            .client_capabilities(capabilities)
            .client_info(Implementation::new("nanobot-rs", env!("CARGO_PKG_VERSION")));

        let initialize_response = connection
            .initialize(initialize_request)
            .await
            .map_err(|err| anyhow!("ACP initialize failed: {}", err))?;
        info!(
            "ACP initialized with protocol version {}",
            initialize_response.protocol_version
        );

        let new_session_response = connection
            .new_session(NewSessionRequest::new(session_cwd))
            .await
            .map_err(|err| anyhow!("ACP new_session failed: {}", err))?;

        Ok(Self {
            process,
            connection,
            session_id: new_session_response.session_id,
            client,
        })
    }

    async fn run_loop(&mut self, command_rx: &mut mpsc::UnboundedReceiver<ActorCommand>) {
        while let Some(command) = command_rx.recv().await {
            match command {
                ActorCommand::Execute { task, reply } => {
                    let _ = reply.send(self.execute_turn(task).await);
                }
                ActorCommand::Shutdown { reply } => {
                    let _ = reply.send(self.shutdown().await);
                    return;
                }
            }
        }

        let _ = self.shutdown().await;
    }

    async fn execute_turn(&mut self, task: String) -> Result<String> {
        self.client.begin_turn(&self.session_id).await;

        let prompt_request =
            PromptRequest::new(self.session_id.clone(), vec![ContentBlock::from(task)]);
        match self.connection.prompt(prompt_request).await {
            Ok(response) => Ok(self
                .client
                .take_turn_output(&self.session_id, response.stop_reason)
                .await),
            Err(err) => {
                let partial = self
                    .client
                    .take_turn_output(&self.session_id, StopReason::Cancelled)
                    .await;
                let partial_output = if partial.starts_with("(ACP turn finished:") {
                    String::new()
                } else {
                    partial
                };

                if partial_output.is_empty() {
                    Err(anyhow!("ACP prompt failed: {}", err))
                } else {
                    Err(anyhow!(
                        "ACP prompt failed: {}. Partial output:\n{}",
                        err,
                        partial_output
                    ))
                }
            }
        }
    }

    async fn shutdown(&mut self) -> Result<()> {
        self.client.close_all_terminals().await;

        if self
            .process
            .try_wait()
            .context("checking ACP process status")?
            .is_none()
        {
            self.process
                .kill()
                .await
                .context("killing ACP process during shutdown")?;
        }
        let _ = self.process.wait().await;
        Ok(())
    }
}

fn resolve_session_cwd(cwd: Option<PathBuf>) -> Result<PathBuf> {
    let cwd = if let Some(cwd) = cwd {
        cwd
    } else {
        std::env::current_dir().context("reading current directory for ACP session")?
    };

    if cwd.is_absolute() {
        Ok(cwd)
    } else {
        Ok(std::env::current_dir()
            .context("reading current directory for ACP relative path")?
            .join(cwd))
    }
}

/// Build a Command for spawning an ACP agent process.
///
/// This function assembles the command with proper configuration:
/// - Sets the working directory
/// - Configures stdin/stdout/stderr pipes
/// - Applies environment variables
/// - Adds command-line arguments
pub fn build_acp_command(
    command_str: &str,
    args: &[String],
    cwd: Option<PathBuf>,
    env: &std::collections::HashMap<String, String>,
) -> Result<(Command, PathBuf)> {
    let session_cwd = resolve_session_cwd(cwd)?;

    let mut command = Command::new(command_str);
    command.args(args);
    command.current_dir(&session_cwd);
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::null());

    for (key, value) in env {
        command.env(key, value);
    }

    Ok((command, session_cwd))
}

async fn join_actor_thread(thread: Option<JoinHandle<()>>) -> Result<()> {
    let Some(thread) = thread else {
        return Ok(());
    };

    tokio::task::spawn_blocking(move || {
        thread
            .join()
            .map_err(|_| anyhow!("ACP actor thread panicked"))
    })
    .await
    .context("waiting for ACP actor thread")?
}

fn sanitize_thread_label(agent_id: &str) -> String {
    let sanitized = agent_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        "agent".to_string()
    } else {
        sanitized
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_session_cwd_makes_relative_path_absolute() {
        let path = resolve_session_cwd(Some(PathBuf::from("src"))).expect("resolve path");
        assert!(path.is_absolute());
        assert!(path.ends_with("src"));
    }

    #[test]
    fn sanitize_thread_label_replaces_unsupported_chars() {
        assert_eq!(sanitize_thread_label("codex@main"), "codex_main");
        assert_eq!(sanitize_thread_label(""), "agent");
    }

    #[tokio::test]
    #[ignore = "requires local codex CLI and valid auth/session"]
    async fn smoke_local_codex() {
        let cwd = std::env::current_dir().expect("current dir");
        let command_str =
            std::env::var("ACP_SMOKE_COMMAND").unwrap_or_else(|_| "codex-acp".to_string());

        let (command, session_cwd) = build_acp_command(
            &command_str,
            &[],
            Some(cwd),
            &std::collections::HashMap::new(),
        )
        .expect("build command");

        let mut client = ACPClient::spawn("codex".to_string(), command, session_cwd)
            .await
            .expect("spawn ACP client");

        let output = client
            .execute("Reply with one short sentence that confirms ACP is working.")
            .await
            .expect("execute prompt");
        assert!(
            !output.trim().is_empty(),
            "codex output should not be empty"
        );

        client.close().await.expect("close ACP client");
    }
}
