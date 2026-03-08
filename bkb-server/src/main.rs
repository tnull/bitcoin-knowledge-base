mod api;
mod config;

use std::sync::Arc;

use anyhow::Result;
use clap::Parser;
use tokio::time::Instant;
use tracing::info;

use bkb_ingest::queue::{JobQueue, Priority, SyncJob};
use bkb_ingest::rate_limiter::RateLimiter;
use bkb_ingest::sources::delving::DelvingSyncSource;
use bkb_ingest::sources::github::{GitHubCommentSyncSource, GitHubIssueSyncSource};
use bkb_ingest::sources::irc::IrcLogSyncSource;
use bkb_ingest::sources::mailing_list::MailingListSyncSource;
use bkb_ingest::sources::optech::OptechNewsletterSyncSource;
use bkb_ingest::sources::specs::{BipSyncSource, BoltSyncSource};
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

		// Register mailing list source
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
			info!("registered mailing list sync source");
		}

		// Register BIP/BOLT spec sources
		{
			let max_bip = if cli.dev_subset { 344 } else { 500 };
			let bip_source = BipSyncSource::new(cli.github_token.clone(), max_bip);
			let bip_interval = bip_source.poll_interval();
			queue
				.add_job(SyncJob {
					source_id: "specs:bips".to_string(),
					source: Box::new(bip_source),
					priority: Priority::Low,
					cursor: if cli.dev_subset { Some("340".to_string()) } else { None },
					next_run: Instant::now(),
					retry_count: 0,
					base_interval: bip_interval,
				})
				.await;
			info!("registered BIP sync source");

			let bolt_source = BoltSyncSource::new(cli.github_token.clone(), 12);
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
		}

		// Register Optech newsletter source
		{
			let optech_source = OptechNewsletterSyncSource::new(cli.github_token.clone(), 400);
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

		// +4 for mailing list, BIPs, BOLTs, Optech
		let total_sources = repos.len() * 2
			+ config.irc_channels().len()
			+ if config.sync_delving() { 1 } else { 0 }
			+ 4;
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
