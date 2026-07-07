//! Unified streaming infrastructure for LLM providers.
//!
//! This module provides a provider-agnostic streaming framework that converts
//! upstream-specific streaming formats into a unified [`StreamEvent`] stream.
//!
//! # Architecture
//!
//! ```text
//! HTTP Response (raw bytes)
//!     │
//!     ▼
//! StreamAdapter (e.g., SseAdapter, OpenAiAdapter)
//!     │  parses SSE/bytes into provider-specific events
//!     ▼
//! StreamEvent stream (unified)
//!     │
//!     ├── TextDelta / ThinkingDelta / SignatureDelta
//!     ├── ToolCallStart / ToolCallArgumentsDelta / ToolCallEnd
//!     ├── UsageUpdate / FinishReasonUpdate
//!     └── Done / Error
//!     │
//!     ▼
//! StreamAccumulator (optional)
//!     │  collects events into LLMResponse
//!     ▼
//! LLMResponse
//! ```
//!
//! # Key Types
//!
//! - [`StreamEvent`] — All possible streaming events (text deltas, tool calls, etc.)
//! - [`StreamError`] — Error types specific to streaming
//! - [`StreamResponse`] — Type alias for `Pin<Box<dyn Stream<Item = Result<StreamEvent, StreamError>>>>`
//! - [`StreamAdapter`] — Trait for converting HTTP responses to event streams
//! - [`SseAdapter`] — Implementation for Anthropic SSE format
//! - [`OpenAiAdapter`] — Implementation for OpenAI Responses SSE format
//! - [`StreamAccumulator`] — Accumulates events into a complete [`LLMResponse`]

pub mod accumulator;
pub mod adapter;
pub mod events;
pub mod openai_adapter;
pub mod sse_adapter;

pub use accumulator::StreamAccumulator;
pub use adapter::StreamAdapter;
pub use events::{StreamError, StreamEvent, StreamResponse};
pub use openai_adapter::OpenAiAdapter;
pub use sse_adapter::SseAdapter;
