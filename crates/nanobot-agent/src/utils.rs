//! Utility helpers for the agent crate.
//!
//! Provides [`Throttle`] for rate-limiting progress updates and
//! re-exports `truncate_text` / `preview_text` from `nanobot-types`.

use std::time::{Duration, Instant};

/// Throttle helper for rate-limiting streaming progress updates.
///
/// Ensures that progress messages are not sent too frequently or with
/// too little new content, reducing bus noise during long LLM responses.
///
/// # Criteria
///
/// A send is allowed when all three conditions are met:
/// 1. The current content length is greater than the last sent length.
/// 2. At least `min_interval` time has elapsed since the last send.
/// 3. The difference in length is at least `min_chars`.
#[derive(Debug, Clone)]
pub struct Throttle {
    min_chars: usize,
    min_interval: Duration,
    last_sent_at: Instant,
    last_sent_len: usize,
}

impl Throttle {
    /// Creates a new `Throttle`.
    ///
    /// * `min_chars` — Minimum character delta before another send fires.
    /// * `min_interval` — Minimum time between sends.
    pub fn new(min_chars: usize, min_interval: Duration) -> Self {
        Self {
            min_chars,
            min_interval,
            last_sent_at: Instant::now(),
            last_sent_len: 0,
        }
    }

    /// Returns `true` if a new update should be sent based on the current
    /// accumulated content length.
    pub fn should_send(&self, current_len: usize) -> bool {
        if current_len == 0 {
            return false;
        }
        if self.last_sent_len == 0 {
            return true;
        }
        if current_len <= self.last_sent_len {
            return false;
        }
        let now = Instant::now();
        let len_delta = current_len.saturating_sub(self.last_sent_len);
        let time_ok = now.duration_since(self.last_sent_at) >= self.min_interval;
        let size_ok = len_delta >= self.min_chars;
        time_ok && size_ok
    }

    /// Records that a send was just performed at the given length.
    pub fn mark_sent(&mut self, current_len: usize) {
        self.last_sent_at = Instant::now();
        self.last_sent_len = current_len;
    }

    /// Resets the throttle state as if no sends have occurred.
    pub fn reset(&mut self) {
        self.last_sent_at = Instant::now();
        self.last_sent_len = 0;
    }

    /// Returns the content length at the last successful send.
    pub fn last_sent_len(&self) -> usize {
        self.last_sent_len
    }
}

/// Truncates text to a maximum character count, appending an ellipsis
/// if truncated.
pub use nanobot_types::text::truncate_text;

/// Truncates and previews text for debug-log display.
pub fn preview_text(text: &str, max_chars: usize) -> String {
    truncate_text(text, max_chars)
}
