use std::collections::HashSet;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, NaiveDateTime, Utc};
use regex::Regex;
use reqwest::Client;
use tracing::{debug, info, warn};

use bkb_core::model::{Document, SourceType};

use super::{SyncPage, SyncSource};
use crate::html_util::{html_unescape, strip_html_tags};
use crate::rate_limiter::RateLimiter;

/// Board IDs for technically relevant sections of BitcoinTalk.
const TECHNICAL_BOARDS: &[u32] = &[
	1,  // Bitcoin Discussion
	6,  // Development & Technical Discussion
	4,  // Bitcoin Technical Support
	12, // Project Development
	14, // Mining
	37, // Mining: Pair mining
	40, // Mining: Mining software (Pair mining)
	41, // Mining: Hardware
	42, // Mining: Pools
	76, // Mining: Cloud Mining
	7,  // Economics
];

/// Default delay between HTTP requests to BitcoinTalk.
const REQUEST_DELAY: Duration = Duration::from_secs(5);

/// Default maximum consecutive 404s before transitioning to tail mode.
const DEFAULT_MAX_MISSES: u32 = 50;

/// Reduced misses threshold for dev_subset mode.
const DEV_SUBSET_MAX_MISSES: u32 = 5;

/// Sync source for BitcoinTalk forum posts.
///
/// Scrapes the HTML pages of BitcoinTalk (SMF 2.0 forum) to index topics
/// and posts from technically relevant boards.
///
/// Two-phase cursor approach:
/// - **Topic mode** (`topic:{id}|misses:{n}`): Walk topic IDs sequentially.
/// - **Tail mode** (`tail:{highest_id}`): Poll recent posts page for new activity.
pub struct BitcointalkSyncSource {
	client: Client,
	request_delay: Duration,
	board_whitelist: HashSet<u32>,
	start_topic: u64,
	max_consecutive_misses: u32,
}

impl BitcointalkSyncSource {
	pub fn new(start_topic: u64, dev_subset: bool) -> Self {
		let board_whitelist: HashSet<u32> = TECHNICAL_BOARDS.iter().copied().collect();
		Self {
			client: Client::builder()
				.user_agent("bkb/0.1")
				.build()
				.unwrap_or_else(|_| Client::new()),
			request_delay: REQUEST_DELAY,
			board_whitelist,
			start_topic,
			max_consecutive_misses: if dev_subset {
				DEV_SUBSET_MAX_MISSES
			} else {
				DEFAULT_MAX_MISSES
			},
		}
	}

	/// Fetch a URL with a self-imposed rate delay (independent of the GitHub rate limiter).
	async fn fetch_html(&self, url: &str) -> Result<Option<String>> {
		tokio::time::sleep(self.request_delay).await;

		debug!(url = %url, "fetching BitcoinTalk page");

		let response = self
			.client
			.get(url)
			.send()
			.await
			.with_context(|| format!("failed to fetch {}", url))?;

		if response.status() == reqwest::StatusCode::NOT_FOUND {
			return Ok(None);
		}

		if !response.status().is_success() {
			let status = response.status();
			let body = response.text().await.unwrap_or_default();

			// Detect rate limiting
			if body.contains("you are requesting too fast")
				|| body.contains("Please try again in a few seconds")
			{
				anyhow::bail!("BitcoinTalk rate limit hit for {}", url);
			}

			anyhow::bail!("BitcoinTalk returned {} for {}: {}", status, url, body);
		}

		let body =
			response.text().await.with_context(|| format!("failed to read body from {}", url))?;

		// Check for rate limiting in response body even on 200
		if body.contains("you are requesting too fast")
			|| body.contains("Please try again in a few seconds")
		{
			anyhow::bail!("BitcoinTalk rate limit hit for {}", url);
		}

		Ok(Some(body))
	}

	/// Fetch all pages of a topic, returning topic + post documents.
	async fn fetch_topic(&self, topic_id: u64) -> Result<Option<Vec<Document>>> {
		let first_url = format!("https://bitcointalk.org/index.php?topic={}.0", topic_id);
		let html = match self.fetch_html(&first_url).await? {
			Some(h) => h,
			None => return Ok(None),
		};

		// Check board from breadcrumb
		let board_id = match extract_board_id(&html) {
			Some(id) => id,
			None => {
				debug!(topic_id, "could not extract board ID, skipping");
				return Ok(None);
			},
		};

		if !self.board_whitelist.contains(&board_id) {
			debug!(topic_id, board_id, "board not whitelisted, skipping");
			// Return empty vec to signal "valid topic but skipped"
			return Ok(Some(vec![]));
		}

		let board_name = extract_board_name(&html).unwrap_or_default();
		let topic_title = extract_topic_title(&html);

		let metadata = serde_json::json!({
			"board_id": board_id,
			"board_name": board_name,
		});

		let mut all_posts = parse_posts(&html);
		let total_pages = extract_page_count(&html, topic_id);

		// Fetch remaining pages
		for page_idx in 1..total_pages {
			let offset = page_idx * 20;
			let page_url =
				format!("https://bitcointalk.org/index.php?topic={}.{}", topic_id, offset);
			match self.fetch_html(&page_url).await? {
				Some(page_html) => {
					all_posts.extend(parse_posts(&page_html));
				},
				None => break,
			}
		}

		let mut documents = Vec::new();

		// Create topic document from first post
		let first_post = all_posts.first();
		let topic_source_id = topic_id.to_string();
		let topic_doc_id = format!("bitcointalk_topic::{}", topic_id);

		documents.push(Document {
			id: topic_doc_id.clone(),
			source_type: SourceType::BitcointalkTopic,
			source_repo: None,
			source_id: topic_source_id,
			title: topic_title,
			body: first_post.map(|p| p.body.clone()),
			author: first_post.map(|p| p.author.clone()),
			author_id: first_post.and_then(|p| p.author_id.clone()),
			created_at: first_post.map(|p| p.date).unwrap_or_else(Utc::now),
			updated_at: None,
			parent_id: None,
			metadata: Some(metadata.clone()),
			seq: None,
		});

		// Create post documents (skip first post since it's the topic body)
		for (idx, post) in all_posts.iter().enumerate() {
			if idx == 0 {
				continue;
			}
			let post_doc_id = format!("bitcointalk_post::{}", post.msg_id);
			documents.push(Document {
				id: post_doc_id,
				source_type: SourceType::BitcointalkPost,
				source_repo: None,
				source_id: post.msg_id.to_string(),
				title: None,
				body: Some(post.body.clone()),
				author: Some(post.author.clone()),
				author_id: post.author_id.clone(),
				created_at: post.date,
				updated_at: None,
				parent_id: Some(topic_doc_id.clone()),
				metadata: Some(metadata.clone()),
				seq: Some(idx as i64),
			});
		}

		Ok(Some(documents))
	}

	/// Fetch the recent posts page and return topic IDs with new activity.
	async fn fetch_recent_topic_ids(&self) -> Result<Vec<u64>> {
		let url = "https://bitcointalk.org/index.php?action=recent";
		let html = match self.fetch_html(url).await? {
			Some(h) => h,
			None => return Ok(vec![]),
		};

		Ok(parse_recent_topic_ids(&html))
	}
}

#[async_trait]
impl SyncSource for BitcointalkSyncSource {
	async fn fetch_page(
		&self, cursor: Option<&str>, _rate_limiter: &RateLimiter,
	) -> Result<SyncPage> {
		let (mode, _) = parse_cursor(cursor, self.start_topic);

		match mode {
			CursorMode::Topic { topic_id, misses } => {
				match self.fetch_topic(topic_id).await {
					Ok(Some(documents)) if documents.is_empty() => {
						// Topic exists but board not whitelisted -- skip, reset misses
						let next = format!("topic:{}|misses:0", topic_id + 1);
						info!(topic_id, "skipped (board not whitelisted)");
						Ok(SyncPage {
							documents: vec![],
							references: vec![],
							next_cursor: Some(next),
						})
					},
					Ok(Some(documents)) => {
						let doc_count = documents.len();
						let next = format!("topic:{}|misses:0", topic_id + 1);
						info!(topic_id, documents = doc_count, "fetched topic");
						Ok(SyncPage { documents, references: vec![], next_cursor: Some(next) })
					},
					Ok(None) => {
						// 404 -- topic doesn't exist
						let new_misses = misses + 1;
						if new_misses >= self.max_consecutive_misses {
							let tail_cursor = format!("tail:{}", topic_id);
							info!(
								topic_id,
								misses = new_misses,
								"reached max consecutive misses, switching to tail mode"
							);
							Ok(SyncPage {
								documents: vec![],
								references: vec![],
								next_cursor: Some(tail_cursor),
							})
						} else {
							let next = format!("topic:{}|misses:{}", topic_id + 1, new_misses);
							debug!(topic_id, misses = new_misses, "topic not found (404)");
							Ok(SyncPage {
								documents: vec![],
								references: vec![],
								next_cursor: Some(next),
							})
						}
					},
					Err(e) => {
						warn!(topic_id, error = %e, "failed to fetch topic");
						Err(e)
					},
				}
			},
			CursorMode::Tail { highest_topic_id } => {
				info!(highest_topic_id, "tail mode: checking recent posts");
				let recent_ids = self.fetch_recent_topic_ids().await?;
				let mut all_documents = Vec::new();

				for &tid in &recent_ids {
					match self.fetch_topic(tid).await {
						Ok(Some(docs)) if !docs.is_empty() => {
							info!(
								topic_id = tid,
								documents = docs.len(),
								"re-scraped active topic"
							);
							all_documents.extend(docs);
						},
						Ok(_) => {},
						Err(e) => {
							warn!(topic_id = tid, error = %e, "failed to fetch recent topic");
						},
					}
				}

				info!(
					recent_topics = recent_ids.len(),
					documents = all_documents.len(),
					"tail mode fetch complete"
				);

				// Return None to pause until next poll_interval
				Ok(SyncPage { documents: all_documents, references: vec![], next_cursor: None })
			},
		}
	}

	fn poll_interval(&self) -> Duration {
		Duration::from_secs(3600) // 1 hour
	}

	fn name(&self) -> &str {
		"bitcointalk"
	}
}

// -- Cursor parsing --

enum CursorMode {
	Topic { topic_id: u64, misses: u32 },
	Tail { highest_topic_id: u64 },
}

fn parse_cursor(cursor: Option<&str>, start_topic: u64) -> (CursorMode, ()) {
	match cursor {
		None => (CursorMode::Topic { topic_id: start_topic, misses: 0 }, ()),
		Some(c) => {
			if let Some(rest) = c.strip_prefix("tail:") {
				let highest = rest.parse::<u64>().unwrap_or(start_topic);
				(CursorMode::Tail { highest_topic_id: highest }, ())
			} else if let Some(rest) = c.strip_prefix("topic:") {
				// Format: topic:{id}|misses:{n}
				let parts: Vec<&str> = rest.split('|').collect();
				let topic_id = parts[0].parse::<u64>().unwrap_or(start_topic);
				let misses = parts
					.get(1)
					.and_then(|p| p.strip_prefix("misses:"))
					.and_then(|n| n.parse::<u32>().ok())
					.unwrap_or(0);
				(CursorMode::Topic { topic_id, misses }, ())
			} else {
				// Unknown format, start fresh
				(CursorMode::Topic { topic_id: start_topic, misses: 0 }, ())
			}
		},
	}
}

// -- HTML parsing --

/// A parsed post from a BitcoinTalk topic page.
#[derive(Debug, Clone)]
struct ParsedPost {
	msg_id: u64,
	author: String,
	author_id: Option<String>,
	date: DateTime<Utc>,
	body: String,
}

/// Extract the board ID from the breadcrumb navigation.
///
/// Looks for `board=N` in breadcrumb links.
fn extract_board_id(html: &str) -> Option<u32> {
	thread_local! {
		static RE: Regex = Regex::new(r#"<a[^>]+href="[^"]*\bboard=(\d+)"#).unwrap();
	}
	RE.with(|re| {
		// The last board= link in the breadcrumb is the specific board
		let mut last_id = None;
		for cap in re.captures_iter(html) {
			if let Some(id) = cap.get(1).and_then(|m| m.as_str().parse::<u32>().ok()) {
				last_id = Some(id);
			}
		}
		last_id
	})
}

/// Extract the board name from the breadcrumb.
fn extract_board_name(html: &str) -> Option<String> {
	thread_local! {
		static RE: Regex = Regex::new(r#"<a[^>]+href="[^"]*\bboard=\d+[^"]*"[^>]*>([^<]+)</a>"#).unwrap();
	}
	RE.with(|re| {
		let mut last_name = None;
		for cap in re.captures_iter(html) {
			if let Some(name) = cap.get(1) {
				last_name = Some(html_unescape(name.as_str().trim()).to_string());
			}
		}
		last_name
	})
}

/// Extract the topic title from the HTML `<title>` tag.
fn extract_topic_title(html: &str) -> Option<String> {
	let start = html.find("<title>")? + "<title>".len();
	let end = html[start..].find("</title>")?;
	let raw = &html[start..start + end];
	let text = html_unescape(raw.trim());
	if text.is_empty() {
		None
	} else {
		Some(text)
	}
}

/// Parse posts from a BitcoinTalk topic page.
///
/// Posts are in `<div class="post">` elements. Each post area contains
/// an anchor `<a name="msg{ID}">`, author in `action=profile;u=` links,
/// date in the smalltext area, and body in the post div.
fn parse_posts(html: &str) -> Vec<ParsedPost> {
	thread_local! {
		static RE_MSG: Regex = Regex::new(
			r#"<a\s+[^>]*name="msg(\d+)"#
		).unwrap();
		static RE_AUTHOR: Regex = Regex::new(
			r#"action=profile;u=(\d+)"[^>]*>([^<]+)</a>"#
		).unwrap();
		static RE_DATE: Regex = Regex::new(
			r#"<div\s+class="smalltext">\s*(?:&laquo;\s*)?<b>\s*(?:on:|&laquo;)?\s*</b>\s*([\w\s,:]+(?:AM|PM))"#
		).unwrap();
		// Broader date pattern for "Today at" and "Month DD, YYYY, HH:MM:SS AM/PM"
		static RE_DATE_ALT: Regex = Regex::new(
			r#"(\w+ \d{1,2}, \d{4}, \d{1,2}:\d{2}:\d{2} (?:AM|PM))"#
		).unwrap();
		static RE_POST_BODY: Regex = Regex::new(
			r#"<div\s+class="post"[^>]*>([\s\S]*?)</div>"#
		).unwrap();
	}

	let mut posts = Vec::new();

	// Split HTML into post sections using message anchors
	let msg_positions: Vec<(u64, usize)> = RE_MSG.with(|re| {
		re.captures_iter(html)
			.filter_map(|cap| {
				let msg_id = cap.get(1)?.as_str().parse::<u64>().ok()?;
				let pos = cap.get(0)?.start();
				Some((msg_id, pos))
			})
			.collect()
	});

	for (idx, &(msg_id, start_pos)) in msg_positions.iter().enumerate() {
		let end_pos = msg_positions.get(idx + 1).map(|&(_, p)| p).unwrap_or(html.len());
		let section = &html[start_pos..end_pos];

		// Extract author
		let (author, author_id) = RE_AUTHOR.with(|re| {
			re.captures(section)
				.map(|cap| {
					let uid = cap.get(1).map(|m| m.as_str().to_string());
					let name =
						cap.get(2).map(|m| html_unescape(m.as_str().trim())).unwrap_or_default();
					(name, uid)
				})
				.unwrap_or_else(|| ("Unknown".to_string(), None))
		});

		// Extract date
		let date = RE_DATE_ALT
			.with(|re| {
				re.captures(section)
					.and_then(|cap| cap.get(1).and_then(|m| parse_bitcointalk_date(m.as_str())))
			})
			.unwrap_or_else(Utc::now);

		// Extract post body
		let body = RE_POST_BODY
			.with(|re| {
				re.captures(section).map(|cap| {
					let raw = cap.get(1).map(|m| m.as_str()).unwrap_or("");
					let text = strip_html_tags(raw);
					html_unescape(text.trim())
				})
			})
			.unwrap_or_default();

		if !body.is_empty() {
			posts.push(ParsedPost { msg_id, author, author_id, date, body });
		}
	}

	posts
}

/// Extract the number of pages in a topic from pagination links.
fn extract_page_count(html: &str, _topic_id: u64) -> usize {
	thread_local! {
		static RE: Regex = Regex::new(r#"<a\s+class="navPages"[^>]*>(\d+)</a>"#).unwrap();
	}
	RE.with(|re| {
		re.captures_iter(html)
			.filter_map(|cap| cap.get(1)?.as_str().parse::<usize>().ok())
			.max()
			.unwrap_or(1)
	})
}

/// Parse topic IDs from the recent posts page.
fn parse_recent_topic_ids(html: &str) -> Vec<u64> {
	thread_local! {
		static RE: Regex = Regex::new(r#"topic=(\d+)\.msg\d+#msg\d+"#).unwrap();
	}
	let mut seen = HashSet::new();
	RE.with(|re| {
		re.captures_iter(html)
			.filter_map(|cap| cap.get(1)?.as_str().parse::<u64>().ok())
			.filter(|id| seen.insert(*id))
			.collect()
	})
}

/// Parse BitcoinTalk date format: "March 10, 2026, 06:46:59 PM"
fn parse_bitcointalk_date(s: &str) -> Option<DateTime<Utc>> {
	let s = s.trim();
	NaiveDateTime::parse_from_str(s, "%B %d, %Y, %I:%M:%S %p").ok().map(|naive| naive.and_utc())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_parse_cursor_none() {
		let (mode, _) = parse_cursor(None, 1);
		match mode {
			CursorMode::Topic { topic_id, misses } => {
				assert_eq!(topic_id, 1);
				assert_eq!(misses, 0);
			},
			_ => panic!("expected Topic mode"),
		}
	}

	#[test]
	fn test_parse_cursor_topic() {
		let (mode, _) = parse_cursor(Some("topic:42|misses:3"), 1);
		match mode {
			CursorMode::Topic { topic_id, misses } => {
				assert_eq!(topic_id, 42);
				assert_eq!(misses, 3);
			},
			_ => panic!("expected Topic mode"),
		}
	}

	#[test]
	fn test_parse_cursor_tail() {
		let (mode, _) = parse_cursor(Some("tail:1000"), 1);
		match mode {
			CursorMode::Tail { highest_topic_id } => {
				assert_eq!(highest_topic_id, 1000);
			},
			_ => panic!("expected Tail mode"),
		}
	}

	#[test]
	fn test_parse_bitcointalk_date() {
		let dt = parse_bitcointalk_date("March 10, 2026, 06:46:59 PM");
		assert!(dt.is_some());
		let dt = dt.unwrap();
		assert_eq!(dt.date_naive(), chrono::NaiveDate::from_ymd_opt(2026, 3, 10).unwrap());
	}

	#[test]
	fn test_parse_bitcointalk_date_am() {
		let dt = parse_bitcointalk_date("January 01, 2009, 12:00:00 AM");
		assert!(dt.is_some());
	}

	#[test]
	fn test_extract_board_id() {
		let html = r#"
		<div id="main_content_section">
		<div class="navigate_section">
			<ul>
				<li><a href="https://bitcointalk.org/index.php">Bitcoin Forum</a></li>
				<li><a href="https://bitcointalk.org/index.php?board=1.0">Bitcoin Discussion</a></li>
			</ul>
		</div>
		"#;
		assert_eq!(extract_board_id(html), Some(1));
	}

	#[test]
	fn test_extract_board_id_nested() {
		let html = r#"
		<a href="https://bitcointalk.org/index.php?board=14.0">Mining</a>
		<a href="https://bitcointalk.org/index.php?board=41.0">Hardware</a>
		"#;
		// Should return the last (most specific) board
		assert_eq!(extract_board_id(html), Some(41));
	}

	#[test]
	fn test_extract_board_name() {
		let html = r#"
		<a href="https://bitcointalk.org/index.php?board=6.0">Development &amp; Technical Discussion</a>
		"#;
		assert_eq!(extract_board_name(html).as_deref(), Some("Development & Technical Discussion"));
	}

	#[test]
	fn test_extract_topic_title() {
		let html = r#"<html><head><title>Bitcoin v0.1 released</title></head>"#;
		assert_eq!(extract_topic_title(html).as_deref(), Some("Bitcoin v0.1 released"));
	}

	#[test]
	fn test_parse_recent_topic_ids() {
		let html = r#"
		<a href="https://bitcointalk.org/index.php?topic=100.msg500#msg500">Post 1</a>
		<a href="https://bitcointalk.org/index.php?topic=200.msg600#msg600">Post 2</a>
		<a href="https://bitcointalk.org/index.php?topic=100.msg501#msg501">Post 3</a>
		"#;
		let ids = parse_recent_topic_ids(html);
		assert_eq!(ids.len(), 2);
		assert!(ids.contains(&100));
		assert!(ids.contains(&200));
	}

	#[test]
	fn test_parse_posts_basic() {
		let html = r#"
		<a name="msg12345">
		<a href="https://bitcointalk.org/index.php?action=profile;u=42">satoshi</a>
		<div class="smalltext">January 09, 2009, 03:15:00 AM</div>
		<div class="post" id="msg_12345">Bitcoin is great.</div>
		"#;
		let posts = parse_posts(html);
		assert_eq!(posts.len(), 1);
		assert_eq!(posts[0].msg_id, 12345);
		assert_eq!(posts[0].author, "satoshi");
		assert_eq!(posts[0].author_id.as_deref(), Some("42"));
		assert!(posts[0].body.contains("Bitcoin is great"));
	}

	#[test]
	fn test_extract_page_count_single() {
		let html = r#"<div>No pagination</div>"#;
		assert_eq!(extract_page_count(html, 1), 1);
	}

	#[test]
	fn test_extract_page_count_multi() {
		let html = r#"
		<a class="navPages" href="...">1</a>
		<a class="navPages" href="...">2</a>
		<a class="navPages" href="...">3</a>
		"#;
		assert_eq!(extract_page_count(html, 1), 3);
	}

	#[test]
	fn test_board_whitelist() {
		let source = BitcointalkSyncSource::new(1, false);
		assert!(source.board_whitelist.contains(&1));
		assert!(source.board_whitelist.contains(&6));
		assert!(source.board_whitelist.contains(&41));
		assert!(!source.board_whitelist.contains(&99));
	}

	#[test]
	fn test_dev_subset_misses() {
		let source = BitcointalkSyncSource::new(1, true);
		assert_eq!(source.max_consecutive_misses, DEV_SUBSET_MAX_MISSES);

		let source = BitcointalkSyncSource::new(1, false);
		assert_eq!(source.max_consecutive_misses, DEFAULT_MAX_MISSES);
	}

	#[test]
	fn test_name() {
		let source = BitcointalkSyncSource::new(1, false);
		assert_eq!(source.name(), "bitcointalk");
	}

	#[test]
	fn test_poll_interval() {
		let source = BitcointalkSyncSource::new(1, false);
		assert_eq!(source.poll_interval(), Duration::from_secs(3600));
	}
}
