use std::time::{Duration, Instant};

/// Throttle helper for rate-limiting updates based on size and time intervals.
///
/// Used for streaming progress updates and tool hints to avoid overwhelming
/// the message bus with too many small updates.
#[derive(Debug, Clone)]
pub struct Throttle {
    min_chars: usize,
    min_interval: Duration,
    last_sent_at: Instant,
    last_sent_len: usize,
}

impl Throttle {
    /// Create a new throttle with minimum character delta and time interval.
    pub fn new(min_chars: usize, min_interval: Duration) -> Self {
        Self {
            min_chars,
            min_interval,
            last_sent_at: Instant::now(),
            last_sent_len: 0,
        }
    }

    /// Check if an update should be sent based on current content length.
    ///
    /// Returns true if either:
    /// - The content has grown by at least `min_chars` since last send
    /// - At least `min_interval` has elapsed since last send
    /// - This is the first update (last_sent_len is 0 and current_len > 0)
    pub fn should_send(&self, current_len: usize) -> bool {
        if current_len == 0 {
            return false;
        }

        // First update
        if self.last_sent_len == 0 {
            return true;
        }

        // No growth
        if current_len <= self.last_sent_len {
            return false;
        }

        let now = Instant::now();
        let len_delta = current_len.saturating_sub(self.last_sent_len);
        let time_ok = now.duration_since(self.last_sent_at) >= self.min_interval;
        let size_ok = len_delta >= self.min_chars;

        time_ok || size_ok
    }

    /// Mark that an update was sent at the current content length.
    pub fn mark_sent(&mut self, current_len: usize) {
        self.last_sent_at = Instant::now();
        self.last_sent_len = current_len;
    }

    /// Reset the throttle state (e.g., when starting a new stream).
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
    format!("{}…", truncated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn throttle_allows_first_update() {
        let throttle = Throttle::new(10, Duration::from_millis(100));
        assert!(throttle.should_send(1));
    }

    #[test]
    fn throttle_blocks_small_updates() {
        let mut throttle = Throttle::new(10, Duration::from_millis(100));
        throttle.mark_sent(5);
        assert!(!throttle.should_send(10)); // Only 5 chars delta, need 10
    }

    #[test]
    fn throttle_allows_large_updates() {
        let mut throttle = Throttle::new(10, Duration::from_millis(100));
        throttle.mark_sent(5);
        assert!(throttle.should_send(20)); // 15 chars delta, exceeds 10
    }

    #[test]
    fn throttle_allows_after_interval() {
        let mut throttle = Throttle::new(10, Duration::from_millis(50));
        throttle.mark_sent(5);
        thread::sleep(Duration::from_millis(60));
        assert!(throttle.should_send(7)); // Only 2 chars delta, but time elapsed
    }

    #[test]
    fn throttle_reset_clears_state() {
        let mut throttle = Throttle::new(10, Duration::from_millis(100));
        throttle.mark_sent(100);
        throttle.reset();
        assert!(throttle.should_send(1));
    }

    #[test]
    fn truncate_text_preserves_short_strings() {
        assert_eq!(truncate_text("hello", 10), "hello");
    }

    #[test]
    fn truncate_text_truncates_long_strings() {
        let result = truncate_text("hello world", 5);
        assert_eq!(result, "hello…");
    }

    #[test]
    fn truncate_text_handles_unicode() {
        let result = truncate_text("你好世界", 2);
        assert_eq!(result, "你好…");
    }
}
