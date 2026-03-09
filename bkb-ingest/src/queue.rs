use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::sync::Mutex;
use tokio::time::Instant;
use tracing::{error, info, warn};

use bkb_store::sqlite::SqliteStore;

use crate::enrichment;
use crate::rate_limiter::RateLimiter;
use crate::sources::SyncSource;

/// Priority level for sync jobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
	High = 3,
	Medium = 2,
	Low = 1,
}

/// A scheduled sync job.
pub struct SyncJob {
	pub source_id: String,
	pub source: Box<dyn SyncSource>,
	pub priority: Priority,
	pub cursor: Option<String>,
	pub next_run: Instant,
	pub retry_count: u32,
	pub base_interval: Duration,
}

impl Eq for SyncJob {}

impl PartialEq for SyncJob {
	fn eq(&self, other: &Self) -> bool {
		self.source_id == other.source_id && self.next_run == other.next_run
	}
}

impl Ord for SyncJob {
	fn cmp(&self, other: &Self) -> Ordering {
		// Reverse ordering so BinaryHeap gives us earliest next_run first
		other
			.next_run
			.cmp(&self.next_run)
			.then_with(|| (self.priority as u8).cmp(&(other.priority as u8)))
	}
}

impl PartialOrd for SyncJob {
	fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
		Some(self.cmp(other))
	}
}

/// Job queue that schedules and runs sync sources.
pub struct JobQueue {
	jobs: Mutex<BinaryHeap<SyncJob>>,
	rate_limiter: Arc<RateLimiter>,
	store: Arc<SqliteStore>,
}

impl JobQueue {
	pub fn new(rate_limiter: Arc<RateLimiter>, store: Arc<SqliteStore>) -> Self {
		Self { jobs: Mutex::new(BinaryHeap::new()), rate_limiter, store }
	}

	/// Add a sync job to the queue.
	pub async fn add_job(&self, job: SyncJob) {
		self.jobs.lock().await.push(job);
	}

	/// Run the job queue forever, processing one job at a time.
	pub async fn run(&self) -> Result<()> {
		info!("job queue started");

		loop {
			let job = {
				let mut jobs = self.jobs.lock().await;
				if let Some(job) = jobs.peek() {
					if job.next_run <= Instant::now() {
						jobs.pop()
					} else {
						None
					}
				} else {
					None
				}
			};

			if let Some(job) = job {
				self.execute_job(job).await;
			} else {
				// No jobs ready; sleep briefly and check again
				tokio::time::sleep(Duration::from_millis(500)).await;
			}
		}
	}

	/// Execute a single job: fetch one page, store results, re-enqueue.
	async fn execute_job(&self, mut job: SyncJob) {
		let source_name = job.source.name().to_string();
		info!(source = %source_name, cursor = ?job.cursor, "executing sync job");

		match job.source.fetch_page(job.cursor.as_deref(), &self.rate_limiter).await {
			Ok(page) => {
				let doc_count = page.documents.len();

				// Store documents and enrichment results
				for doc in &page.documents {
					if let Err(e) = self.store.upsert_document(doc).await {
						error!(doc_id = %doc.id, error = %e, "failed to upsert document");
						continue;
					}

					// Run enrichment pipeline on the document body
					if let Some(ref body) = doc.body {
						let output = enrichment::enrich(&doc.id, body, doc.source_repo.as_deref());
						// Delete old refs and insert new ones
						if let Err(e) = self.store.delete_refs_from(&doc.id).await {
							warn!(doc_id = %doc.id, error = %e, "failed to delete old refs");
						}
						for reference in &output.references {
							if let Err(e) = self.store.insert_reference(reference).await {
								warn!(doc_id = %doc.id, error = %e, "failed to insert reference");
							}
						}

						// Store concept tags
						if let Err(e) = self.store.delete_concept_mentions(&doc.id).await {
							warn!(doc_id = %doc.id, error = %e, "failed to delete old concept mentions");
						}
						for (slug, confidence) in &output.concept_tags {
							if let Err(e) =
								self.store.upsert_concept_mention(&doc.id, slug, *confidence).await
							{
								warn!(doc_id = %doc.id, concept = %slug, error = %e, "failed to store concept mention");
							}
						}
					}
				}

				// Store any source-level references
				for reference in &page.references {
					if let Err(e) = self.store.insert_reference(reference).await {
						warn!(error = %e, "failed to insert source reference");
					}
				}

				info!(source = %source_name, documents = doc_count, "page processed");

				// Determine next run
				if let Some(next_cursor) = page.next_cursor {
					// More pages to fetch -- run immediately
					job.cursor = Some(next_cursor);
					job.next_run = Instant::now();
					job.retry_count = 0;
				} else {
					// Caught up -- apply adaptive scheduling
					let interval = adaptive_interval(job.base_interval, doc_count);
					job.cursor = None; // Will use saved cursor from sync_state on next cycle
					job.next_run = Instant::now() + interval;
					job.retry_count = 0;

					info!(
						source = %source_name,
						next_in_secs = interval.as_secs(),
						"sync cycle complete, scheduling next run"
					);
				}
			},
			Err(e) => {
				error!(source = %source_name, error = %e, "sync job failed");

				// Exponential backoff on failure
				job.retry_count += 1;
				let backoff =
					Duration::from_secs((30 * (1u64 << job.retry_count.min(6))).min(3600));
				job.next_run = Instant::now() + backoff;

				warn!(
					source = %source_name,
					retry_count = job.retry_count,
					backoff_secs = backoff.as_secs(),
					"retrying after backoff"
				);
			},
		}

		self.jobs.lock().await.push(job);
	}
}

/// Compute the adaptive polling interval based on how many items were found.
fn adaptive_interval(base: Duration, items_found: usize) -> Duration {
	match items_found {
		0 => (base * 2).min(base * 4),
		1..=5 => base,
		_ => base / 2,
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_adaptive_interval_no_items() {
		let base = Duration::from_secs(3600);
		let interval = adaptive_interval(base, 0);
		assert_eq!(interval, Duration::from_secs(7200));
	}

	#[test]
	fn test_adaptive_interval_few_items() {
		let base = Duration::from_secs(3600);
		let interval = adaptive_interval(base, 3);
		assert_eq!(interval, base);
	}

	#[test]
	fn test_adaptive_interval_many_items() {
		let base = Duration::from_secs(3600);
		let interval = adaptive_interval(base, 10);
		assert_eq!(interval, Duration::from_secs(1800));
	}
}
