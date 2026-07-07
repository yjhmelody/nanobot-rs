//! ReAct (Reason-Act-Observe) execution engine.
//!
//! This module implements the core agent reasoning loop. It is decomposed
//! into four sub-modules:
//!
//! | Module | Responsibility |
//! |--------|----------------|
//! | [`planner`] | Queries the LLM model and parses responses |
//! | [`executor`] | Orchestrates the plan-act-observe state machine |
//! | [`state`] | State-machine types (`LoopState`, `LoopOutcome`, etc.) |
//! | [`tool_runner`] | Executes individual tool calls |
//!
//! # The ReAct Loop
//!
//! 1. **Query Model** — Send the conversation to the LLM and stream the
//!    response.
//! 2. **If tool calls** — Execute each tool (with cancellation checks
//!    between calls), append observations, return to step 1.
//! 3. **If final answer** — Return the response and exit.
//! 4. **If truncated** (finish_reason == "length") — Continue the loop
//!    with the partial response appended.
//!
//! The loop is bounded by `max_iterations` and can be preempted at any
//! iteration or tool boundary via an [`AtomicBool`] cancellation signal.

mod executor;
mod planner;
mod state;
mod tool_runner;

pub use executor::{ExecutionContext, ReActExecutor};
pub use planner::{ModelConfig, Planner, PlannerResponse, ProgressEmitter};
pub use state::{LoopExitReason, LoopOutcome, LoopState, StepResult};
pub use tool_runner::{ToolObservation, ToolRunner};

/// Logging target for all ReAct sub-modules.
pub const TARGET: &str = "nanobot::react";
