use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use chrono::Utc;
use tokio::sync::Mutex;
use tokio::time::Instant;
use tracing::{debug, error, info, warn};

use bkb_core::model::{SyncState, SyncStatus};
use bkb_store::sqlite::SqliteStore;

use crate::enrichment;
use crate::metrics::Metrics;
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
	/// Counter for pages fetched since last cursor persist.
	/// Used to periodically save progress during long pagination runs.
	pub pages_since_persist: u32,
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

/// How often to persist the cursor during pagination (every N pages).
/// This ensures progress survives server restarts for slow-paginating
/// sources like IRC logs and BitcoinTalk.
const PERSIST_EVERY_N_PAGES: u32 = 10;

/// Job queue that schedules and runs sync sources.
pub struct JobQueue {
	jobs: Mutex<BinaryHeap<SyncJob>>,
	rate_limiter: Arc<RateLimiter>,
	store: Arc<SqliteStore>,
	metrics: Option<Arc<Metrics>>,
}

impl JobQueue {
	pub fn new(
		rate_limiter: Arc<RateLimiter>, store: Arc<SqliteStore>, metrics: Option<Arc<Metrics>>,
	) -> Self {
		Self { jobs: Mutex::new(BinaryHeap::new()), rate_limiter, store, metrics }
	}

	/// Add a sync job to the queue.
	///
	/// If the job has no cursor, attempts to load the last persisted cursor
	/// from `sync_state` so that incremental sync resumes where it left off.
	pub async fn add_job(&self, mut job: SyncJob) {
		if job.cursor.is_none() {
			match self.store.get_sync_state(&job.source_id).await {
				Ok(Some(state)) if state.last_cursor.is_some() => {
					debug!(
						source = %job.source_id,
						cursor = ?state.last_cursor,
						"restored persisted cursor from sync_state"
					);
					job.cursor = state.last_cursor;
				},
				Ok(_) => {},
				Err(e) => {
					warn!(
						source = %job.source_id,
						error = %e,
						"failed to load sync_state, starting from scratch"
					);
				},
			}
		}
		if let Some(ref metrics) = self.metrics {
			metrics.register_job(&job.source_id);
		}
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

		let started = std::time::Instant::now();

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
				let paginating = page.next_cursor.is_some();
				if let Some(next_cursor) = page.next_cursor {
					// More pages to fetch -- run immediately
					job.cursor = Some(next_cursor);
					job.next_run = Instant::now();
					job.retry_count = 0;

					// Periodically persist cursor during pagination so that
					// progress survives server restarts.
					job.pages_since_persist += 1;
					if job.pages_since_persist >= PERSIST_EVERY_N_PAGES {
						self.persist_cursor(&job.source_id, job.cursor.as_deref(), doc_count).await;
						job.pages_since_persist = 0;
						debug!(
							source = %source_name,
							cursor = ?job.cursor,
							"persisted cursor checkpoint during pagination"
						);
					}
				} else {
					// Caught up -- persist cursor to sync_state so we resume
					// from here on restart / next cycle
					self.persist_cursor(&job.source_id, job.cursor.as_deref(), doc_count).await;

					// Apply adaptive scheduling
					let interval = adaptive_interval(job.base_interval, doc_count);
					job.next_run = Instant::now() + interval;
					job.retry_count = 0;

					info!(
						source = %source_name,
						cursor = ?job.cursor,
						next_in_secs = interval.as_secs(),
						"sync cycle complete, scheduling next run"
					);
				}

				// Record metrics
				if let Some(ref metrics) = self.metrics {
					metrics.record_job_run(
						&job.source_id,
						started.elapsed(),
						doc_count as u32,
						job.base_interval,
						None,
						paginating,
					);
				}
			},
			Err(e) => {
				// Record metrics for failed run
				if let Some(ref metrics) = self.metrics {
					metrics.record_job_run(
						&job.source_id,
						started.elapsed(),
						0,
						job.base_interval,
						Some(e.to_string()),
						false,
					);
				}

				error!(source = %source_name, error = ?e, "sync job failed");

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

	/// Persist the current cursor to `sync_state` so it survives restarts.
	async fn persist_cursor(&self, source_id: &str, cursor: Option<&str>, items_found: usize) {
		let state = SyncState {
			source_id: source_id.to_string(),
			source_type: source_id.split(':').next().unwrap_or("unknown").to_string(),
			source_repo: None,
			last_cursor: cursor.map(String::from),
			last_synced_at: Some(Utc::now()),
			next_run_at: None,
			status: SyncStatus::Ok,
			error_message: None,
			retry_count: 0,
			items_found: items_found as i32,
		};

		if let Err(e) = self.store.update_sync_state(&state).await {
			warn!(
				source = %source_id,
				error = %e,
				"failed to persist sync cursor"
			);
		}
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
