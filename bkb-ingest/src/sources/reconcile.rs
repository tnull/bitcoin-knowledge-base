//! Resumable local backfill for databases created before cross-forge support.
use super::{SyncPage, SyncSource};
use crate::rate_limiter::RateLimiter;
use anyhow::Result;
use async_trait::async_trait;
use bkb_store::sqlite::SqliteStore;
use std::{sync::Arc, time::Duration};

pub struct ReconcileSyncSource {
	store: Arc<SqliteStore>,
}
impl ReconcileSyncSource {
	pub fn new(store: Arc<SqliteStore>) -> Self {
		Self { store }
	}
}
#[async_trait]
impl SyncSource for ReconcileSyncSource {
	async fn fetch_page(&self, cursor: Option<&str>, _: &RateLimiter) -> Result<SyncPage> {
		let after = cursor.map(str::parse::<i64>).transpose()?.unwrap_or(0);
		let rows = self.store.reconciliation_page(after).await?;
		let next = rows.last().map(|r| r.0).unwrap_or(after).to_string();
		let more = rows.len() == 200;
		Ok(SyncPage {
			documents: rows.into_iter().map(|(_, doc)| doc).collect(),
			references: vec![],
			next_cursor: more.then(|| next.clone()),
			checkpoint: (!more).then_some(next),
		})
	}
	fn name(&self) -> &str {
		"reconcile:v1"
	}
	fn poll_interval(&self) -> Duration {
		Duration::from_secs(7 * 24 * 3600)
	}
}
