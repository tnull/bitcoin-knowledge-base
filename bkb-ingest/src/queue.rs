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

		// If the job has a cursor but sync_state was deleted (e.g. via an
		// admin reset), clear the in-memory cursor so we start from scratch
		// instead of re-fetching only the tail page.
		if job.cursor.is_some() {
			match self.store.get_sync_state(&job.source_id).await {
				Ok(Some(state)) if state.last_cursor.is_some() => {},
				Ok(_) => {
					info!(
						source = %source_name,
						old_cursor = ?job.cursor,
						"sync_state was reset, clearing in-memory cursor"
					);
					job.cursor = None;
				},
				Err(e) => {
					warn!(
						source = %source_name,
						error = %e,
						"failed to check sync_state, keeping current cursor"
					);
				},
			}
		}

		info!(source = %source_name, cursor = ?job.cursor, "executing sync job");

		let started = std::time::Instant::now();

		let result = async {
			let page = job.source.fetch_page(job.cursor.as_deref(), &self.rate_limiter).await?;
			let cursor =
				store_page(&self.store, &job.source_id, job.cursor.as_deref(), &page).await?;
			Ok::<_, anyhow::Error>((page, cursor))
		}
		.await;
		match result {
			Ok((page, cursor)) => {
				let doc_count = page.documents.len();
				let paginating = page.next_cursor.is_some();
				job.cursor = cursor;
				job.next_run = Instant::now()
					+ if paginating {
						Duration::ZERO
					} else {
						adaptive_interval(job.base_interval, doc_count)
					};
				job.retry_count = 0;
				info!(source = %source_name, documents = doc_count, paginating, "page committed");

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
}

/// Enrich and commit a page using the same atomic path in scheduled and CLI ingestion.
pub async fn store_page(
	store: &SqliteStore, source_id: &str, previous: Option<&str>, page: &crate::sources::SyncPage,
) -> Result<Option<String>> {
	let cursor = page
		.next_cursor
		.clone()
		.or_else(|| page.checkpoint.clone())
		.or_else(|| previous.map(str::to_owned));
	let mut references = page.references.clone();
	let mut mentions = Vec::new();
	for doc in &page.documents {
		if let Some(body) = &doc.body {
			let output = enrichment::enrich(&doc.id, body, doc.source_repo.as_deref());
			for reference in output.references {
				if !references.iter().any(|r| {
					r.from_doc_id == reference.from_doc_id
						&& r.to_doc_id == reference.to_doc_id
						&& r.to_external == reference.to_external
						&& r.ref_type == reference.ref_type
				}) {
					references.push(reference);
				}
			}
			mentions.extend(
				output
					.concept_tags
					.into_iter()
					.map(|(slug, confidence)| (doc.id.clone(), slug, confidence)),
			);
		}
		if let Some(parent) = &doc.parent_id {
			references.push(bkb_core::model::Reference {
				id: None,
				from_doc_id: doc.id.clone(),
				to_doc_id: Some(parent.clone()),
				ref_type: bkb_core::model::RefType::RepliesTo,
				to_external: None,
				context: None,
			});
		}
	}
	let state = SyncState {
		source_id: source_id.into(),
		source_type: source_id.split(':').next().unwrap_or("unknown").into(),
		source_repo: None,
		last_cursor: cursor.clone(),
		last_synced_at: Some(Utc::now()),
		next_run_at: None,
		status: SyncStatus::Ok,
		error_message: None,
		retry_count: 0,
		items_found: page.documents.len() as i32,
	};
	store.commit_page(&page.documents, &references, &mentions, &state).await?;
	Ok(cursor)
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
	use crate::sources::SyncPage;
	use bkb_core::model::{Document, SourceType};
	use bkb_core::store::KnowledgeStore;

	struct ConflictingPage;
	#[async_trait::async_trait]
	impl SyncSource for ConflictingPage {
		async fn fetch_page(&self, _: Option<&str>, _: &RateLimiter) -> Result<SyncPage> {
			let doc = Document {
				id: "github_issue:test/repo:1".into(),
				source_type: SourceType::GithubIssue,
				source_repo: Some("test/repo".into()),
				source_id: "1".into(),
				title: None,
				body: None,
				author: None,
				author_id: None,
				created_at: Utc::now(),
				updated_at: None,
				parent_id: None,
				metadata: None,
				seq: None,
			};
			let mut conflicting = doc.clone();
			conflicting.id = "conflicting".into();
			Ok(SyncPage {
				checkpoint: None,
				documents: vec![doc, conflicting],
				references: vec![],
				next_cursor: Some("next".into()),
			})
		}
		fn poll_interval(&self) -> Duration {
			Duration::from_secs(3600)
		}
		fn name(&self) -> &str {
			"test:failure"
		}
	}

	#[tokio::test]
	async fn failed_page_must_not_advance_or_partially_commit() {
		let store = Arc::new(SqliteStore::open_in_memory().unwrap());
		let queue = JobQueue::new(Arc::new(RateLimiter::new(0)), store.clone(), None);
		queue
			.execute_job(SyncJob {
				source_id: "test:failure".into(),
				source: Box::new(ConflictingPage),
				priority: Priority::Low,
				cursor: None,
				next_run: Instant::now(),
				retry_count: 0,
				base_interval: Duration::from_secs(3600),
			})
			.await;
		assert!(
			queue.jobs.lock().await.peek().unwrap().cursor.is_none(),
			"failed page must not advance its cursor"
		);
		assert!(
			store.get_document("github_issue:test/repo:1").await.unwrap().is_none(),
			"failed page must roll back all documents"
		);
	}

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
