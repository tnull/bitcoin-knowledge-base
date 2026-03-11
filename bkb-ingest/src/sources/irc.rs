use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{NaiveDate, Utc};
use reqwest::{Client, StatusCode};
use tracing::{debug, info, warn};

use bkb_core::model::{Document, SourceType};

use super::{SyncPage, SyncSource};
use crate::rate_limiter::RateLimiter;

const GNUSHA_BASE_URL: &str = "https://gnusha.org";

/// Default start date for initial sync (when no cursor is provided).
/// The gnusha.org IRC archive for `#bitcoin-core-dev` starts around
/// 2015-09-30. We use 2015-06-01 as a safe starting point to capture
/// the full history across all channels.
const DEFAULT_START_DATE: &str = "2015-06-01";

/// Sync source for IRC chat logs from gnusha.org.
///
/// Fetches daily plain-text log files at URLs like
/// `https://gnusha.org/{channel}/{YYYY-MM-DD}.log`.
/// Each daily log is indexed as one document with `source_type = IrcLog`.
/// The cursor is the date (`YYYY-MM-DD`), incrementing by one day per page.
pub struct IrcLogSyncSource {
	client: Client,
	channel: String,
}

impl IrcLogSyncSource {
	pub fn new(channel: &str) -> Self {
		Self { client: Client::new(), channel: channel.to_string() }
	}

	/// Build the URL for a given date's log file.
	fn log_url(&self, date: &NaiveDate) -> String {
		format!("{}/{}/{}.log", GNUSHA_BASE_URL, self.channel, date.format("%Y-%m-%d"))
	}

	/// Parse a cursor string into a `NaiveDate`, or return the default start date.
	fn parse_cursor(cursor: Option<&str>) -> Result<NaiveDate> {
		match cursor {
			Some(s) => NaiveDate::parse_from_str(s, "%Y-%m-%d")
				.with_context(|| format!("invalid IRC log cursor: {}", s)),
			None => Ok(NaiveDate::parse_from_str(DEFAULT_START_DATE, "%Y-%m-%d")
				.expect("hardcoded default date is valid")),
		}
	}
}

#[async_trait]
impl SyncSource for IrcLogSyncSource {
	async fn fetch_page(
		&self, cursor: Option<&str>, rate_limiter: &RateLimiter,
	) -> Result<SyncPage> {
		rate_limiter.acquire().await;

		let date = Self::parse_cursor(cursor)?;
		let today = Utc::now().date_naive();

		// If the date is today or in the future, we are caught up.
		if date >= today {
			debug!(channel = %self.channel, date = %date, "caught up to today, no more pages");
			return Ok(SyncPage { documents: vec![], references: vec![], next_cursor: None });
		}

		let url = self.log_url(&date);
		let date_str = date.format("%Y-%m-%d").to_string();

		debug!(url = %url, "fetching IRC log");

		let response =
			self.client.get(&url).header("User-Agent", "bkb/0.1").send().await.with_context(
				|| format!("failed to fetch IRC log for {}/{}", self.channel, date_str),
			)?;

		let next_date = date.succ_opt().expect("date overflow");
		let next_cursor = Some(next_date.format("%Y-%m-%d").to_string());

		let status = response.status();

		if status == StatusCode::NOT_FOUND {
			// No log for this date; skip and advance cursor.
			info!(
				channel = %self.channel,
				date = %date_str,
				"no IRC log for this date (404), skipping"
			);
			return Ok(SyncPage { documents: vec![], references: vec![], next_cursor });
		}

		if !status.is_success() {
			let body = response.text().await.unwrap_or_default();
			anyhow::bail!("gnusha.org returned {} for {}: {}", status, url, body);
		}

		let body = response
			.text()
			.await
			.with_context(|| format!("failed to read IRC log body for {}", date_str))?;

		if body.trim().is_empty() {
			warn!(
				channel = %self.channel,
				date = %date_str,
				"IRC log is empty, skipping"
			);
			return Ok(SyncPage { documents: vec![], references: vec![], next_cursor });
		}

		let source_id = format!("{}:{}", self.channel, date_str);
		let id = Document::make_id(&SourceType::IrcLog, None, &source_id);

		// Use midnight UTC of the log date as `created_at`.
		let created_at = date.and_hms_opt(0, 0, 0).expect("midnight is valid").and_utc();

		let document = Document {
			id,
			source_type: SourceType::IrcLog,
			source_repo: None,
			source_id,
			title: Some(format!("#{} IRC log {}", self.channel, date_str)),
			body: Some(body),
			author: None,
			author_id: None,
			created_at,
			updated_at: None,
			parent_id: None,
			metadata: None,
			seq: None,
		};

		info!(
			source = %self.name(),
			date = %date_str,
			"fetched IRC log"
		);

		Ok(SyncPage { documents: vec![document], references: vec![], next_cursor })
	}

	fn poll_interval(&self) -> Duration {
		Duration::from_secs(3600) // 1 hour
	}

	fn name(&self) -> &str {
		Box::leak(format!("irc:{}", self.channel).into_boxed_str())
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_log_url() {
		let source = IrcLogSyncSource::new("bitcoin-core-dev");
		let date = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
		assert_eq!(source.log_url(&date), "https://gnusha.org/bitcoin-core-dev/2024-01-15.log");
	}

	#[test]
	fn test_parse_cursor_some() {
		let date = IrcLogSyncSource::parse_cursor(Some("2024-01-15")).unwrap();
		assert_eq!(date, NaiveDate::from_ymd_opt(2024, 1, 15).unwrap());
	}

	#[test]
	fn test_parse_cursor_none() {
		let date = IrcLogSyncSource::parse_cursor(None).unwrap();
		assert_eq!(date, NaiveDate::from_ymd_opt(2015, 6, 1).unwrap());
	}

	#[test]
	fn test_parse_cursor_invalid() {
		assert!(IrcLogSyncSource::parse_cursor(Some("not-a-date")).is_err());
	}

	#[test]
	fn test_source_id_format() {
		let channel = "bitcoin-core-dev";
		let date_str = "2024-01-15";
		let source_id = format!("{}:{}", channel, date_str);
		assert_eq!(source_id, "bitcoin-core-dev:2024-01-15");
	}

	#[test]
	fn test_name() {
		let source = IrcLogSyncSource::new("bitcoin-core-dev");
		assert_eq!(source.name(), "irc:bitcoin-core-dev");
	}

	#[tokio::test]
	async fn test_fetch_page_future_date_returns_no_cursor() {
		let source = IrcLogSyncSource::new("bitcoin-core-dev");
		let rate_limiter = RateLimiter::new(0);

		// Use a date far in the future
		let page = source.fetch_page(Some("2099-12-31"), &rate_limiter).await.unwrap();
		assert!(page.documents.is_empty());
		assert!(page.next_cursor.is_none());
	}
}
