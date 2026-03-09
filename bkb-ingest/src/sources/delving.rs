use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;
use tracing::{debug, info, warn};

use bkb_core::model::{Document, RefType, Reference, SourceType};

use super::{SyncPage, SyncSource};
use crate::rate_limiter::RateLimiter;

const DELVING_BASE: &str = "https://delvingbitcoin.org";

/// Expected number of topics per page from the Discourse `/latest.json` endpoint.
///
/// Discourse typically returns 30 topics per page. When we receive fewer, we
/// know we have reached the last page.
const TOPICS_PER_PAGE: usize = 30;

/// Sync source for Delving Bitcoin (Discourse forum).
///
/// Fetches topics from `GET /latest.json?page={n}` and, for each topic,
/// fetches the full topic JSON from `GET /t/{id}.json`. The opening post
/// becomes a `DelvingTopic` document and subsequent posts become
/// `DelvingPost` documents.
///
/// The cursor is the page number (starting at 0). When a page returns
/// fewer topics than expected, we consider ourselves caught up and
/// return `next_cursor = None`.
pub struct DelvingSyncSource {
	client: Client,
}

impl DelvingSyncSource {
	pub fn new() -> Self {
		Self { client: Client::new() }
	}

	fn build_request(&self, url: &str) -> reqwest::RequestBuilder {
		self.client.get(url).header("User-Agent", "bkb/0.1").header("Accept", "application/json")
	}
}

#[async_trait]
impl SyncSource for DelvingSyncSource {
	async fn fetch_page(
		&self, cursor: Option<&str>, rate_limiter: &RateLimiter,
	) -> Result<SyncPage> {
		let page: u64 = cursor.and_then(|c| c.parse().ok()).unwrap_or(0);

		rate_limiter.acquire().await;

		let latest_url = format!("{}/latest.json?page={}", DELVING_BASE, page);
		debug!(url = %latest_url, "fetching Delving Bitcoin topics page");

		let response = self
			.build_request(&latest_url)
			.send()
			.await
			.context("failed to fetch Delving Bitcoin latest topics")?;

		let status = response.status();
		if !status.is_success() {
			let body = response.text().await.unwrap_or_default();
			anyhow::bail!("Delving Bitcoin API returned {}: {}", status, body);
		}

		let response_text =
			response.text().await.context("failed to read Delving Bitcoin response")?;
		let latest: DiscourseLatestResponse =
			serde_json::from_str(&response_text).with_context(|| {
				format!(
					"failed to parse Delving Bitcoin latest response (first 500 chars): {}",
					&response_text[..response_text.len().min(500)]
				)
			})?;

		let topics = latest.topic_list.topics;

		info!(
			source = %self.name(),
			count = topics.len(),
			page = page,
			"fetched topics page"
		);

		if topics.is_empty() {
			return Ok(SyncPage {
				documents: Vec::new(),
				references: Vec::new(),
				next_cursor: None,
			});
		}

		let mut documents = Vec::new();
		let mut references = Vec::new();

		for topic_summary in &topics {
			rate_limiter.acquire().await;

			let topic_url = format!("{}/t/{}.json", DELVING_BASE, topic_summary.id);
			debug!(url = %topic_url, "fetching Delving Bitcoin topic");

			let topic_response = match self.build_request(&topic_url).send().await {
				Ok(resp) => resp,
				Err(e) => {
					warn!(topic_id = topic_summary.id, error = %e, "failed to fetch topic, skipping");
					continue;
				},
			};

			let topic_status = topic_response.status();
			if !topic_status.is_success() {
				let body = topic_response.text().await.unwrap_or_default();
				warn!(
					topic_id = topic_summary.id,
					status = %topic_status,
					body = %body,
					"topic fetch returned error, skipping"
				);
				continue;
			}

			let topic: DiscourseTopicResponse = match topic_response.json().await {
				Ok(t) => t,
				Err(e) => {
					warn!(topic_id = topic_summary.id, error = %e, "failed to parse topic, skipping");
					continue;
				},
			};

			let posts = topic.post_stream.posts;
			if posts.is_empty() {
				continue;
			}

			// The topic document ID (used as parent_id for reply posts).
			let topic_source_id = topic.id.to_string();
			let topic_doc_id = Document::make_id(&SourceType::DelvingTopic, None, &topic_source_id);

			// First post (post_number == 1) becomes the DelvingTopic document.
			let first_post = &posts[0];

			let topic_metadata = serde_json::json!({
				"category_id": topic_summary.category_id,
				"tags": topic_summary.tags.iter().map(|t| &t.name).collect::<Vec<_>>(),
			});

			documents.push(Document {
				id: topic_doc_id.clone(),
				source_type: SourceType::DelvingTopic,
				source_repo: None,
				source_id: topic_source_id,
				title: Some(topic.title.clone()),
				body: Some(first_post.cooked.clone()),
				author: Some(first_post.username.clone()),
				author_id: Some(first_post.id.to_string()),
				created_at: first_post.created_at,
				updated_at: Some(first_post.updated_at),
				parent_id: None,
				metadata: Some(topic_metadata),
				seq: Some(first_post.post_number as i64),
			});

			// Extract references from the first post body.
			references.extend(extract_delving_refs(&first_post.cooked, &topic_doc_id));

			// Subsequent posts become DelvingPost documents.
			for post in posts.iter().skip(1) {
				let post_source_id = post.id.to_string();
				let post_doc_id =
					Document::make_id(&SourceType::DelvingPost, None, &post_source_id);

				documents.push(Document {
					id: post_doc_id.clone(),
					source_type: SourceType::DelvingPost,
					source_repo: None,
					source_id: post_source_id,
					title: None,
					body: Some(post.cooked.clone()),
					author: Some(post.username.clone()),
					author_id: Some(post.id.to_string()),
					created_at: post.created_at,
					updated_at: Some(post.updated_at),
					parent_id: Some(topic_doc_id.clone()),
					metadata: None,
					seq: Some(post.post_number as i64),
				});

				// Extract references from post body.
				references.extend(extract_delving_refs(&post.cooked, &post_doc_id));

				// Add a RepliesTo reference if this post replies to another post.
				if let Some(reply_to) = post.reply_to_post_number {
					// Find the post ID for the replied-to post number.
					if let Some(parent_post) = posts.iter().find(|p| p.post_number == reply_to) {
						let parent_post_doc_id = if reply_to == 1 {
							topic_doc_id.clone()
						} else {
							Document::make_id(
								&SourceType::DelvingPost,
								None,
								&parent_post.id.to_string(),
							)
						};

						references.push(Reference {
							id: None,
							from_doc_id: post_doc_id.clone(),
							to_doc_id: Some(parent_post_doc_id),
							ref_type: RefType::RepliesTo,
							to_external: None,
							context: None,
						});
					}
				}
			}
		}

		// Determine the next cursor. If we got a full page of topics, there
		// may be more pages to fetch.
		let next_cursor =
			if topics.len() >= TOPICS_PER_PAGE { Some((page + 1).to_string()) } else { None };

		Ok(SyncPage { documents, references, next_cursor })
	}

	fn poll_interval(&self) -> Duration {
		Duration::from_secs(3600) // 1 hour
	}

	fn name(&self) -> &str {
		"delving_bitcoin"
	}
}

/// Extract BIP and BOLT references from Delving Bitcoin post HTML content.
fn extract_delving_refs(html: &str, from_doc_id: &str) -> Vec<Reference> {
	use regex::Regex;

	thread_local! {
		static RE_BIP: Regex = Regex::new(r"(?i)\bBIP[- ]?(\d{1,4})\b").unwrap();
		static RE_BOLT: Regex = Regex::new(r"(?i)\bBOLT[- ]?(\d{1,2})\b").unwrap();
	}

	let mut refs = Vec::new();

	RE_BIP.with(|re| {
		for cap in re.captures_iter(html) {
			refs.push(Reference {
				id: None,
				from_doc_id: from_doc_id.to_string(),
				to_doc_id: None,
				ref_type: RefType::ReferencesBip,
				to_external: Some(format!("BIP-{}", &cap[1])),
				context: Some(cap[0].to_string()),
			});
		}
	});

	RE_BOLT.with(|re| {
		for cap in re.captures_iter(html) {
			refs.push(Reference {
				id: None,
				from_doc_id: from_doc_id.to_string(),
				to_doc_id: None,
				ref_type: RefType::ReferencesBolt,
				to_external: Some(format!("BOLT-{}", &cap[1])),
				context: Some(cap[0].to_string()),
			});
		}
	});

	refs
}

// --- Discourse API response types ---

#[derive(Debug, Deserialize)]
struct DiscourseLatestResponse {
	topic_list: DiscourseTopicList,
}

#[derive(Debug, Deserialize)]
struct DiscourseTopicList {
	topics: Vec<DiscourseTopicSummary>,
}

#[derive(Debug, Deserialize)]
struct DiscourseTopicSummary {
	id: u64,
	#[allow(dead_code)]
	title: String,
	#[allow(dead_code)]
	created_at: DateTime<Utc>,
	#[allow(dead_code)]
	last_posted_at: Option<DateTime<Utc>>,
	category_id: Option<u64>,
	#[serde(default)]
	tags: Vec<DiscourseTag>,
}

#[derive(Debug, Deserialize)]
struct DiscourseTag {
	name: String,
}

#[derive(Debug, Deserialize)]
struct DiscourseTopicResponse {
	id: u64,
	title: String,
	post_stream: DiscoursePostStream,
}

#[derive(Debug, Deserialize)]
struct DiscoursePostStream {
	posts: Vec<DiscoursePost>,
}

#[derive(Debug, Deserialize)]
struct DiscoursePost {
	id: u64,
	post_number: u64,
	username: String,
	cooked: String,
	created_at: DateTime<Utc>,
	updated_at: DateTime<Utc>,
	reply_to_post_number: Option<u64>,
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_extract_bip_refs() {
		let refs = extract_delving_refs(
			"<p>This relates to BIP-340 and BIP 341 taproot</p>",
			"delving_topic:42",
		);
		let bip_refs: Vec<_> =
			refs.iter().filter(|r| r.ref_type == RefType::ReferencesBip).collect();
		assert_eq!(bip_refs.len(), 2);
		assert!(bip_refs.iter().any(|r| r.to_external.as_deref() == Some("BIP-340")));
		assert!(bip_refs.iter().any(|r| r.to_external.as_deref() == Some("BIP-341")));
	}

	#[test]
	fn test_extract_bolt_refs() {
		let refs = extract_delving_refs(
			"<p>See BOLT-11 for invoice format and BOLT 12 offers</p>",
			"delving_post:99",
		);
		let bolt_refs: Vec<_> =
			refs.iter().filter(|r| r.ref_type == RefType::ReferencesBolt).collect();
		assert_eq!(bolt_refs.len(), 2);
		assert!(bolt_refs.iter().any(|r| r.to_external.as_deref() == Some("BOLT-11")));
		assert!(bolt_refs.iter().any(|r| r.to_external.as_deref() == Some("BOLT-12")));
	}

	#[test]
	fn test_extract_no_refs() {
		let refs = extract_delving_refs("<p>No references here</p>", "delving_topic:1");
		assert!(refs.is_empty());
	}

	#[test]
	fn test_delving_sync_source_name() {
		let source = DelvingSyncSource::new();
		assert_eq!(source.name(), "delving_bitcoin");
	}

	#[test]
	fn test_delving_sync_source_poll_interval() {
		let source = DelvingSyncSource::new();
		assert_eq!(source.poll_interval(), Duration::from_secs(3600));
	}
}
