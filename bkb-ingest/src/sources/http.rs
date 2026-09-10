//! Bounded polling windows and HTTP pagination shared by repository adapters.
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use reqwest::{header::HeaderMap, Client};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::SyncPage;
use crate::rate_limiter::RateLimiter;
use bkb_core::model::Document;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PollCursor {
	version: u8,
	pub since: Option<String>,
	pub until: DateTime<Utc>,
	pub page: u32,
	pub complete: bool,
	pub last_full: DateTime<Utc>,
	#[serde(default)]
	pub pending: Vec<u64>,
	#[serde(default)]
	pub child_page: u32,
	#[serde(default)]
	pub more_parents: bool,
}

impl PollCursor {
	pub fn resume(cursor: Option<&str>) -> Result<Self> {
		let now = Utc::now();
		let mut value = if let Some(cursor) = cursor {
			if cursor.starts_with('{') {
				let value: Self = serde_json::from_str(cursor).context("invalid polling cursor")?;
				anyhow::ensure!(value.version == 1, "unsupported polling cursor version");
				value
			} else {
				let since =
					DateTime::parse_from_rfc3339(cursor).context("invalid legacy cursor")?;
				Self::new(Some(since.to_rfc3339()), now)
			}
		} else {
			Self::new(None, now)
		};
		if value.complete {
			let full = now.signed_duration_since(value.last_full).num_days() >= 7;
			let since = (!full).then(|| (value.until - chrono::Duration::minutes(5)).to_rfc3339());
			let last_full = if full { now } else { value.last_full };
			value = Self::new(since, now);
			value.last_full = last_full;
		}
		Ok(value)
	}

	fn new(since: Option<String>, now: DateTime<Utc>) -> Self {
		Self {
			version: 1,
			since,
			until: now,
			page: 1,
			complete: false,
			last_full: now,
			pending: vec![],
			child_page: 1,
			more_parents: true,
		}
	}

	pub fn result(mut self, documents: Vec<Document>, more: bool) -> Result<SyncPage> {
		self.complete = !more;
		let encoded = serde_json::to_string(&self)?;
		Ok(SyncPage {
			documents,
			references: vec![],
			next_cursor: more.then(|| encoded.clone()),
			checkpoint: (!more).then_some(encoded),
		})
	}
}

pub(crate) fn client() -> Client {
	Client::builder()
		.timeout(Duration::from_secs(30))
		.redirect(reqwest::redirect::Policy::none())
		.build()
		.expect("valid HTTP client")
}

pub(crate) fn more_pages(headers: &HeaderMap, page: u32, limit: u32, len: usize) -> bool {
	if let Some(link) = headers.get("link").and_then(|h| h.to_str().ok()) {
		return link.split(',').any(|part| part.contains("rel=\"next\""));
	}
	if let Some(count) = headers
		.get("x-total-count")
		.and_then(|h| h.to_str().ok())
		.and_then(|h| h.parse::<u64>().ok())
	{
		return u64::from(page) * u64::from(limit) < count;
	}
	len >= limit as usize
}

pub(crate) async fn github_get(
	client: &Client, base: &str, repo: &str, path: &str, token: Option<&str>,
	rate_limiter: &RateLimiter, query: &[(String, String)], page: u32,
) -> Result<(Vec<Value>, bool)> {
	rate_limiter.acquire().await;
	let mut request = client
		.get(format!("{base}/repos/{repo}/{path}"))
		.header("User-Agent", "bkb/0.2")
		.header("Accept", "application/vnd.github+json")
		.query(query)
		.query(&[("page", page.to_string()), ("per_page", "100".into())]);
	if let Some(token) = token {
		request = request.bearer_auth(token);
	}
	let response = request.send().await.context("GitHub request failed")?;
	rate_limiter.update_from_response(response.headers());
	let response = response.error_for_status().context("GitHub API error")?;
	let headers = response.headers().clone();
	let items: Vec<Value> = response.json().await.context("invalid GitHub list response")?;
	let more = more_pages(&headers, page, 100, items.len());
	Ok((items, more))
}

pub(crate) fn string(value: &Value, field: &str) -> Result<String> {
	value[field].as_str().map(str::to_owned).with_context(|| format!("missing {field}"))
}

pub(crate) fn timestamp(value: &Value, field: &str) -> Result<DateTime<Utc>> {
	Ok(DateTime::parse_from_rfc3339(&string(value, field)?)?.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn terminal_checkpoint_starts_a_new_window_with_overlap() {
		let mut cursor = PollCursor::resume(None).unwrap();
		cursor.until = Utc::now() - chrono::Duration::hours(1);
		let until = cursor.until;
		let result = cursor.result(vec![], false).unwrap();
		assert!(result.next_cursor.is_none());
		let resumed = PollCursor::resume(result.checkpoint.as_deref()).unwrap();
		assert_eq!(resumed.since, Some((until - chrono::Duration::minutes(5)).to_rfc3339()));
		assert_eq!(resumed.page, 1);
	}
	#[test]
	fn hidden_items_do_not_end_pagination_early() {
		let mut headers = HeaderMap::new();
		headers.insert("x-total-count", "201".parse().unwrap());
		assert!(more_pages(&headers, 1, 100, 0));
		assert!(!more_pages(&headers, 3, 100, 1));
	}
}
