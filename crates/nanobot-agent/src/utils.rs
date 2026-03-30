use std::time::{Duration, Instant};

/// Throttle helper for rate-limiting updates based on size and time intervals.
#[derive(Debug, Clone)]
pub struct Throttle {
    min_chars: usize,
    min_interval: Duration,
    last_sent_at: Instant,
    last_sent_len: usize,
}

impl Throttle {
    pub fn new(min_chars: usize, min_interval: Duration) -> Self {
        Self {
            min_chars,
            min_interval,
            last_sent_at: Instant::now(),
            last_sent_len: 0,
        }
    }

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
        time_ok || size_ok
    }

    pub fn mark_sent(&mut self, current_len: usize) {
        self.last_sent_at = Instant::now();
        self.last_sent_len = current_len;
    }

    pub fn reset(&mut self) {
        self.last_sent_at = Instant::now();
        self.last_sent_len = 0;
    }
}

/// Truncate text to a maximum character length, adding ellipsis if truncated.
pub fn truncate_text(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text.to_string();
    }
    let truncated: String = text.chars().take(max_chars).collect();
    format!("{}\u{2026}", truncated)
}

/// Truncate and preview text for display.
pub fn preview_text(text: &str, max_chars: usize) -> String {
    truncate_text(text, max_chars)
}
