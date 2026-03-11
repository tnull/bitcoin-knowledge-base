mod api;
mod config;
mod dashboard;
mod examples;
mod landing;
mod sources;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use clap::Parser;
use tokio::time::Instant;
use tracing::info;

use bkb_ingest::metrics::Metrics;
use bkb_ingest::queue::{JobQueue, Priority, SyncJob};
use bkb_ingest::rate_limiter::RateLimiter;
use bkb_ingest::repo_cache::RepoCache;
use bkb_ingest::sources::commits::GitCommitSyncSource;
use bkb_ingest::sources::delving::DelvingSyncSource;
use bkb_ingest::sources::github::{GitHubCommentSyncSource, GitHubIssueSyncSource};
use bkb_ingest::sources::irc::IrcLogSyncSource;
use bkb_ingest::sources::mail_archive::MailArchiveSyncSource;
use bkb_ingest::sources::mailing_list::MailingListSyncSource;
use bkb_ingest::sources::optech::OptechNewsletterSyncSource;
use bkb_ingest::sources::specs::{BipSyncSource, BlipSyncSource, BoltSyncSource};
use bkb_ingest::sources::SyncSource;
use bkb_store::sqlite::SqliteStore;

use crate::config::Config;

#[derive(Parser)]
#[command(name = "bkb-server", about = "Bitcoin Knowledge Base server")]
struct Cli {
	/// Path to the SQLite database file.
	#[arg(long, default_value = "bkb.db")]
	db: String,

	/// HTTP server bind address.
	#[arg(long, default_value = "127.0.0.1:3000")]
	bind: String,

	/// GitHub API token (also reads GITHUB_TOKEN env var).
	#[arg(long, env = "GITHUB_TOKEN")]
	github_token: Option<String>,

	/// Use a small development subset of sources for fast iteration.
	#[arg(long, env = "BKB_DEV_SUBSET")]
	dev_subset: bool,

	/// Skip ingestion and only run the HTTP API server.
	#[arg(long)]
	no_ingest: bool,

	/// Run a single source adapter and exit (for testing).
	/// Format: "github:owner/repo", "irc:channel", "delving", "mailing_list",
	/// "lightning_dev", "bips", "bolts", "blips", "optech".
	#[arg(long)]
	ingest_only: Option<String>,

	/// Maximum number of pages to fetch when using --ingest-only.
	#[arg(long, default_value = "1")]
	limit_pages: u32,

	/// Directory for cached git bare clones.
	#[arg(long, env = "BKB_CACHE_DIR", default_value = "~/.cache/bkb/repos")]
	cache_dir: String,

	/// Maximum total cache size in GB.
	#[arg(long, default_value = "40")]
	max_cache_gb: u64,

	/// Maximum single repo size in MB (skip larger repos).
	#[arg(long, default_value = "4096")]
	max_repo_size_mb: u64,

	/// Password for /metrics and /dashboard endpoints (HTTP Basic Auth).
	/// If unset, these routes are not registered.
	#[arg(long, env = "BKB_ADMIN_PASSWORD")]
	admin_password: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
	// Set SSL_CERT_FILE/SSL_CERT_DIR so that libgit2's statically-linked
	// OpenSSL can find the system CA certificates.
	// SAFETY: called once at startup before any threads are spawned.
	unsafe { openssl_probe::init_openssl_env_vars() };

	tracing_subscriber::fmt()
		.with_env_filter(
			tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
				"info,bkb_server=debug,bkb_ingest=debug,bkb_store=debug".into()
			}),
		)
		.init();

	let cli = Cli::parse();
	let config = Config::new(cli.dev_subset);

	info!(
		db = %cli.db,
		bind = %cli.bind,
		dev_subset = cli.dev_subset,
		version = env!("CARGO_PKG_VERSION"),
		git_hash = option_env!("BKB_GIT_HASH").unwrap_or("unknown"),
		"starting BKB server"
	);

	let store = Arc::new(SqliteStore::open(std::path::Path::new(&cli.db))?);
	info!("database opened");

	// Single-source ingest mode
	if let Some(ref source_spec) = cli.ingest_only {
		let repo_cache = Arc::new(create_repo_cache(&cli)?);
		return run_single_source(source_spec, &cli, &store, &repo_cache).await;
	}

	let bind_addr: std::net::SocketAddr = cli.bind.parse()?;

	// Build metrics and app state based on whether ingestion is enabled
	let (metrics, cache_dir_for_metrics) = if !cli.no_ingest {
		let cache_dir = expand_tilde(&cli.cache_dir);
		let max_cache_bytes = cli.max_cache_gb * 1024 * 1024 * 1024;
		(
			Some(Arc::new(Metrics::new(
				PathBuf::from(&cli.db),
				Some(cache_dir),
				Some(max_cache_bytes),
			))),
			true,
		)
	} else {
		(Some(Arc::new(Metrics::new(PathBuf::from(&cli.db), None, None))), false)
	};
	let _ = cache_dir_for_metrics;

	let app_state = api::AppState {
		store: Arc::clone(&store),
		metrics: metrics.clone(),
		admin_password: cli.admin_password.clone(),
	};

	// Start HTTP API server
	let api_handle = tokio::spawn(async move {
		if let Err(e) = api::serve(app_state, bind_addr).await {
			tracing::error!(error = %e, "API server failed");
		}
	});

	info!(addr = %bind_addr, "HTTP API server started");

	if !cli.no_ingest {
		// Start ingestion scheduler
		let repo_cache = Arc::new(create_repo_cache(&cli)?);
		let rate_limiter = Arc::new(RateLimiter::new(200));
		let queue = Arc::new(JobQueue::new(Arc::clone(&rate_limiter), Arc::clone(&store), metrics));

		// Register sync sources from config
		let repos = config.github_repos();
		for (owner, repo) in &repos {
			let token = cli.github_token.clone();

			// Issues/PRs source
			let issue_source = GitHubIssueSyncSource::new(owner, repo, token.clone());
			let issue_interval = issue_source.poll_interval();
			queue
				.add_job(SyncJob {
					source_id: format!("github:{}/{}:issues", owner, repo),
					source: Box::new(issue_source),
					priority: Priority::Medium,
					cursor: None,
					next_run: Instant::now(),
					retry_count: 0,
					base_interval: issue_interval,
				})
				.await;

			// Comments source
			let comment_source = GitHubCommentSyncSource::new(owner, repo, token);
			let comment_interval = comment_source.poll_interval();
			queue
				.add_job(SyncJob {
					source_id: format!("github:{}/{}:comments", owner, repo),
					source: Box::new(comment_source),
					priority: Priority::Low,
					cursor: None,
					next_run: Instant::now(),
					retry_count: 0,
					base_interval: comment_interval,
				})
				.await;

			info!(repo = %format!("{}/{}", owner, repo), "registered GitHub sync sources");
		}

		// Register IRC sources
		for channel in config.irc_channels() {
			let irc_source = IrcLogSyncSource::new(&channel);
			let irc_interval = irc_source.poll_interval();
			queue
				.add_job(SyncJob {
					source_id: format!("irc:{}", channel),
					source: Box::new(irc_source),
					priority: Priority::Low,
					cursor: None,
					next_run: Instant::now(),
					retry_count: 0,
					base_interval: irc_interval,
				})
				.await;
			info!(channel = %channel, "registered IRC sync source");
		}

		// Register Delving Bitcoin source
		if config.sync_delving() {
			let delving_source = DelvingSyncSource::new();
			let delving_interval = delving_source.poll_interval();
			queue
				.add_job(SyncJob {
					source_id: "delving:delvingbitcoin.org".to_string(),
					source: Box::new(delving_source),
					priority: Priority::Medium,
					cursor: None,
					next_run: Instant::now(),
					retry_count: 0,
					base_interval: delving_interval,
				})
				.await;
			info!("registered Delving Bitcoin sync source");
		}

		// Register mailing list sources
		{
			let ml_source = MailingListSyncSource::new();
			let ml_interval = ml_source.poll_interval();
			queue
				.add_job(SyncJob {
					source_id: "mailing_list:bitcoindev".to_string(),
					source: Box::new(ml_source),
					priority: Priority::Medium,
					cursor: None,
					next_run: Instant::now(),
					retry_count: 0,
					base_interval: ml_interval,
				})
				.await;
			info!("registered bitcoindev mailing list sync source");

			let ld_source = MailArchiveSyncSource::new(
				"lightning-dev@lists.linuxfoundation.org",
				"lightning-dev",
			);
			let ld_interval = ld_source.poll_interval();
			queue
				.add_job(SyncJob {
					source_id: "mail_archive:lightning-dev".to_string(),
					source: Box::new(ld_source),
					priority: Priority::Low,
					cursor: None,
					next_run: Instant::now(),
					retry_count: 0,
					base_interval: ld_interval,
				})
				.await;
			info!("registered lightning-dev mail-archive sync source");
		}

		// Register BIP/BOLT/bLIP spec sources
		{
			let bip_source = BipSyncSource::new(cli.github_token.clone());
			let bip_interval = bip_source.poll_interval();
			queue
				.add_job(SyncJob {
					source_id: "specs:bips".to_string(),
					source: Box::new(bip_source),
					priority: Priority::Low,
					cursor: None,
					next_run: Instant::now(),
					retry_count: 0,
					base_interval: bip_interval,
				})
				.await;
			info!("registered BIP sync source");

			let bolt_source = BoltSyncSource::new(cli.github_token.clone());
			let bolt_interval = bolt_source.poll_interval();
			queue
				.add_job(SyncJob {
					source_id: "specs:bolts".to_string(),
					source: Box::new(bolt_source),
					priority: Priority::Low,
					cursor: None,
					next_run: Instant::now(),
					retry_count: 0,
					base_interval: bolt_interval,
				})
				.await;
			info!("registered BOLT sync source");

			let blip_source = BlipSyncSource::new(cli.github_token.clone());
			let blip_interval = blip_source.poll_interval();
			queue
				.add_job(SyncJob {
					source_id: "specs:blips".to_string(),
					source: Box::new(blip_source),
					priority: Priority::Low,
					cursor: None,
					next_run: Instant::now(),
					retry_count: 0,
					base_interval: blip_interval,
				})
				.await;
			info!("registered bLIP sync source");
		}

		// Register Optech newsletter source
		{
			let optech_source = OptechNewsletterSyncSource::new(cli.github_token.clone());
			let optech_interval = optech_source.poll_interval();
			queue
				.add_job(SyncJob {
					source_id: "optech:newsletters".to_string(),
					source: Box::new(optech_source),
					priority: Priority::Low,
					cursor: None,
					next_run: Instant::now(),
					retry_count: 0,
					base_interval: optech_interval,
				})
				.await;
			info!("registered Optech newsletter sync source");
		}

		// Register git commit sources per repo
		for (owner, repo) in &repos {
			let commit_source = GitCommitSyncSource::new(
				owner,
				repo,
				Arc::clone(&repo_cache),
				cli.github_token.clone(),
			);
			let commit_interval = commit_source.poll_interval();
			queue
				.add_job(SyncJob {
					source_id: format!("commits:{}/{}", owner, repo),
					source: Box::new(commit_source),
					priority: Priority::Low,
					cursor: None,
					next_run: Instant::now(),
					retry_count: 0,
					base_interval: commit_interval,
				})
				.await;
		}
		info!(repos = repos.len(), "registered git commit sync sources");

		// +6 for mailing lists (bitcoindev + lightning-dev), BIPs, BOLTs, bLIPs, Optech
		let total_sources = repos.len() * 3 // issues + comments + commits
			+ config.irc_channels().len()
			+ if config.sync_delving() { 1 } else { 0 }
			+ 5;
		info!(sources = total_sources, "ingestion scheduler starting");

		let queue_handle = tokio::spawn(async move {
			if let Err(e) = queue.run().await {
				tracing::error!(error = %e, "job queue failed");
			}
		});

		tokio::select! {
			r = api_handle => { r?; },
			r = queue_handle => { r?; },
		}
	} else {
		info!("ingestion disabled (--no-ingest)");
		api_handle.await?;
	}

	Ok(())
}

/// Run a single source adapter for a limited number of pages, then exit.
async fn run_single_source(
	spec: &str, cli: &Cli, store: &Arc<SqliteStore>, repo_cache: &Arc<RepoCache>,
) -> Result<()> {
	let rate_limiter = Arc::new(RateLimiter::new(200));
	let token = cli.github_token.clone();

	let source: Box<dyn SyncSource> = if let Some(rest) = spec.strip_prefix("github:") {
		let parts: Vec<&str> = rest.splitn(2, '/').collect();
		if parts.len() != 2 {
			anyhow::bail!("invalid github spec, expected 'github:owner/repo'");
		}
		Box::new(GitHubIssueSyncSource::new(parts[0], parts[1], token))
	} else if let Some(rest) = spec.strip_prefix("commits:") {
		let parts: Vec<&str> = rest.splitn(2, '/').collect();
		if parts.len() != 2 {
			anyhow::bail!("invalid commits spec, expected 'commits:owner/repo'");
		}
		Box::new(GitCommitSyncSource::new(parts[0], parts[1], Arc::clone(repo_cache), token))
	} else if let Some(channel) = spec.strip_prefix("irc:") {
		Box::new(IrcLogSyncSource::new(channel))
	} else if spec == "delving" {
		Box::new(DelvingSyncSource::new())
	} else if spec == "mailing_list" {
		Box::new(MailingListSyncSource::new())
	} else if spec == "lightning_dev" {
		Box::new(MailArchiveSyncSource::new(
			"lightning-dev@lists.linuxfoundation.org",
			"lightning-dev",
		))
	} else if spec == "bips" {
		Box::new(BipSyncSource::new(token))
	} else if spec == "bolts" {
		Box::new(BoltSyncSource::new(token))
	} else if spec == "blips" {
		Box::new(BlipSyncSource::new(token))
	} else if spec == "optech" {
		Box::new(OptechNewsletterSyncSource::new(token))
	} else {
		anyhow::bail!(
			"unknown source: '{}'. Expected: github:owner/repo, commits:owner/repo, \
			 irc:channel, delving, mailing_list, lightning_dev, bips, bolts, blips, optech",
			spec
		);
	};

	info!(source = spec, limit_pages = cli.limit_pages, "running single source ingest");

	let mut cursor: Option<String> = None;
	let mut total_docs = 0u32;

	for page in 0..cli.limit_pages {
		let result = source.fetch_page(cursor.as_deref(), &rate_limiter).await?;
		let doc_count = result.documents.len();
		total_docs += doc_count as u32;

		for doc in &result.documents {
			store.upsert_document(doc).await?;

			if let Some(ref body) = doc.body {
				let output =
					bkb_ingest::enrichment::enrich(&doc.id, body, doc.source_repo.as_deref());
				store.delete_refs_from(&doc.id).await?;
				for reference in &output.references {
					store.insert_reference(reference).await?;
				}
				store.delete_concept_mentions(&doc.id).await?;
				for (slug, confidence) in &output.concept_tags {
					store.upsert_concept_mention(&doc.id, slug, *confidence).await?;
				}
			}
		}

		for reference in &result.references {
			store.insert_reference(reference).await?;
		}

		info!(page = page + 1, documents = doc_count, "ingested page");

		cursor = result.next_cursor;
		if cursor.is_none() {
			info!("source exhausted");
			break;
		}
	}

	info!(total_documents = total_docs, "single source ingest complete");

	// Print stats
	let stats = store.get_stats().await?;
	for (source_type, count) in &stats {
		info!(source_type, count, "document count");
	}

	Ok(())
}

/// Create a `RepoCache` from CLI arguments. Only called when ingestion is enabled.
fn create_repo_cache(cli: &Cli) -> Result<RepoCache> {
	let cache_dir = expand_tilde(&cli.cache_dir);
	RepoCache::new(cache_dir, cli.max_cache_gb * 1024 * 1024 * 1024, cli.max_repo_size_mb * 1024)
}

/// Expand `~` at the start of a path to the user's home directory.
fn expand_tilde(path: &str) -> PathBuf {
	if let Some(rest) = path.strip_prefix("~/") {
		if let Ok(home) = std::env::var("HOME") {
			return PathBuf::from(home).join(rest);
		}
	}
	PathBuf::from(path)
}
