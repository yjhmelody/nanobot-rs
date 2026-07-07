//! High-level ACP client that spawns an external agent process and
//! communicates with it via the Agent Client Protocol.
//!
//! ## Architecture
//!
//! `ACPClient` is the public API used by `ACPTool`. It provides three
//! lifecycle operations:
//!
//! 1. `spawn` — launches a subprocess and performs the ACP initialisation
//!    handshake (initialize, new_session) in a dedicated OS thread.
//! 2. `execute` — sends a natural-language task to the ACP agent and streams
//!    back the result.
//! 3. `close` — cleanly shuts down the subprocess.
//!
//! ## Threading Model
//!
//! ACP runs on a dedicated OS thread (`run_actor_thread`) with its own tokio
//! `current_thread` runtime and a `LocalSet` (required by the
//! `ClientSideConnection` which is `!Send`). Communication with the async
//! main thread happens via `mpsc` and `oneshot` channels.
//!
//! ## Timeouts
//!
//! - **Initialisation**: 20 seconds (`INIT_TIMEOUT`)
//! - **Execution**: 1200 seconds (20 minutes, `EXECUTE_TIMEOUT`)
//! - **Shutdown**: 20 seconds (`CLOSE_TIMEOUT`)

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

/// Duration to wait for the ACP agent to complete initialisation handshake.
const INIT_TIMEOUT: Duration = Duration::from_secs(20);
/// Maximum duration for a single ACP execute request (20 minutes).
///
/// Coding tasks can involve long-running edits, so this is deliberately generous.
const EXECUTE_TIMEOUT: Duration = Duration::from_secs(1_200);
/// Duration to wait for the ACP agent to shut down cleanly.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(20);
/// Tracing log target for this module.
const TARGET: &str = "nanobot::acp::client";

/// A client that manages the lifecycle of an external ACP agent process.
///
/// # Fields
///
/// * `agent_id` — User-visible identifier (e.g., "codex") used in log messages and errors.
/// * `command_tx` — Channel sender for dispatching `ActorCommand` messages to the actor thread.
/// * `actor_thread` — Handle to the dedicated OS thread running the ACP actor.
pub struct ACPClient {
    agent_id: String,
    command_tx: mpsc::UnboundedSender<ActorCommand>,
    actor_thread: Option<JoinHandle<()>>,
}

impl ACPClient {
    /// Spawn an ACP agent process with full filesystem and terminal access.
    ///
    /// This is the main entry point. The underlying `SimpleClient` is
    /// configured with both filesystem and terminal capabilities.
    ///
    /// # Arguments
    ///
    /// * `agent_id` — Human-readable label for logging.
    /// * `command` — A pre-configured `tokio::process::Command` (build via `build_acp_command`).
    /// * `session_cwd` — Working directory for the ACP session.
    ///
    /// # Errors
    ///
    /// Returns an error if the subprocess cannot be spawned, the handshake
    /// times out, or the actor thread panics.
    pub async fn spawn(agent_id: String, command: Command, session_cwd: PathBuf) -> Result<Self> {
        Self::spawn_with_client(
            agent_id,
            command,
            session_cwd.clone(),
            SimpleClient::new(session_cwd),
        )
        .await
    }

    /// Spawn an ACP agent process with prompt-only capabilities (no filesystem or terminal).
    ///
    /// Useful when the external agent should only generate text and not
    /// perform read/write or shell operations.
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

    /// Internal: spawn with a caller-provided `SimpleClient` (enables testability).
    ///
    /// Creates the actor thread, waits for initialisation, then returns an
    /// `ACPClient` ready to receive execute requests.
    async fn spawn_with_client(
        agent_id: String,
        command: Command,
        session_cwd: PathBuf,
        client: SimpleClient,
    ) -> Result<Self> {
        // Channel for sending commands to the actor thread.
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        // Channel the actor uses to signal that initialisation is complete.
        let (ready_tx, ready_rx) = oneshot::channel();

        let thread_name = format!("acp-{}", sanitize_thread_label(&agent_id));
        let actor_thread = std::thread::Builder::new()
            .name(thread_name)
            .spawn(move || run_actor_thread(command, session_cwd, client, command_rx, ready_tx))
            .context("failed to spawn ACP actor thread")?;

        // Wait for initialisation with timeout.
        let mut actor_thread = Some(actor_thread);
        match tokio::time::timeout(INIT_TIMEOUT, ready_rx)
            .await
            .context("ACP client initialization timed out")?
        {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                // Initialisation reported an error; clean up the thread.
                join_actor_thread(actor_thread.take())
                    .await
                    .context("joining ACP actor thread after init failure")?;
                return Err(err.context("ACP client initialization failed"));
            }
            Err(err) => {
                // Channel sender dropped without sending — actor thread likely panicked.
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

    /// Send a natural-language task to the ACP agent and return the result.
    ///
    /// # Arguments
    ///
    /// * `task` — The natural-language instruction for the agent.
    ///
    /// # Returns
    ///
    /// The agent's text output. May be empty if the agent produced no
    /// content before finishing the turn.
    ///
    /// # Errors
    ///
    /// Returns an error if the actor thread has exited, the request times out
    /// after `EXECUTE_TIMEOUT`, or the agent signals an error.
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
            .with_context(|| {
                format!(
                    "ACP execute request timed out for '{}' after {}s (likely waiting for external auth/approval or blocked terminal interaction)",
                    self.agent_id,
                    EXECUTE_TIMEOUT.as_secs()
                )
            })?
        {
            Ok(result) => result,
            Err(err) => Err(anyhow!("ACP execute response channel closed: {}", err)),
        }
    }

    /// Shut down the ACP agent process and join the actor thread.
    ///
    /// Sends a `Shutdown` command and waits for the actor to exit gracefully.
    /// If the shutdown command cannot be sent (channel closed), the actor
    /// thread is joined directly.
    ///
    /// # Errors
    ///
    /// Returns an error if shutdown times out, the actor thread panics, or
    /// the subprocess cannot be killed.
    pub async fn close(mut self) -> Result<()> {
        info!(target: TARGET, "closing ACP client for '{}'", self.agent_id);
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
        // Merge shutdown and join errors.
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

/// Internal command type sent from `ACPClient` to the actor thread.
enum ActorCommand {
    /// Execute a natural-language task and send back the text result.
    Execute {
        task: String,
        reply: oneshot::Sender<Result<String>>,
    },
    /// Gracefully shut down the ACP agent and signal completion.
    Shutdown { reply: oneshot::Sender<Result<()>> },
}

/// Entry point for the dedicated ACP actor thread.
///
/// Creates a single-threaded tokio runtime with a `LocalSet` (required
/// because `ClientSideConnection` is `!Send`). On success, sends an `Ok(())`
/// through `ready_tx` and then enters the command-processing loop.
fn run_actor_thread(
    command: Command,
    session_cwd: PathBuf,
    client: SimpleClient,
    mut command_rx: mpsc::UnboundedReceiver<ActorCommand>,
    ready_tx: oneshot::Sender<Result<()>>,
) {
    // Build a single-threaded tokio runtime for the `!Send` ACP connection.
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

/// Internal actor that owns the ACP subprocess and protocol connection.
struct ACPActor {
    /// The spawned subprocess (stdin/stdout used for ACP transport).
    process: Child,
    /// ACP client-side protocol connection.
    connection: ClientSideConnection,
    /// Active session identifier returned by `new_session`.
    session_id: SessionId,
    /// Local handler for filesystem/terminal operations and output buffering.
    client: SimpleClient,
}

impl ACPActor {
    /// Spawn the subprocess and perform the ACP handshake.
    ///
    /// 1. Spawns the agent process and captures stdin/stdout.
    /// 2. Creates a `ClientSideConnection` over those streams.
    /// 3. Sends `InitializeRequest` with protocol capabilities and client info.
    /// 4. Sends `NewSessionRequest` with the session working directory.
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

        // `ClientSideConnection` wraps the subprocess I/O streams.
        // It requires `spawn_local` because the connection object is `!Send`.
        let (connection, io_task) = ClientSideConnection::new(
            client.clone(),
            outgoing.compat_write(),
            incoming.compat(),
            |future| {
                tokio::task::spawn_local(future);
            },
        );

        // Drive the transport layer — reads/writes ACP frames on the I/O streams.
        tokio::task::spawn_local(async move {
            if let Err(err) = io_task.await {
                warn!(target: TARGET, "ACP transport loop exited with error: {}", err);
            }
        });

        let initialize_request = InitializeRequest::new(ProtocolVersion::LATEST)
            .client_capabilities(capabilities)
            .client_info(Implementation::new("nanobot", env!("CARGO_PKG_VERSION")));

        let initialize_response = connection
            .initialize(initialize_request)
            .await
            .map_err(|err| anyhow!("ACP initialize failed: {}", err))?;
        info!(
            target: TARGET,
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

    /// Main command-processing loop: waits for `ActorCommand` messages and
    /// dispatches to `execute_turn` or `shutdown`. Exits on `Shutdown` or
    /// when the channel closes.
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

        // Channel closed without Shutdown — clean up anyway.
        let _ = self.shutdown().await;
    }

    /// Execute a single ACP prompt turn.
    ///
    /// Sends a `PromptRequest` and waits for the agent to finish. While
    /// waiting, a 15-second ticker logs progress snapshots (elapsed time,
    /// idle time, buffer size) so users can see long-running tasks making
    /// progress.
    ///
    /// On error, attempts to capture any partial output and includes it in
    /// the error message along with telemetry.
    async fn execute_turn(&mut self, task: String) -> Result<String> {
        self.client.begin_turn(&self.session_id).await;

        let prompt_request =
            PromptRequest::new(self.session_id.clone(), vec![ContentBlock::from(task)]);
        let mut prompt = Box::pin(self.connection.prompt(prompt_request));
        // Log progress every 15 seconds while the agent is thinking.
        let mut ticker = tokio::time::interval(Duration::from_secs(15));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ticker.tick().await;

        let response = loop {
            tokio::select! {
                result = &mut prompt => break result,
                _ = ticker.tick() => {
                    if let Some(snapshot) = self.client.turn_snapshot(&self.session_id).await {
                        info!(
                            target: TARGET,
                            elapsed_secs = snapshot.elapsed_secs,
                            idle_secs = snapshot.idle_secs,
                            updates_count = snapshot.updates_count,
                            buffer_bytes = snapshot.buffer_bytes,
                            last_update_kind = snapshot.last_update_kind.unwrap_or("none"),
                            "ACP turn still running"
                        );
                    } else {
                        info!(target: TARGET, "ACP turn still running");
                    }
                }
            }
        };

        match response {
            Ok(response) => Ok(self
                .client
                .take_turn_output(&self.session_id, response.stop_reason)
                .await),
            Err(err) => {
                // Attempt to salvage partial output even when the turn failed.
                let snapshot = self.client.turn_snapshot(&self.session_id).await;
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
                    if let Some(snapshot) = snapshot {
                        Err(anyhow!(
                            "ACP prompt failed: {}. Turn telemetry: elapsed={}s idle={}s updates={} buffer_bytes={} last_update={}",
                            err,
                            snapshot.elapsed_secs,
                            snapshot.idle_secs,
                            snapshot.updates_count,
                            snapshot.buffer_bytes,
                            snapshot.last_update_kind.unwrap_or("none")
                        ))
                    } else {
                        Err(anyhow!("ACP prompt failed: {}", err))
                    }
                } else if let Some(snapshot) = snapshot {
                    Err(anyhow!(
                        "ACP prompt failed: {}. Turn telemetry: elapsed={}s idle={}s updates={} buffer_bytes={} last_update={}. Partial output:\n{}",
                        err,
                        snapshot.elapsed_secs,
                        snapshot.idle_secs,
                        snapshot.updates_count,
                        snapshot.buffer_bytes,
                        snapshot.last_update_kind.unwrap_or("none"),
                        partial_output
                    ))
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

    /// Gracefully shut down the ACP subprocess.
    ///
    /// First closes all tracked terminal sessions via `SimpleClient`, then
    /// kills the main process if it is still running.
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

/// Resolve the session working directory, expanding relative paths against the
/// current working directory.
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

/// Build a `tokio::process::Command` for spawning an ACP agent process.
///
/// Configures the working directory, stdin/stdout/stderr pipes, environment
/// variables, and command-line arguments. Returns the `Command` along with
/// the resolved session working directory.
///
/// # Arguments
///
/// * `command_str` — Path or name of the ACP agent binary.
/// * `args` — Command-line arguments to pass to the agent.
/// * `cwd` — Optional working directory (relative paths are resolved against
///   the current working directory).
/// * `env` — Environment variables to set for the subprocess.
///
/// # Returns
///
/// A tuple of `(Command, session_cwd)` where `session_cwd` is the resolved,
/// absolute path of the working directory.
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

/// Join a `std::thread::JoinHandle` from an async context.
///
/// Uses `tokio::task::spawn_blocking` so the calling task is not blocked.
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

/// Sanitise an `agent_id` so it is safe for use as an OS thread name.
///
/// Replaces all characters that are not ASCII alphanumeric, `-`, or `_`
/// with `_`. Falls back to `"agent"` if the result would be empty.
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
