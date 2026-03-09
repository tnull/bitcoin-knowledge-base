use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use regex::Regex;
use reqwest::Client;
use serde::Deserialize;
use tracing::{debug, info, warn};

use bkb_core::model::{Document, SourceType};

use super::{SyncPage, SyncSource};
use crate::rate_limiter::RateLimiter;

/// A spec file discovered from the GitHub Tree API.
#[derive(Debug, Clone)]
struct SpecFile {
	/// The spec number (e.g., 340 for BIP-340).
	number: u32,
	/// The filename relative to the repo root (e.g., "bip-0340.mediawiki").
	filename: String,
}

/// List spec files from a GitHub repo using the Tree API (single API call).
///
/// `pattern` is a regex with a capture group for the spec number, e.g.,
/// `r"^bip-(\d{4})\.(mediawiki|md)$"`.
async fn discover_spec_files(
	client: &Client, owner: &str, repo: &str, branch: &str, pattern: &Regex, token: Option<&str>,
	rate_limiter: &RateLimiter,
) -> Result<Vec<SpecFile>> {
	rate_limiter.acquire().await;

	let url =
		format!("https://api.github.com/repos/{}/{}/git/trees/{}?recursive=1", owner, repo, branch);

	let mut req = client
		.get(&url)
		.header("User-Agent", "bkb/0.1")
		.header("Accept", "application/vnd.github+json");

	if let Some(token) = token {
		req = req.header("Authorization", format!("Bearer {}", token));
	}

	let response = req.send().await.context("failed to fetch repo tree")?;
	rate_limiter.update_from_response(response.headers());

	let status = response.status();
	if !status.is_success() {
		let body = response.text().await.unwrap_or_default();
		anyhow::bail!("GitHub Tree API returned {}: {}", status, body);
	}

	let tree: GitTreeResponse = response.json().await.context("failed to parse tree response")?;

	let mut files = Vec::new();
	for entry in &tree.tree {
		if entry.entry_type != "blob" {
			continue;
		}
		if let Some(caps) = pattern.captures(&entry.path) {
			if let Some(num_str) = caps.get(1) {
				if let Ok(number) = num_str.as_str().parse::<u32>() {
					files.push(SpecFile { number, filename: entry.path.clone() });
				}
			}
		}
	}

	files.sort_by_key(|f| f.number);
	Ok(files)
}

#[derive(Debug, Deserialize)]
struct GitTreeResponse {
	tree: Vec<GitTreeEntry>,
}

#[derive(Debug, Deserialize)]
struct GitTreeEntry {
	path: String,
	#[serde(rename = "type")]
	entry_type: String,
}

// ---------------------------------------------------------------------------
// BIP Sync Source
// ---------------------------------------------------------------------------

/// Sync source for BIP specification documents.
///
/// Uses the GitHub Tree API to discover which BIPs exist, then fetches
/// their content from the raw content API. No hardcoded max needed.
pub struct BipSyncSource {
	client: Client,
	token: Option<String>,
	/// How many BIPs to fetch per page.
	page_size: usize,
	/// Discovered BIP files (populated on first fetch_page call).
	discovered: Mutex<Option<Vec<SpecFile>>>,
}

impl BipSyncSource {
	pub fn new(token: Option<String>) -> Self {
		Self { client: Client::new(), token, page_size: 10, discovered: Mutex::new(None) }
	}

	/// Ensure we've discovered the list of BIP files.
	async fn ensure_discovered(&self, rate_limiter: &RateLimiter) -> Result<Vec<SpecFile>> {
		{
			let guard = self.discovered.lock().unwrap();
			if let Some(ref files) = *guard {
				return Ok(files.clone());
			}
		}

		let pattern = Regex::new(r"^bip-(\d{4})\.(mediawiki|md)$").unwrap();
		let files = discover_spec_files(
			&self.client,
			"bitcoin",
			"bips",
			"master",
			&pattern,
			self.token.as_deref(),
			rate_limiter,
		)
		.await?;

		info!(count = files.len(), "discovered BIP files via Tree API");

		let mut guard = self.discovered.lock().unwrap();
		*guard = Some(files.clone());
		Ok(files)
	}

	async fn fetch_bip_by_file(
		&self, file: &SpecFile, rate_limiter: &RateLimiter,
	) -> Result<Option<Document>> {
		rate_limiter.acquire().await;

		let url =
			format!("https://raw.githubusercontent.com/bitcoin/bips/master/{}", file.filename);

		let mut req = self.client.get(&url).header("User-Agent", "bkb/0.1");

		if let Some(ref token) = self.token {
			req = req.header("Authorization", format!("Bearer {}", token));
		}

		let response = req.send().await?;
		rate_limiter.update_from_response(response.headers());

		if !response.status().is_success() {
			return Ok(None);
		}

		let body = response.text().await?;
		if body.is_empty() {
			return Ok(None);
		}

		let ext = file.filename.rsplit('.').next().unwrap_or("mediawiki");
		let title = extract_bip_title(&body, file.number);
		let source_id = file.number.to_string();
		let id = Document::make_id(&SourceType::Bip, None, &source_id);

		Ok(Some(Document {
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
		}))
	}
}

#[async_trait]
impl SyncSource for BipSyncSource {
	async fn fetch_page(
		&self, cursor: Option<&str>, rate_limiter: &RateLimiter,
	) -> Result<SyncPage> {
		let all_files = self.ensure_discovered(rate_limiter).await?;

		// Cursor is an index into the discovered files list
		let start: usize = cursor.and_then(|c| c.parse().ok()).unwrap_or(0);

		let page_files = &all_files[start..all_files.len().min(start + self.page_size)];
		let mut documents = Vec::new();

		for file in page_files {
			match self.fetch_bip_by_file(file, rate_limiter).await {
				Ok(Some(doc)) => {
					debug!(bip = file.number, "fetched BIP");
					documents.push(doc);
				},
				Ok(None) => {
					debug!(bip = file.number, "BIP content empty, skipping");
				},
				Err(e) => {
					warn!(bip = file.number, error = %e, "failed to fetch BIP");
				},
			}
		}

		let next_idx = start + self.page_size;
		let next_cursor =
			if next_idx < all_files.len() { Some(next_idx.to_string()) } else { None };

		info!(count = documents.len(), start, total = all_files.len(), "fetched BIPs page");
		Ok(SyncPage { documents, references: Vec::new(), next_cursor })
	}

	fn poll_interval(&self) -> Duration {
		Duration::from_secs(86400) // Daily -- specs don't change often
	}

	fn name(&self) -> &str {
		"specs:bips"
	}
}

// ---------------------------------------------------------------------------
// BOLT Sync Source
// ---------------------------------------------------------------------------

/// Sync source for BOLT specification documents.
///
/// BOLTs use a slug-based naming scheme (e.g., `02-peer-protocol.md`), so
/// we discover files via the Tree API and parse out the BOLT number.
pub struct BoltSyncSource {
	client: Client,
	token: Option<String>,
	/// Discovered BOLT files (populated on first fetch_page call).
	discovered: Mutex<Option<Vec<SpecFile>>>,
}

impl BoltSyncSource {
	pub fn new(token: Option<String>) -> Self {
		Self { client: Client::new(), token, discovered: Mutex::new(None) }
	}

	/// Discover BOLT files via Tree API.
	async fn ensure_discovered(&self, rate_limiter: &RateLimiter) -> Result<Vec<SpecFile>> {
		{
			let guard = self.discovered.lock().unwrap();
			if let Some(ref files) = *guard {
				return Ok(files.clone());
			}
		}

		// BOLT files are named like "00-introduction.md", "02-peer-protocol.md"
		let pattern = Regex::new(r"^(\d{2})-[\w-]+\.md$").unwrap();
		let files = discover_spec_files(
			&self.client,
			"lightning",
			"bolts",
			"master",
			&pattern,
			self.token.as_deref(),
			rate_limiter,
		)
		.await?;

		info!(count = files.len(), "discovered BOLT files via Tree API");

		let mut guard = self.discovered.lock().unwrap();
		*guard = Some(files.clone());
		Ok(files)
	}

	async fn fetch_bolt_by_file(
		&self, file: &SpecFile, rate_limiter: &RateLimiter,
	) -> Result<Option<Document>> {
		rate_limiter.acquire().await;

		let url =
			format!("https://raw.githubusercontent.com/lightning/bolts/master/{}", file.filename);

		let mut req = self.client.get(&url).header("User-Agent", "bkb/0.1");

		if let Some(ref token) = self.token {
			req = req.header("Authorization", format!("Bearer {}", token));
		}

		let response = req.send().await?;
		rate_limiter.update_from_response(response.headers());

		if !response.status().is_success() {
			return Ok(None);
		}

		let body = response.text().await?;
		if body.is_empty() {
			return Ok(None);
		}

		let title = extract_bolt_title(&body, file.number);
		let source_id = file.number.to_string();
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
		&self, _cursor: Option<&str>, rate_limiter: &RateLimiter,
	) -> Result<SyncPage> {
		let all_files = self.ensure_discovered(rate_limiter).await?;

		let mut documents = Vec::new();

		// BOLTs are few enough to fetch all in one page
		for file in &all_files {
			match self.fetch_bolt_by_file(file, rate_limiter).await {
				Ok(Some(doc)) => {
					debug!(bolt = file.number, filename = %file.filename, "fetched BOLT");
					documents.push(doc);
				},
				Ok(None) => {
					debug!(bolt = file.number, "BOLT content empty, skipping");
				},
				Err(e) => {
					warn!(bolt = file.number, error = %e, "failed to fetch BOLT");
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

// ---------------------------------------------------------------------------
// bLIP Sync Source
// ---------------------------------------------------------------------------

/// Sync source for bLIP (Bitcoin Lightning Improvement Proposal) documents.
///
/// Uses the GitHub Tree API to discover which bLIPs exist, then fetches
/// their content. No hardcoded max needed.
pub struct BlipSyncSource {
	client: Client,
	token: Option<String>,
	/// Discovered bLIP files (populated on first fetch_page call).
	discovered: Mutex<Option<Vec<SpecFile>>>,
}

impl BlipSyncSource {
	pub fn new(token: Option<String>) -> Self {
		Self { client: Client::new(), token, discovered: Mutex::new(None) }
	}

	/// Discover bLIP files via Tree API.
	async fn ensure_discovered(&self, rate_limiter: &RateLimiter) -> Result<Vec<SpecFile>> {
		{
			let guard = self.discovered.lock().unwrap();
			if let Some(ref files) = *guard {
				return Ok(files.clone());
			}
		}

		let pattern = Regex::new(r"^blip-(\d{4})\.md$").unwrap();
		let files = discover_spec_files(
			&self.client,
			"lightning",
			"blips",
			"master",
			&pattern,
			self.token.as_deref(),
			rate_limiter,
		)
		.await?;

		info!(count = files.len(), "discovered bLIP files via Tree API");

		let mut guard = self.discovered.lock().unwrap();
		*guard = Some(files.clone());
		Ok(files)
	}

	async fn fetch_blip_by_file(
		&self, file: &SpecFile, rate_limiter: &RateLimiter,
	) -> Result<Option<Document>> {
		rate_limiter.acquire().await;

		let url =
			format!("https://raw.githubusercontent.com/lightning/blips/master/{}", file.filename);

		let mut req = self.client.get(&url).header("User-Agent", "bkb/0.1");

		if let Some(ref token) = self.token {
			req = req.header("Authorization", format!("Bearer {}", token));
		}

		let response = req.send().await?;
		rate_limiter.update_from_response(response.headers());

		if !response.status().is_success() {
			return Ok(None);
		}

		let body = response.text().await?;
		if body.is_empty() {
			return Ok(None);
		}

		let title = extract_blip_title(&body, file.number);
		let source_id = file.number.to_string();
		let id = Document::make_id(&SourceType::Blip, None, &source_id);

		Ok(Some(Document {
			id,
			source_type: SourceType::Blip,
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
impl SyncSource for BlipSyncSource {
	async fn fetch_page(
		&self, _cursor: Option<&str>, rate_limiter: &RateLimiter,
	) -> Result<SyncPage> {
		let all_files = self.ensure_discovered(rate_limiter).await?;

		// bLIPs are few enough to fetch all in one page
		let mut documents = Vec::new();

		for file in &all_files {
			match self.fetch_blip_by_file(file, rate_limiter).await {
				Ok(Some(doc)) => {
					debug!(blip = file.number, "fetched bLIP");
					documents.push(doc);
				},
				Ok(None) => {
					debug!(blip = file.number, "bLIP content empty, skipping");
				},
				Err(e) => {
					warn!(blip = file.number, error = %e, "failed to fetch bLIP");
				},
			}
		}

		info!(count = documents.len(), "fetched all bLIPs");
		Ok(SyncPage { documents, references: Vec::new(), next_cursor: None })
	}

	fn poll_interval(&self) -> Duration {
		Duration::from_secs(86400)
	}

	fn name(&self) -> &str {
		"specs:blips"
	}
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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

/// Extract title from a bLIP document body.
fn extract_blip_title(body: &str, number: u32) -> String {
	// bLIPs use markdown with a "Title:" field or a `#` heading
	for line in body.lines().take(30) {
		let trimmed = line.trim();
		if let Some(rest) = trimmed.strip_prefix("Title:") {
			let title = rest.trim();
			if !title.is_empty() {
				return format!("bLIP-{}: {}", number, title);
			}
		}
		if trimmed.starts_with('#') && !trimmed.starts_with("##") {
			let title = trimmed.trim_start_matches('#').trim();
			if !title.is_empty() {
				return format!("bLIP-{}: {}", number, title);
			}
		}
	}
	format!("bLIP-{}", number)
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
	fn test_extract_blip_title_heading() {
		let body = "# bLIP 1: Key Send\n\nSome content here.";
		assert_eq!(extract_blip_title(body, 1), "bLIP-1: bLIP 1: Key Send");
	}

	#[test]
	fn test_extract_blip_title_field() {
		let body = "```\n  bLIP: 2\n  Title: Hosted Channels\n  Author: Anton\n```";
		assert_eq!(extract_blip_title(body, 2), "bLIP-2: Hosted Channels");
	}

	#[test]
	fn test_extract_blip_title_fallback() {
		let body = "Some body without a title";
		assert_eq!(extract_blip_title(body, 99), "bLIP-99");
	}
}
