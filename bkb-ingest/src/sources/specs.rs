use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;
use tracing::{debug, info, warn};

use bkb_core::model::{Document, SourceType};

use super::{SyncPage, SyncSource};
use crate::rate_limiter::RateLimiter;

/// Sync source for BIP specification documents.
///
/// Fetches BIP files from the GitHub raw content API. Each BIP becomes
/// a single document. Cursor is the BIP number to start from.
pub struct BipSyncSource {
	client: Client,
	token: Option<String>,
	/// Maximum BIP number to check (inclusive).
	max_bip: u32,
	/// How many BIPs to fetch per page.
	page_size: u32,
}

impl BipSyncSource {
	pub fn new(token: Option<String>, max_bip: u32) -> Self {
		Self { client: Client::new(), token, max_bip, page_size: 10 }
	}

	async fn fetch_bip(&self, number: u32, rate_limiter: &RateLimiter) -> Result<Option<Document>> {
		rate_limiter.acquire().await;

		// Try mediawiki format first, then markdown
		let padded = format!("{:04}", number);
		let extensions = ["mediawiki", "md"];

		for ext in &extensions {
			let url = format!(
				"https://raw.githubusercontent.com/bitcoin/bips/master/bip-{}.{}",
				padded, ext
			);

			let mut req = self.client.get(&url).header("User-Agent", "bkb/0.1");

			if let Some(ref token) = self.token {
				req = req.header("Authorization", format!("Bearer {}", token));
			}

			let response = req.send().await?;
			rate_limiter.update_from_response(response.headers());

			if response.status() == reqwest::StatusCode::NOT_FOUND {
				continue;
			}

			if !response.status().is_success() {
				continue;
			}

			let body = response.text().await?;
			if body.is_empty() {
				continue;
			}

			// Extract title from the BIP content
			let title = extract_bip_title(&body, number);

			let source_id = number.to_string();
			let id = Document::make_id(&SourceType::Bip, None, &source_id);

			return Ok(Some(Document {
				id,
				source_type: SourceType::Bip,
				source_repo: None,
				source_id,
				title: Some(title),
				body: Some(body),
				author: None,
				author_id: None,
				created_at: Utc::now(),
				updated_at: Some(Utc::now()),
				parent_id: None,
				metadata: Some(serde_json::json!({ "format": ext })),
				seq: None,
			}));
		}

		Ok(None)
	}
}

#[async_trait]
impl SyncSource for BipSyncSource {
	async fn fetch_page(
		&self, cursor: Option<&str>, rate_limiter: &RateLimiter,
	) -> Result<SyncPage> {
		let start: u32 = cursor.and_then(|c| c.parse().ok()).unwrap_or(0);

		let mut documents = Vec::new();
		let mut next_num = start;

		for num in start..start.saturating_add(self.page_size) {
			if num > self.max_bip {
				return Ok(SyncPage { documents, references: Vec::new(), next_cursor: None });
			}

			match self.fetch_bip(num, rate_limiter).await {
				Ok(Some(doc)) => {
					debug!(bip = num, "fetched BIP");
					documents.push(doc);
				},
				Ok(None) => {
					debug!(bip = num, "BIP not found, skipping");
				},
				Err(e) => {
					warn!(bip = num, error = %e, "failed to fetch BIP");
				},
			}

			next_num = num + 1;
		}

		let next_cursor = if next_num <= self.max_bip { Some(next_num.to_string()) } else { None };

		info!(count = documents.len(), start, "fetched BIPs page");
		Ok(SyncPage { documents, references: Vec::new(), next_cursor })
	}

	fn poll_interval(&self) -> Duration {
		Duration::from_secs(86400) // Daily -- specs don't change often
	}

	fn name(&self) -> &str {
		"specs:bips"
	}
}

/// Sync source for BOLT specification documents.
pub struct BoltSyncSource {
	client: Client,
	token: Option<String>,
	/// Maximum BOLT number to check.
	max_bolt: u32,
}

impl BoltSyncSource {
	pub fn new(token: Option<String>, max_bolt: u32) -> Self {
		Self { client: Client::new(), token, max_bolt }
	}

	async fn fetch_bolt(
		&self, number: u32, rate_limiter: &RateLimiter,
	) -> Result<Option<Document>> {
		rate_limiter.acquire().await;

		let padded = format!("{:02}", number);
		let url = format!(
			"https://raw.githubusercontent.com/lightning/bolts/master/{}-{}.md",
			padded,
			bolt_slug(number)
		);

		let mut req = self.client.get(&url).header("User-Agent", "bkb/0.1");

		if let Some(ref token) = self.token {
			req = req.header("Authorization", format!("Bearer {}", token));
		}

		let response = req.send().await?;
		rate_limiter.update_from_response(response.headers());

		if response.status() == reqwest::StatusCode::NOT_FOUND {
			return Ok(None);
		}

		if !response.status().is_success() {
			return Ok(None);
		}

		let body = response.text().await?;
		if body.is_empty() {
			return Ok(None);
		}

		let title = extract_bolt_title(&body, number);
		let source_id = number.to_string();
		let id = Document::make_id(&SourceType::Bolt, None, &source_id);

		Ok(Some(Document {
			id,
			source_type: SourceType::Bolt,
			source_repo: None,
			source_id,
			title: Some(title),
			body: Some(body),
			author: None,
			author_id: None,
			created_at: Utc::now(),
			updated_at: Some(Utc::now()),
			parent_id: None,
			metadata: None,
			seq: None,
		}))
	}
}

#[async_trait]
impl SyncSource for BoltSyncSource {
	async fn fetch_page(
		&self, cursor: Option<&str>, rate_limiter: &RateLimiter,
	) -> Result<SyncPage> {
		let start: u32 = cursor.and_then(|c| c.parse().ok()).unwrap_or(0);

		let mut documents = Vec::new();

		for num in start..=self.max_bolt {
			match self.fetch_bolt(num, rate_limiter).await {
				Ok(Some(doc)) => {
					debug!(bolt = num, "fetched BOLT");
					documents.push(doc);
				},
				Ok(None) => {
					debug!(bolt = num, "BOLT not found, skipping");
				},
				Err(e) => {
					warn!(bolt = num, error = %e, "failed to fetch BOLT");
				},
			}
		}

		info!(count = documents.len(), "fetched all BOLTs");
		Ok(SyncPage { documents, references: Vec::new(), next_cursor: None })
	}

	fn poll_interval(&self) -> Duration {
		Duration::from_secs(86400)
	}

	fn name(&self) -> &str {
		"specs:bolts"
	}
}

/// Map BOLT number to its file slug.
fn bolt_slug(number: u32) -> &'static str {
	match number {
		0 => "introduction",
		1 => "messaging",
		2 => "peer-protocol",
		3 => "transactions",
		4 => "onion-routing",
		5 => "onchain",
		7 => "p2p",
		8 => "transport",
		9 => "feature-bits",
		10 => "dns-bootstrap",
		11 => "payment-encoding",
		12 => "offers",
		_ => "unknown",
	}
}

/// Extract title from a BIP document body.
fn extract_bip_title(body: &str, number: u32) -> String {
	// Try to find "Title:" in the preamble
	for line in body.lines().take(30) {
		let trimmed = line.trim();
		if let Some(rest) = trimmed.strip_prefix("Title:") {
			let title = rest.trim();
			if !title.is_empty() {
				return format!("BIP-{}: {}", number, title);
			}
		}
		// Also try mediawiki format
		if let Some(rest) = trimmed.strip_prefix("| Title") {
			if let Some(title) = rest.strip_prefix("=").or_else(|| rest.strip_prefix(" =")) {
				let title = title.trim();
				if !title.is_empty() {
					return format!("BIP-{}: {}", number, title);
				}
			}
		}
	}
	format!("BIP-{}", number)
}

/// Extract title from a BOLT document body.
fn extract_bolt_title(body: &str, number: u32) -> String {
	// BOLTs typically start with "# BOLT #N: Title"
	for line in body.lines().take(5) {
		let trimmed = line.trim();
		if trimmed.starts_with('#') {
			let title = trimmed.trim_start_matches('#').trim();
			if !title.is_empty() {
				return title.to_string();
			}
		}
	}
	format!("BOLT-{}", number)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_extract_bip_title_mediawiki() {
		let body = r#"<pre>
  BIP: 340
  Title: Schnorr Signatures for secp256k1
  Author: Pieter Wuille
</pre>"#;
		assert_eq!(extract_bip_title(body, 340), "BIP-340: Schnorr Signatures for secp256k1");
	}

	#[test]
	fn test_extract_bip_title_fallback() {
		let body = "Some body without a title";
		assert_eq!(extract_bip_title(body, 1), "BIP-1");
	}

	#[test]
	fn test_extract_bolt_title() {
		let body = "# BOLT #2: Peer Protocol for Channel Management\n\nSome content here.";
		assert_eq!(extract_bolt_title(body, 2), "BOLT #2: Peer Protocol for Channel Management");
	}

	#[test]
	fn test_bolt_slug() {
		assert_eq!(bolt_slug(0), "introduction");
		assert_eq!(bolt_slug(2), "peer-protocol");
		assert_eq!(bolt_slug(11), "payment-encoding");
	}
}
