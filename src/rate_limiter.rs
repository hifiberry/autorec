//! Generic rate limiter with adaptive backoff.
//!
//! Used by songrec (Shazam), MusicBrainz, and Discogs API clients
//! to stay within their respective rate limits.

use std::time::{Duration, Instant};
use std::thread;

/// A rate limiter that enforces a minimum interval between requests
/// with optional adaptive backoff on failures.
pub struct RateLimiter {
    name: String,
    last_request: Option<Instant>,
    current_interval: Duration,
    base_interval: Duration,
    max_interval: Duration,
    success_count: u32,
    successes_to_reduce: u32,
}

impl RateLimiter {
    /// Create a new rate limiter.
    ///
    /// * `name` — label for log messages (e.g. "songrec", "MusicBrainz", "Discogs")
    /// * `base_interval` — minimum time between requests
    /// * `max_interval` — upper bound after repeated failures
    /// * `successes_to_reduce` — how many consecutive successes before halving the interval
    ///   (set to 0 to disable adaptive backoff reduction)
    pub fn new(name: &str, base_interval: Duration, max_interval: Duration, successes_to_reduce: u32) -> Self {
        RateLimiter {
            name: name.to_string(),
            last_request: None,
            current_interval: base_interval,
            base_interval,
            max_interval,
            success_count: 0,
            successes_to_reduce,
        }
    }

    /// Convenience: create a rate limiter from a base interval in seconds.
    /// Max interval = 16× base, reduce after 10 successes.
    pub fn from_secs(name: &str, secs: u64) -> Self {
        let base = Duration::from_secs(secs);
        Self::new(name, base, base * 16, 10)
    }

    /// Convenience: create a rate limiter from a base interval in milliseconds.
    /// Max interval = 16× base, reduce after 10 successes.
    pub fn from_millis(name: &str, millis: u64) -> Self {
        let base = Duration::from_millis(millis);
        Self::new(name, base, base * 16, 10)
    }

    /// Sleep if not enough time has elapsed since the last request.
    /// Must be called *before* making a request.
    pub fn wait_if_needed(&mut self) {
        if let Some(last) = self.last_request {
            let elapsed = last.elapsed();
            if elapsed < self.current_interval {
                let wait_time = self.current_interval - elapsed;
                println!("  [{}] Rate limiting: waiting {:.1}s...",
                         self.name, wait_time.as_secs_f64());
                thread::sleep(wait_time);
            }
        }
        self.last_request = Some(Instant::now());
    }

    /// Report a successful request.  After enough consecutive successes
    /// the interval is halved (down to the base).
    pub fn report_success(&mut self) {
        if self.successes_to_reduce == 0 {
            return; // adaptive reduction disabled
        }

        self.success_count += 1;

        if self.success_count >= self.successes_to_reduce && self.current_interval > self.base_interval {
            let new_interval = self.current_interval / 2;
            if new_interval >= self.base_interval {
                self.current_interval = new_interval;
            } else {
                self.current_interval = self.base_interval;
            }
            println!("  [{}] Rate limit reduced to {:.1}s after {} successes",
                     self.name, self.current_interval.as_secs_f64(), self.success_count);
            self.success_count = 0;
        }
    }

    /// Report a failed request.  Doubles the interval (up to max).
    pub fn report_failure(&mut self) {
        let new_interval = self.current_interval * 2;
        if new_interval <= self.max_interval {
            self.current_interval = new_interval;
        } else {
            self.current_interval = self.max_interval;
        }
        println!("  [{}] Rate limit increased to {:.1}s due to error",
                 self.name, self.current_interval.as_secs_f64());
        self.success_count = 0;
    }
}
