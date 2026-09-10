use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::header::HeaderMap;
use tracing::debug;

/// Ask the scheduler to retry later, without holding up other sources.
#[derive(Debug, thiserror::Error)]
#[error("request deferred for {0:?}")]
pub struct Deferred(pub Duration);

/// Rate limiter for GitHub API requests.
///
/// Tracks `X-RateLimit-Remaining` and `X-RateLimit-Reset` from response
/// headers. Blocks `acquire()` when remaining requests drop below the
/// safety margin.
pub struct RateLimiter {
	limit: AtomicU32,
	remaining: AtomicU32,
	reset_at: AtomicU64,
	safety_margin: u32,
}

impl RateLimiter {
	pub fn new(safety_margin: u32) -> Self {
		Self {
			limit: AtomicU32::new(5000),
			remaining: AtomicU32::new(5000), // Assume full quota initially
			reset_at: AtomicU64::new(0),
			safety_margin,
		}
	}

	/// Check permission without blocking the single-threaded ingestion scheduler.
	pub async fn acquire(&self) -> Result<(), Deferred> {
		let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
		let reset = self.reset_at.load(Ordering::Relaxed);
		let limit = self.limit.load(Ordering::Relaxed);
		let margin = self.safety_margin.min(limit / 10);
		if self.remaining.load(Ordering::Relaxed) <= margin && reset > now {
			return Err(Deferred(Duration::from_secs(reset - now + 1)));
		}
		if reset <= now {
			self.remaining.store(limit, Ordering::Relaxed);
		}
		Ok(())
	}

	/// Update rate limit state from GitHub API response headers.
	pub fn update_from_response(&self, headers: &HeaderMap) {
		if let Some(limit) = headers
			.get("x-ratelimit-limit")
			.and_then(|h| h.to_str().ok())
			.and_then(|h| h.parse().ok())
		{
			self.limit.store(limit, Ordering::Relaxed);
		}
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
	async fn exhausted_quota_must_not_block_the_scheduler() {
		let rl = RateLimiter::new(200);
		rl.remaining.store(0, Ordering::Relaxed);
		let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
		rl.reset_at.store(now + 3600, Ordering::Relaxed);
		let result = tokio::time::timeout(Duration::from_millis(30), rl.acquire()).await;
		assert!(
			result.is_ok(),
			"rate-limited requests must defer without sleeping in the scheduler"
		);
	}

	#[tokio::test]
	async fn test_acquire_when_plenty_remaining() {
		let rl = RateLimiter::new(200);
		// Should return immediately when we have plenty of remaining requests
		rl.acquire().await.unwrap();
	}

	#[tokio::test]
	async fn test_acquire_when_reset_passed() {
		let rl = RateLimiter::new(200);
		rl.remaining.store(0, Ordering::Relaxed);
		// Reset time is in the past
		rl.reset_at.store(0, Ordering::Relaxed);
		rl.acquire().await.unwrap();
		assert_eq!(rl.remaining(), 5000);
	}
}
