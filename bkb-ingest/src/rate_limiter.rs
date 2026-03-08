use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::header::HeaderMap;
use tracing::{debug, warn};

/// Rate limiter for GitHub API requests.
///
/// Tracks `X-RateLimit-Remaining` and `X-RateLimit-Reset` from response
/// headers. Blocks `acquire()` when remaining requests drop below the
/// safety margin.
pub struct RateLimiter {
	remaining: AtomicU32,
	reset_at: AtomicU64,
	safety_margin: u32,
}

impl RateLimiter {
	pub fn new(safety_margin: u32) -> Self {
		Self {
			remaining: AtomicU32::new(5000), // Assume full quota initially
			reset_at: AtomicU64::new(0),
			safety_margin,
		}
	}

	/// Wait until a request is permitted.
	pub async fn acquire(&self) {
		loop {
			let remaining = self.remaining.load(Ordering::Relaxed);
			if remaining > self.safety_margin {
				return;
			}

			let reset_at = self.reset_at.load(Ordering::Relaxed);
			let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();

			if reset_at <= now {
				// Reset time has passed; assume quota is refreshed
				self.remaining.store(5000, Ordering::Relaxed);
				return;
			}

			let wait_secs = reset_at - now + 1;
			warn!(remaining, reset_in_secs = wait_secs, "rate limit low, waiting for reset");
			tokio::time::sleep(Duration::from_secs(wait_secs)).await;

			// After sleeping, assume refreshed
			self.remaining.store(5000, Ordering::Relaxed);
		}
	}

	/// Update rate limit state from GitHub API response headers.
	pub fn update_from_response(&self, headers: &HeaderMap) {
		if let Some(remaining) = headers
			.get("x-ratelimit-remaining")
			.and_then(|v| v.to_str().ok())
			.and_then(|s| s.parse::<u32>().ok())
		{
			self.remaining.store(remaining, Ordering::Relaxed);
			debug!(remaining, "updated rate limit remaining");
		}

		if let Some(reset) = headers
			.get("x-ratelimit-reset")
			.and_then(|v| v.to_str().ok())
			.and_then(|s| s.parse::<u64>().ok())
		{
			self.reset_at.store(reset, Ordering::Relaxed);
		}
	}

	/// Get the current remaining request count.
	pub fn remaining(&self) -> u32 {
		self.remaining.load(Ordering::Relaxed)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_initial_state() {
		let rl = RateLimiter::new(200);
		assert_eq!(rl.remaining(), 5000);
	}

	#[test]
	fn test_update_from_headers() {
		let rl = RateLimiter::new(200);
		let mut headers = HeaderMap::new();
		headers.insert("x-ratelimit-remaining", "4500".parse().unwrap());
		headers.insert("x-ratelimit-reset", "1700000000".parse().unwrap());
		rl.update_from_response(&headers);
		assert_eq!(rl.remaining(), 4500);
	}

	#[tokio::test]
	async fn test_acquire_when_plenty_remaining() {
		let rl = RateLimiter::new(200);
		// Should return immediately when we have plenty of remaining requests
		rl.acquire().await;
	}

	#[tokio::test]
	async fn test_acquire_when_reset_passed() {
		let rl = RateLimiter::new(200);
		rl.remaining.store(0, Ordering::Relaxed);
		// Reset time is in the past
		rl.reset_at.store(0, Ordering::Relaxed);
		rl.acquire().await;
		assert_eq!(rl.remaining(), 5000);
	}
}
