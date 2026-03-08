mod api;
mod config;

use std::sync::Arc;

use anyhow::Result;
use clap::Parser;
use tokio::time::Instant;
use tracing::info;

use bkb_ingest::queue::{JobQueue, Priority, SyncJob};
use bkb_ingest::rate_limiter::RateLimiter;
use bkb_ingest::sources::github::{GitHubCommentSyncSource, GitHubIssueSyncSource};
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
}

#[tokio::main]
async fn main() -> Result<()> {
	tracing_subscriber::fmt()
		.with_env_filter(
			tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
				"info,bkb_server=debug,bkb_ingest=debug,bkb_store=debug".into()
			}),
		)
		.init();

	let cli = Cli::parse();
	let config = Config::new(cli.dev_subset);

	info!(db = %cli.db, bind = %cli.bind, dev_subset = cli.dev_subset, "starting BKB server");

	let store = Arc::new(SqliteStore::open(std::path::Path::new(&cli.db))?);
	info!("database opened");

	// Start HTTP API server
	let api_store = Arc::clone(&store);
	let bind_addr: std::net::SocketAddr = cli.bind.parse()?;
	let api_handle = tokio::spawn(async move {
		if let Err(e) = api::serve(api_store, bind_addr).await {
			tracing::error!(error = %e, "API server failed");
		}
	});

	info!(addr = %bind_addr, "HTTP API server started");

	if !cli.no_ingest {
		// Start ingestion scheduler
		let rate_limiter = Arc::new(RateLimiter::new(200));
		let queue = Arc::new(JobQueue::new(Arc::clone(&rate_limiter), Arc::clone(&store)));

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

		info!(sources = repos.len() * 2, "ingestion scheduler starting");

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
