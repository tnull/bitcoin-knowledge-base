pub mod commits;
pub mod delving;
pub mod github;
pub mod irc;
pub mod mail_archive;
pub mod mailing_list;
pub mod optech;
pub mod specs;

use anyhow::Result;
use async_trait::async_trait;

use std::time::Duration;

use bkb_core::model::{Document, Reference};

use crate::rate_limiter::RateLimiter;

/// A page of results from a sync source.
pub struct SyncPage {
	/// Documents fetched in this page.
	pub documents: Vec<Document>,
	/// References extracted during fetch (source-level, e.g., parent_id links).
	pub references: Vec<Reference>,
	/// Cursor for the next page, or `None` if this source is caught up.
	pub next_cursor: Option<String>,
}

/// Trait implemented by each data source adapter.
///
/// Each call to `fetch_page` fetches one page of updates from the source.
/// The same codepath handles both initial sync (many pages) and incremental
/// sync (usually one page).
#[async_trait]
pub trait SyncSource: Send + Sync {
	/// Fetch one page of updates starting from the given cursor.
	///
	/// Returns the fetched documents, any source-level references, and
	/// an optional cursor for the next page.
	async fn fetch_page(
		&self, cursor: Option<&str>, rate_limiter: &RateLimiter,
	) -> Result<SyncPage>;

	/// The base polling interval for this source.
	fn poll_interval(&self) -> Duration;

	/// A human-readable name for this source (for logging).
	fn name(&self) -> &str;
}
