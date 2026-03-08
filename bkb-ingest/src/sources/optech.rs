use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;
use tracing::{debug, info, warn};

use bkb_core::model::{Document, SourceType};

use super::{SyncPage, SyncSource};
use crate::rate_limiter::RateLimiter;

/// Sync source for Bitcoin Optech newsletters.
///
/// Fetches newsletter markdown files from the Optech GitHub repo via
/// the raw content API. Each newsletter becomes a single document.
pub struct OptechNewsletterSyncSource {
	client: Client,
	token: Option<String>,
	/// Maximum newsletter number to check.
	max_number: u32,
	/// Newsletters per page.
	page_size: u32,
}

impl OptechNewsletterSyncSource {
	pub fn new(token: Option<String>, max_number: u32) -> Self {
		Self { client: Client::new(), token, max_number, page_size: 10 }
	}

}

#[async_trait]
impl SyncSource for OptechNewsletterSyncSource {
	async fn fetch_page(
		&self, cursor: Option<&str>, rate_limiter: &RateLimiter,
	) -> Result<SyncPage> {
		rate_limiter.acquire().await;

		let page: u32 = cursor.and_then(|c| c.parse().ok()).unwrap_or(1);

		// Fetch directory listing of newsletter files
		let url = format!(
			"https://api.github.com/repos/bitcoinops/bitcoinops.github.io/contents/_posts/en/newsletters?ref=master&per_page=100&page={}",
			page,
		);

		let mut req = self
			.client
			.get(&url)
			.header("User-Agent", "bkb/0.1")
			.header("Accept", "application/vnd.github+json");

		if let Some(ref token) = self.token {
			req = req.header("Authorization", format!("Bearer {}", token));
		}

		let response = req.send().await.context("failed to list Optech newsletters")?;
		rate_limiter.update_from_response(response.headers());

		if !response.status().is_success() {
			let status = response.status();
			let body = response.text().await.unwrap_or_default();
			anyhow::bail!("GitHub API returned {}: {}", status, body);
		}

		let files: Vec<GitHubContentEntry> =
			response.json().await.context("failed to parse directory listing")?;

		info!(count = files.len(), page, "listed Optech newsletter files");

		let mut documents = Vec::new();

		for entry in &files {
			if !entry.name.ends_with(".md") {
				continue;
			}

			// Extract newsletter slug from filename like "2024-01-10-newsletter.md"
			let slug = entry.name.trim_end_matches(".md");

			// Fetch raw content
			if let Some(ref download_url) = entry.download_url {
				rate_limiter.acquire().await;

				let resp =
					self.client.get(download_url).header("User-Agent", "bkb/0.1").send().await;

				match resp {
					Ok(r) if r.status().is_success() => {
						let body = r.text().await.unwrap_or_default();
						let title = extract_newsletter_title(&body, slug);
						let newsletter_num = extract_newsletter_number(&body);
						let source_id = newsletter_num
							.map(|n| n.to_string())
							.unwrap_or_else(|| slug.to_string());

						let id = Document::make_id(&SourceType::OptechNewsletter, None, &source_id);

						documents.push(Document {
							id,
							source_type: SourceType::OptechNewsletter,
							source_repo: None,
							source_id,
							title: Some(title),
							body: Some(body),
							author: None,
							author_id: None,
							created_at: Utc::now(),
							updated_at: Some(Utc::now()),
							parent_id: None,
							metadata: Some(serde_json::json!({ "slug": slug })),
							seq: None,
						});

						debug!(slug, "fetched newsletter");
					},
					Ok(r) => {
						warn!(slug, status = %r.status(), "failed to fetch newsletter content");
					},
					Err(e) => {
						warn!(slug, error = %e, "failed to fetch newsletter content");
					},
				}
			}
		}

		// If we got a full page (100 items), there may be more
		let next_cursor = if files.len() >= 100 { Some((page + 1).to_string()) } else { None };

		info!(count = documents.len(), "fetched Optech newsletters");
		Ok(SyncPage { documents, references: Vec::new(), next_cursor })
	}

	fn poll_interval(&self) -> Duration {
		Duration::from_secs(86400) // Daily
	}

	fn name(&self) -> &str {
		"optech:newsletters"
	}
}

#[derive(serde::Deserialize)]
struct GitHubContentEntry {
	name: String,
	download_url: Option<String>,
}

/// Extract the title from an Optech newsletter markdown file.
fn extract_newsletter_title(body: &str, slug: &str) -> String {
	// Look for "title:" in the YAML frontmatter
	let mut in_frontmatter = false;
	for line in body.lines() {
		if line.trim() == "---" {
			if in_frontmatter {
				break;
			}
			in_frontmatter = true;
			continue;
		}
		if in_frontmatter {
			if let Some(rest) = line.strip_prefix("title:") {
				let title = rest.trim().trim_matches('"').trim_matches('\'');
				if !title.is_empty() {
					return title.to_string();
				}
			}
		}
	}
	format!("Optech Newsletter {}", slug)
}

/// Extract the newsletter number from frontmatter.
fn extract_newsletter_number(body: &str) -> Option<u32> {
	let mut in_frontmatter = false;
	for line in body.lines() {
		if line.trim() == "---" {
			if in_frontmatter {
				break;
			}
			in_frontmatter = true;
			continue;
		}
		if in_frontmatter {
			// Look for "slug: cs01" or "permalink: /en/newsletters/..." patterns
			// or extract from the title like "Newsletter #123"
			if let Some(rest) = line.strip_prefix("title:") {
				let title = rest.trim();
				if let Some(idx) = title.find('#') {
					let num_str: String =
						title[idx + 1..].chars().take_while(|c| c.is_ascii_digit()).collect();
					if let Ok(num) = num_str.parse::<u32>() {
						return Some(num);
					}
				}
			}
		}
	}
	None
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_extract_newsletter_title() {
		let body = r#"---
title: "Bitcoin Optech Newsletter #283"
permalink: /en/newsletters/2024/01/03/
---

Content here."#;
		assert_eq!(
			extract_newsletter_title(body, "2024-01-03-newsletter"),
			"Bitcoin Optech Newsletter #283"
		);
	}

	#[test]
	fn test_extract_newsletter_number() {
		let body = r#"---
title: "Bitcoin Optech Newsletter #283"
---"#;
		assert_eq!(extract_newsletter_number(body), Some(283));
	}

	#[test]
	fn test_extract_newsletter_title_fallback() {
		let body = "No frontmatter here";
		assert_eq!(extract_newsletter_title(body, "2024-01-03"), "Optech Newsletter 2024-01-03");
	}
}
