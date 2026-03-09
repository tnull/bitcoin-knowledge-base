use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use regex::Regex;
use reqwest::Client;
use tracing::{debug, info, warn};

use bkb_core::model::{Document, SourceType};

use super::{SyncPage, SyncSource};
use crate::rate_limiter::RateLimiter;

/// Number of messages to fetch per page.
const PAGE_SIZE: u32 = 25;

/// Sync source for mailing list archives hosted on mail-archive.com.
///
/// Messages are sequentially numbered (`msg00001.html` .. `msgNNNNN.html`).
/// The cursor is the last fetched message number. On initial sync, starts
/// from message 1 and works forward.
pub struct MailArchiveSyncSource {
	/// e.g., `"lightning-dev@lists.linuxfoundation.org"`
	list_address: String,
	/// Short name for logging / source_id, e.g., `"lightning-dev"`
	list_name: String,
	client: Client,
}

impl MailArchiveSyncSource {
	pub fn new(list_address: &str, list_name: &str) -> Self {
		Self {
			list_address: list_address.to_string(),
			list_name: list_name.to_string(),
			client: Client::new(),
		}
	}

	/// URL for a specific message number.
	fn message_url(&self, num: u32) -> String {
		format!("https://www.mail-archive.com/{}/msg{:05}.html", self.list_address, num)
	}

	/// Fetch and parse a single message page. Returns `None` if the message
	/// doesn't exist (404).
	async fn fetch_message(
		&self, num: u32, rate_limiter: &RateLimiter,
	) -> Result<Option<ParsedMessage>> {
		let url = self.message_url(num);
		rate_limiter.acquire().await;

		debug!(url = %url, "fetching mail-archive message");

		let response = self
			.client
			.get(&url)
			.header("User-Agent", "bkb/0.1")
			.send()
			.await
			.with_context(|| format!("failed to fetch {}", url))?;

		if response.status() == reqwest::StatusCode::NOT_FOUND {
			return Ok(None);
		}

		if !response.status().is_success() {
			let status = response.status();
			let body = response.text().await.unwrap_or_default();
			anyhow::bail!("mail-archive returned {} for {}: {}", status, url, body);
		}

		let html =
			response.text().await.with_context(|| format!("failed to read body from {}", url))?;

		Ok(Some(parse_message_html(&html, num)))
	}
}

#[async_trait]
impl SyncSource for MailArchiveSyncSource {
	async fn fetch_page(
		&self, cursor: Option<&str>, rate_limiter: &RateLimiter,
	) -> Result<SyncPage> {
		let start_num = cursor.and_then(|c| c.parse::<u32>().ok()).map(|n| n + 1).unwrap_or(1);

		let mut documents = Vec::new();
		let mut last_num = None;
		let mut consecutive_missing = 0u32;

		for num in start_num..start_num + PAGE_SIZE {
			match self.fetch_message(num, rate_limiter).await {
				Ok(Some(msg)) => {
					consecutive_missing = 0;
					last_num = Some(num);

					let source_id = format!("msg{:05}", num);
					let id = Document::make_id(&SourceType::MailingListMsg, None, &source_id);

					documents.push(Document {
						id,
						source_type: SourceType::MailingListMsg,
						source_repo: None,
						source_id,
						title: msg.subject,
						body: msg.body,
						author: msg.from,
						author_id: None,
						created_at: msg.date.unwrap_or_else(Utc::now),
						updated_at: None,
						parent_id: None,
						metadata: None,
						seq: Some(num as i64),
					});
				},
				Ok(None) => {
					// 404 -- message doesn't exist. After several consecutive
					// 404s we've probably reached the end of the archive.
					consecutive_missing += 1;
					if consecutive_missing >= 3 {
						debug!(
							source = %self.list_name,
							last_num = ?last_num,
							"reached end of archive ({} consecutive 404s)",
							consecutive_missing
						);
						break;
					}
				},
				Err(e) => {
					warn!(
						source = %self.list_name,
						num = num,
						error = %e,
						"failed to fetch message, skipping"
					);
				},
			}
		}

		// If we fetched at least one message, set cursor to continue.
		// If we hit the end (consecutive 404s with no new messages), signal
		// caught-up by using the last attempted number as cursor (so we
		// don't re-fetch known 404s) but return no next_cursor to pause.
		let next_cursor = if documents.is_empty() {
			// No new messages -- we're caught up.
			None
		} else {
			// More messages likely exist -- continue from the last one we got.
			last_num.map(|n| n.to_string())
		};

		info!(
			source = %self.list_name,
			start = start_num,
			documents = documents.len(),
			next_cursor = ?next_cursor,
			"fetched mail-archive messages"
		);

		Ok(SyncPage { documents, references: vec![], next_cursor })
	}

	fn poll_interval(&self) -> Duration {
		// This is a dead archive, so poll very infrequently.
		Duration::from_secs(86400) // 24 hours
	}

	fn name(&self) -> &str {
		Box::leak(format!("mail_archive:{}", self.list_name).into_boxed_str())
	}
}

/// Parsed fields from a mail-archive.com message page.
struct ParsedMessage {
	subject: Option<String>,
	from: Option<String>,
	date: Option<DateTime<Utc>>,
	body: Option<String>,
}

/// Parse a mail-archive.com HTML message page.
fn parse_message_html(html: &str, num: u32) -> ParsedMessage {
	let subject = extract_itemprop(html, "name");
	let from = extract_itemprop_author(html);
	let date_str = extract_date(html);
	let date = date_str.as_deref().and_then(parse_date);
	let body = extract_body(html);

	debug!(num = num, subject = ?subject, from = ?from, "parsed message");

	ParsedMessage { subject, from, date, body }
}

/// Extract the subject from `<span itemprop="name">...</span>` inside the
/// subject span.
fn extract_itemprop(html: &str, prop: &str) -> Option<String> {
	let needle = format!("itemprop=\"{}\"", prop);
	let pos = html.find(&needle)?;
	let after = &html[pos + needle.len()..];
	let start = after.find('>')? + 1;
	let end = after[start..].find('<')?;
	let text = &after[start..start + end];
	let text = html_unescape(text.trim());
	if text.is_empty() {
		None
	} else {
		Some(text)
	}
}

/// Extract the author name from the sender span's itemprop="name".
fn extract_itemprop_author(html: &str) -> Option<String> {
	// Find the sender span first, then extract itemprop="name" within it
	let sender_pos = html.find("class=\"sender")?;
	let sender_html = &html[sender_pos..];
	let end_pos = sender_html.find("</span></a></span>")?;
	let sender_block = &sender_html[..end_pos];
	extract_itemprop(sender_block, "name")
}

/// Extract the date string from `<span class="date"><a ...>DATE</a></span>`.
fn extract_date(html: &str) -> Option<String> {
	let pos = html.find("class=\"date\"")?;
	let after = &html[pos..];
	// Find the <a> tag content
	let a_start = after.find('>')? + 1; // end of <span>
	let a_tag_start = after[a_start..].find('>')? + a_start + 1; // end of <a>
	let a_end = after[a_tag_start..].find('<')?;
	let text = &after[a_tag_start..a_tag_start + a_end];
	let text = text.trim();
	if text.is_empty() {
		None
	} else {
		Some(text.to_string())
	}
}

/// Extract the message body from between `<!--X-Body-of-Message-->` and the
/// closing `</div>` of `class="msgBody"`.
fn extract_body(html: &str) -> Option<String> {
	let marker = "<!--X-Body-of-Message-->";
	let start = html.find(marker)? + marker.len();
	let remaining = &html[start..];

	// Body ends at the closing </div> of msgBody
	let end = remaining.find("</div>")?;
	let body_html = &remaining[..end];

	// Strip HTML tags and decode entities
	let text = strip_html_tags(body_html);
	let text = html_unescape(text.trim());

	// Remove the mailing list footer if present
	let text = if let Some(pos) = text.find("_______________________________________________") {
		text[..pos].trim().to_string()
	} else {
		text
	};

	if text.is_empty() {
		None
	} else {
		Some(text)
	}
}

/// Simple HTML tag stripper.
fn strip_html_tags(html: &str) -> String {
	thread_local! {
		static RE_TAGS: Regex = Regex::new(r"<[^>]+>").unwrap();
	}
	RE_TAGS.with(|re| re.replace_all(html, "").to_string())
}

/// Unescape common HTML entities.
fn html_unescape(s: &str) -> String {
	s.replace("&amp;", "&")
		.replace("&lt;", "<")
		.replace("&gt;", ">")
		.replace("&quot;", "\"")
		.replace("&#39;", "'")
		.replace("&nbsp;", " ")
}

/// Parse a date string in RFC 2822 or common email formats.
fn parse_date(s: &str) -> Option<DateTime<Utc>> {
	DateTime::parse_from_rfc2822(s)
		.ok()
		.map(|dt| dt.with_timezone(&Utc))
		.or_else(|| DateTime::parse_from_rfc3339(s).ok().map(|dt| dt.with_timezone(&Utc)))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_message_url() {
		let source =
			MailArchiveSyncSource::new("lightning-dev@lists.linuxfoundation.org", "lightning-dev");
		assert_eq!(
			source.message_url(100),
			"https://www.mail-archive.com/lightning-dev@lists.linuxfoundation.org/msg00100.html"
		);
		assert_eq!(
			source.message_url(3525),
			"https://www.mail-archive.com/lightning-dev@lists.linuxfoundation.org/msg03525.html"
		);
	}

	#[test]
	fn test_parse_message_html() {
		let html = r#"
<div class="msgHead">
<h1><span class="subject"><a href="/search?..."><span itemprop="name">Re: [Lightning-dev] Test subject</span></a></span></h1>
<p class="darkgray font13">
<span class="sender pipe"><a href="/search?..."><span itemprop="author" itemscope itemtype="http://schema.org/Person"><span itemprop="name">Alice via Lightning-dev</span></span></a></span>
<span class="date"><a href="/search?...">Mon, 1 Jan 2024 12:00:00 +0000</a></span>
</p>
</div>
<div itemprop="articleBody" class="msgBody">
<!--X-Body-of-Message-->
<pre>Hello world,

This is a test message.

Regards,
Alice</pre>
</div>
"#;
		let msg = parse_message_html(html, 1);
		assert_eq!(msg.subject.as_deref(), Some("Re: [Lightning-dev] Test subject"));
		assert_eq!(msg.from.as_deref(), Some("Alice via Lightning-dev"));
		assert!(msg.date.is_some());
		assert!(msg.body.as_deref().unwrap().contains("Hello world"));
		assert!(msg.body.as_deref().unwrap().contains("test message"));
	}

	#[test]
	fn test_extract_body_strips_footer() {
		let html = r#"<!--X-Body-of-Message-->
<pre>Actual content here.
_______________________________________________
Lightning-dev mailing list
lightning-dev@lists.linuxfoundation.org
</pre>
</div>"#;
		let body = extract_body(html).unwrap();
		assert_eq!(body, "Actual content here.");
	}

	#[test]
	fn test_html_unescape() {
		assert_eq!(html_unescape("a &amp; b &lt; c"), "a & b < c");
		assert_eq!(html_unescape("&quot;hello&quot;"), "\"hello\"");
	}

	#[test]
	fn test_strip_html_tags() {
		assert_eq!(strip_html_tags("<b>bold</b> text"), "bold text");
		assert_eq!(strip_html_tags("<a href=\"x\">link</a>"), "link");
	}

	#[test]
	fn test_name() {
		let source =
			MailArchiveSyncSource::new("lightning-dev@lists.linuxfoundation.org", "lightning-dev");
		assert_eq!(source.name(), "mail_archive:lightning-dev");
	}

	#[test]
	fn test_parse_date() {
		let dt = parse_date("Mon, 1 Jan 2024 12:00:00 +0000");
		assert!(dt.is_some());

		let dt = parse_date("Wed, 13 Dec 2017 04:54:29 -0800");
		assert!(dt.is_some());
	}
}
