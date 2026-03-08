use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use reqwest::Client;
use tracing::{debug, info, warn};

use bkb_core::model::{Document, SourceType};

use super::{SyncPage, SyncSource};
use crate::rate_limiter::RateLimiter;

const BITCOINDEV_BASE_URL: &str = "https://gnusha.org/pi/bitcoindev";

/// Maximum number of entries to process per page (to keep pages bounded).
const MAX_ENTRIES_PER_PAGE: usize = 50;

/// Sync source for the bitcoin-dev mailing list (public-inbox archive).
///
/// Fetches messages from the public-inbox Atom feed at gnusha.org.
/// For incremental sync, uses the `new.atom` endpoint.
/// For initial sync (no cursor), uses date-sorted queries.
/// Each message becomes one document with `source_type = MailingListMsg`.
/// The cursor is the ISO 8601 date of the last fetched message.
pub struct MailingListSyncSource {
	client: Client,
}

impl MailingListSyncSource {
	pub fn new() -> Self {
		Self { client: Client::new() }
	}

	/// Build the Atom feed URL for a given cursor.
	///
	/// If no cursor is given, fetches all messages from the beginning of the archive.
	/// If a cursor (date) is given, fetches messages after that date.
	fn feed_url(cursor: Option<&str>) -> String {
		match cursor {
			Some(date) => {
				// Use date-sorted query for incremental sync: messages after the cursor date.
				format!("{}/?q=d:{}..&x=A", BITCOINDEV_BASE_URL, date)
			},
			None => {
				// Initial sync: fetch from the very beginning using a wide date range.
				format!("{}/?q=d:..&x=A", BITCOINDEV_BASE_URL)
			},
		}
	}

	/// Build the raw message URL for a given message link.
	///
	/// The link from the Atom feed points to the message page; appending `/raw`
	/// gives the plain-text email.
	fn raw_message_url(link: &str) -> String {
		let trimmed = link.trim_end_matches('/');
		format!("{}/raw", trimmed)
	}

	/// Fetch and parse the Atom feed, returning a list of (link, updated_date) tuples.
	async fn fetch_feed(&self, url: &str, rate_limiter: &RateLimiter) -> Result<Vec<AtomEntry>> {
		rate_limiter.acquire().await;

		debug!(url = %url, "fetching mailing list Atom feed");

		let response = self
			.client
			.get(url)
			.header("User-Agent", "bkb/0.1")
			.send()
			.await
			.with_context(|| format!("failed to fetch Atom feed from {}", url))?;

		let status = response.status();
		if !status.is_success() {
			let body = response.text().await.unwrap_or_default();
			anyhow::bail!("Atom feed returned {} for {}: {}", status, url, body);
		}

		let body = response
			.text()
			.await
			.with_context(|| format!("failed to read Atom feed body from {}", url))?;

		parse_atom_entries(&body)
	}

	/// Fetch the raw email text for a single message.
	async fn fetch_raw_message(&self, url: &str, rate_limiter: &RateLimiter) -> Result<String> {
		rate_limiter.acquire().await;

		debug!(url = %url, "fetching raw mailing list message");

		let response = self
			.client
			.get(url)
			.header("User-Agent", "bkb/0.1")
			.send()
			.await
			.with_context(|| format!("failed to fetch raw message from {}", url))?;

		let status = response.status();
		if !status.is_success() {
			let body = response.text().await.unwrap_or_default();
			anyhow::bail!("raw message returned {} for {}: {}", status, url, body);
		}

		response
			.text()
			.await
			.with_context(|| format!("failed to read raw message body from {}", url))
	}
}

#[async_trait]
impl SyncSource for MailingListSyncSource {
	async fn fetch_page(
		&self, cursor: Option<&str>, rate_limiter: &RateLimiter,
	) -> Result<SyncPage> {
		let feed_url = Self::feed_url(cursor);
		let entries = self.fetch_feed(&feed_url, rate_limiter).await?;

		if entries.is_empty() {
			debug!("no entries in Atom feed, caught up");
			return Ok(SyncPage { documents: vec![], references: vec![], next_cursor: None });
		}

		let mut documents = Vec::new();
		let mut latest_date: Option<NaiveDate> = None;

		// Parse the cursor date so we can skip entries at or before it.
		let cursor_date = cursor.and_then(|c| NaiveDate::parse_from_str(c, "%Y-%m-%d").ok());

		for entry in entries.iter().take(MAX_ENTRIES_PER_PAGE) {
			// Extract the message ID from the link URL.
			let message_id = extract_message_id(&entry.link);
			if message_id.is_empty() {
				warn!(link = %entry.link, "could not extract message ID from link, skipping");
				continue;
			}

			// Parse the entry date and skip entries at or before cursor.
			let entry_date = entry.updated_date();
			if let (Some(cd), Some(ed)) = (cursor_date, entry_date) {
				if ed <= cd {
					continue;
				}
			}

			// Fetch raw message.
			let raw_url = Self::raw_message_url(&entry.link);
			let raw_text = match self.fetch_raw_message(&raw_url, rate_limiter).await {
				Ok(text) => text,
				Err(e) => {
					warn!(url = %raw_url, error = %e, "failed to fetch raw message, skipping");
					continue;
				},
			};

			let parsed = parse_email_headers(&raw_text);

			// Determine the created_at timestamp from the parsed Date header,
			// falling back to the Atom entry's updated timestamp.
			let created_at = parsed
				.date
				.as_deref()
				.and_then(|d| parse_email_date(d))
				.or_else(|| {
					DateTime::parse_from_rfc3339(&entry.updated)
						.ok()
						.map(|dt| dt.with_timezone(&Utc))
				})
				.unwrap_or_else(Utc::now);

			// Track the latest date for the cursor.
			let msg_date = created_at.date_naive();
			if latest_date.map_or(true, |ld| msg_date > ld) {
				latest_date = Some(msg_date);
			}

			let source_id = message_id.to_string();
			let id = Document::make_id(&SourceType::MailingListMsg, None, &source_id);

			let document = Document {
				id,
				source_type: SourceType::MailingListMsg,
				source_repo: None,
				source_id,
				title: parsed.subject,
				body: parsed.body,
				author: parsed.from,
				author_id: None,
				created_at,
				updated_at: None,
				parent_id: None,
				metadata: None,
				seq: None,
			};

			documents.push(document);
		}

		let next_cursor = latest_date.map(|d| d.format("%Y-%m-%d").to_string());

		info!(
			source = "mailing_list",
			count = documents.len(),
			next_cursor = ?next_cursor,
			"fetched mailing list messages"
		);

		Ok(SyncPage { documents, references: vec![], next_cursor })
	}

	fn poll_interval(&self) -> Duration {
		Duration::from_secs(1800) // 30 minutes
	}

	fn name(&self) -> &str {
		"mailing_list:bitcoindev"
	}
}

/// A parsed entry from the Atom feed.
#[derive(Debug, Clone)]
struct AtomEntry {
	/// The link URL (href) for this entry.
	link: String,
	/// The `<updated>` timestamp as a string.
	updated: String,
}

impl AtomEntry {
	/// Parse the updated timestamp to a `NaiveDate`, if possible.
	fn updated_date(&self) -> Option<NaiveDate> {
		// Try RFC 3339 first, then plain date.
		DateTime::parse_from_rfc3339(&self.updated)
			.ok()
			.map(|dt| dt.date_naive())
			.or_else(|| NaiveDate::parse_from_str(&self.updated, "%Y-%m-%d").ok())
	}
}

/// Parse Atom XML entries using simple string-based parsing.
///
/// Extracts `<entry>` blocks and within each, the `<link href="..."/>` and
/// `<updated>` elements.
fn parse_atom_entries(xml: &str) -> Result<Vec<AtomEntry>> {
	let mut entries = Vec::new();
	let mut remaining = xml;

	while let Some(entry_start) = remaining.find("<entry") {
		remaining = &remaining[entry_start..];
		let entry_end = match remaining.find("</entry>") {
			Some(pos) => pos + "</entry>".len(),
			None => break,
		};
		let entry_xml = &remaining[..entry_end];
		remaining = &remaining[entry_end..];

		let link = extract_atom_link(entry_xml).unwrap_or_default();
		let updated = extract_atom_element(entry_xml, "updated").unwrap_or_default();

		if !link.is_empty() && !updated.is_empty() {
			entries.push(AtomEntry { link, updated });
		}
	}

	Ok(entries)
}

/// Extract the `href` attribute from a `<link .../>` element within an entry.
fn extract_atom_link(entry_xml: &str) -> Option<String> {
	// Look for <link with href="..." pattern.
	let link_start = entry_xml.find("<link")?;
	let after_link = &entry_xml[link_start..];

	// Find the end of this element (either /> or >).
	let link_end = after_link.find('>')?.min(after_link.len());
	let link_tag = &after_link[..=link_end];

	// Extract href value.
	let href_start = link_tag.find("href=\"")?;
	let href_value_start = href_start + "href=\"".len();
	let href_end = link_tag[href_value_start..].find('"')?;
	Some(link_tag[href_value_start..href_value_start + href_end].to_string())
}

/// Extract text content of a simple XML element like `<tag>content</tag>`.
fn extract_atom_element(xml: &str, tag: &str) -> Option<String> {
	let open = format!("<{}", tag);
	let close = format!("</{}>", tag);

	let start = xml.find(&open)?;
	let after_open = &xml[start..];
	// Find the closing > of the opening tag.
	let content_start = after_open.find('>')? + 1;
	let content_after = &after_open[content_start..];
	let content_end = content_after.find(&close)?;
	let content = content_after[..content_end].trim();
	if content.is_empty() {
		None
	} else {
		Some(content.to_string())
	}
}

/// Extract the message ID from a public-inbox link URL.
///
/// Given a URL like `https://gnusha.org/pi/bitcoindev/MSG-ID/`, this returns `MSG-ID`.
fn extract_message_id(link: &str) -> String {
	let trimmed = link.trim_end_matches('/');
	// The message ID is the last path segment after the list name.
	if let Some(base_pos) = trimmed.find("/pi/bitcoindev/") {
		let after_base = &trimmed[base_pos + "/pi/bitcoindev/".len()..];
		// The message ID may contain slashes in theory, but typically it doesn't
		// for public-inbox. Take everything after the base.
		if !after_base.is_empty() {
			return after_base.to_string();
		}
	}
	// Fallback: just use the last path segment.
	trimmed.rsplit('/').next().unwrap_or("").to_string()
}

/// Parsed email headers and body.
#[derive(Debug, Clone, Default)]
pub(crate) struct ParsedEmail {
	pub subject: Option<String>,
	pub from: Option<String>,
	pub date: Option<String>,
	pub body: Option<String>,
}

/// Parse email headers from raw email text using simple string parsing.
///
/// Looks for `Subject:`, `From:`, and `Date:` headers in the header section
/// (before the first blank line), then captures the body after the blank line.
pub(crate) fn parse_email_headers(raw: &str) -> ParsedEmail {
	let mut subject = None;
	let mut from = None;
	let mut date = None;
	let mut body_start = None;

	// The header/body boundary is the first blank line.
	let lines: Vec<&str> = raw.lines().collect();
	let mut i = 0;
	let mut current_header: Option<&str> = None;
	let mut current_value = String::new();

	while i < lines.len() {
		let line = lines[i];

		// A blank line marks the end of headers.
		if line.is_empty() {
			// Flush the last header being accumulated.
			flush_header(current_header, &current_value, &mut subject, &mut from, &mut date);
			body_start = Some(i + 1);
			break;
		}

		// Continuation lines (start with whitespace) are part of the previous header.
		if line.starts_with(' ') || line.starts_with('\t') {
			if current_header.is_some() {
				current_value.push(' ');
				current_value.push_str(line.trim());
			}
			i += 1;
			continue;
		}

		// New header line. Flush the previous one first.
		flush_header(current_header, &current_value, &mut subject, &mut from, &mut date);
		current_header = None;
		current_value.clear();

		if let Some(colon_pos) = line.find(':') {
			let name = &line[..colon_pos];
			let value = line[colon_pos + 1..].trim();

			let name_lower = name.to_ascii_lowercase();
			match name_lower.as_str() {
				"subject" | "from" | "date" => {
					current_header = Some(match name_lower.as_str() {
						"subject" => "subject",
						"from" => "from",
						"date" => "date",
						_ => unreachable!(),
					});
					current_value = value.to_string();
				},
				_ => {},
			}
		}

		i += 1;
	}

	// If no blank line was found, flush whatever we have.
	if body_start.is_none() {
		flush_header(current_header, &current_value, &mut subject, &mut from, &mut date);
	}

	let body = body_start
		.map(|start| {
			let body_text: String = lines[start..].join("\n");
			let trimmed = body_text.trim();
			if trimmed.is_empty() {
				return None;
			}
			Some(trimmed.to_string())
		})
		.flatten();

	ParsedEmail { subject, from, date, body }
}

/// Helper to flush a parsed header value into the appropriate field.
fn flush_header(
	header: Option<&str>, value: &str, subject: &mut Option<String>, from: &mut Option<String>,
	date: &mut Option<String>,
) {
	if value.is_empty() {
		return;
	}
	match header {
		Some("subject") => *subject = Some(value.to_string()),
		Some("from") => *from = Some(value.to_string()),
		Some("date") => *date = Some(value.to_string()),
		_ => {},
	}
}

/// Try to parse a Date header value into a `DateTime<Utc>`.
///
/// Email dates come in various RFC 2822 formats. We try chrono's RFC 2822
/// parser first, then fall back to a few common patterns.
fn parse_email_date(date_str: &str) -> Option<DateTime<Utc>> {
	// Try RFC 2822 first (the standard for email).
	if let Ok(dt) = DateTime::parse_from_rfc2822(date_str) {
		return Some(dt.with_timezone(&Utc));
	}

	// Try RFC 3339 as a fallback.
	if let Ok(dt) = DateTime::parse_from_rfc3339(date_str) {
		return Some(dt.with_timezone(&Utc));
	}

	None
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_feed_url_no_cursor() {
		let url = MailingListSyncSource::feed_url(None);
		assert_eq!(url, "https://gnusha.org/pi/bitcoindev/?q=d:..&x=A");
	}

	#[test]
	fn test_feed_url_with_cursor() {
		let url = MailingListSyncSource::feed_url(Some("2024-06-15"));
		assert_eq!(url, "https://gnusha.org/pi/bitcoindev/?q=d:2024-06-15..&x=A");
	}

	#[test]
	fn test_raw_message_url() {
		let url = MailingListSyncSource::raw_message_url(
			"https://gnusha.org/pi/bitcoindev/CABaSBaz7E+ZGU2GBGm=a9V=BGXRWPCH4gH-8j03gJN=OyTQ_+g@mail.gmail.com/",
		);
		assert_eq!(
			url,
			"https://gnusha.org/pi/bitcoindev/CABaSBaz7E+ZGU2GBGm=a9V=BGXRWPCH4gH-8j03gJN=OyTQ_+g@mail.gmail.com/raw"
		);
	}

	#[test]
	fn test_raw_message_url_no_trailing_slash() {
		let url = MailingListSyncSource::raw_message_url(
			"https://gnusha.org/pi/bitcoindev/some-id@example.com",
		);
		assert_eq!(url, "https://gnusha.org/pi/bitcoindev/some-id@example.com/raw");
	}

	#[test]
	fn test_extract_message_id() {
		let id = extract_message_id("https://gnusha.org/pi/bitcoindev/CABaSBaz7E@mail.gmail.com/");
		assert_eq!(id, "CABaSBaz7E@mail.gmail.com");
	}

	#[test]
	fn test_extract_message_id_no_trailing_slash() {
		let id = extract_message_id("https://gnusha.org/pi/bitcoindev/some-id@example.com");
		assert_eq!(id, "some-id@example.com");
	}

	#[test]
	fn test_parse_email_headers_basic() {
		let raw = "From: Alice <alice@example.com>\r\n\
			Subject: Test Subject\r\n\
			Date: Mon, 1 Jan 2024 12:00:00 +0000\r\n\
			Message-ID: <test-id@example.com>\r\n\
			\r\n\
			This is the body.\r\n\
			\r\n\
			Second paragraph.";

		let parsed = parse_email_headers(raw);
		assert_eq!(parsed.from.as_deref(), Some("Alice <alice@example.com>"));
		assert_eq!(parsed.subject.as_deref(), Some("Test Subject"));
		assert_eq!(parsed.date.as_deref(), Some("Mon, 1 Jan 2024 12:00:00 +0000"));
		assert_eq!(parsed.body.as_deref(), Some("This is the body.\n\nSecond paragraph."));
	}

	#[test]
	fn test_parse_email_headers_folded() {
		let raw = "From: Bob <bob@example.com>\n\
			Subject: A very long subject that\n \
			continues on the next line\n\
			Date: Tue, 2 Jan 2024 14:30:00 +0000\n\
			\n\
			Body text here.";

		let parsed = parse_email_headers(raw);
		assert_eq!(
			parsed.subject.as_deref(),
			Some("A very long subject that continues on the next line")
		);
		assert_eq!(parsed.from.as_deref(), Some("Bob <bob@example.com>"));
		assert_eq!(parsed.body.as_deref(), Some("Body text here."));
	}

	#[test]
	fn test_parse_email_headers_empty_body() {
		let raw = "\
Subject: No body\n\
From: test@example.com\n\
Date: Wed, 3 Jan 2024 00:00:00 +0000\n\
\n\
";

		let parsed = parse_email_headers(raw);
		assert_eq!(parsed.subject.as_deref(), Some("No body"));
		assert!(parsed.body.is_none());
	}

	#[test]
	fn test_parse_email_headers_no_blank_line() {
		let raw = "\
Subject: Headers only\n\
From: test@example.com";

		let parsed = parse_email_headers(raw);
		assert_eq!(parsed.subject.as_deref(), Some("Headers only"));
		assert_eq!(parsed.from.as_deref(), Some("test@example.com"));
		assert!(parsed.body.is_none());
	}

	#[test]
	fn test_parse_email_headers_case_insensitive() {
		let raw = "\
SUBJECT: Upper Case\n\
FROM: test@example.com\n\
DATE: Thu, 4 Jan 2024 10:00:00 +0000\n\
\n\
Body.";

		let parsed = parse_email_headers(raw);
		assert_eq!(parsed.subject.as_deref(), Some("Upper Case"));
		assert_eq!(parsed.from.as_deref(), Some("test@example.com"));
		assert_eq!(parsed.date.as_deref(), Some("Thu, 4 Jan 2024 10:00:00 +0000"));
	}

	#[test]
	fn test_parse_email_date_rfc2822() {
		let dt = parse_email_date("Mon, 1 Jan 2024 12:00:00 +0000");
		assert!(dt.is_some());
		let dt = dt.unwrap();
		assert_eq!(dt.date_naive(), NaiveDate::from_ymd_opt(2024, 1, 1).unwrap());
	}

	#[test]
	fn test_parse_email_date_rfc3339() {
		let dt = parse_email_date("2024-01-15T10:30:00+00:00");
		assert!(dt.is_some());
	}

	#[test]
	fn test_parse_email_date_invalid() {
		assert!(parse_email_date("not a date").is_none());
	}

	#[test]
	fn test_parse_atom_entries() {
		let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>bitcoindev</title>
  <entry>
    <title>Test message 1</title>
    <link href="https://gnusha.org/pi/bitcoindev/msg1@example.com/"/>
    <updated>2024-01-15T10:00:00Z</updated>
  </entry>
  <entry>
    <title>Test message 2</title>
    <link href="https://gnusha.org/pi/bitcoindev/msg2@example.com/"/>
    <updated>2024-01-16T12:00:00Z</updated>
  </entry>
</feed>"#;

		let entries = parse_atom_entries(xml).unwrap();
		assert_eq!(entries.len(), 2);
		assert_eq!(entries[0].link, "https://gnusha.org/pi/bitcoindev/msg1@example.com/");
		assert_eq!(entries[0].updated, "2024-01-15T10:00:00Z");
		assert_eq!(entries[1].link, "https://gnusha.org/pi/bitcoindev/msg2@example.com/");
		assert_eq!(entries[1].updated, "2024-01-16T12:00:00Z");
	}

	#[test]
	fn test_parse_atom_entries_empty_feed() {
		let xml = r#"<?xml version="1.0"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>bitcoindev</title>
</feed>"#;

		let entries = parse_atom_entries(xml).unwrap();
		assert!(entries.is_empty());
	}

	#[test]
	fn test_extract_atom_link() {
		let entry = r#"<entry>
    <link href="https://gnusha.org/pi/bitcoindev/test@mail.com/"/>
    <updated>2024-01-01T00:00:00Z</updated>
  </entry>"#;
		let link = extract_atom_link(entry);
		assert_eq!(link.as_deref(), Some("https://gnusha.org/pi/bitcoindev/test@mail.com/"));
	}

	#[test]
	fn test_extract_atom_element() {
		let xml = "<entry><updated>2024-01-15T10:00:00Z</updated></entry>";
		let updated = extract_atom_element(xml, "updated");
		assert_eq!(updated.as_deref(), Some("2024-01-15T10:00:00Z"));
	}

	#[test]
	fn test_extract_atom_element_missing() {
		let xml = "<entry><title>Test</title></entry>";
		let result = extract_atom_element(xml, "updated");
		assert!(result.is_none());
	}

	#[test]
	fn test_name() {
		let source = MailingListSyncSource::new();
		assert_eq!(source.name(), "mailing_list:bitcoindev");
	}

	#[test]
	fn test_poll_interval() {
		let source = MailingListSyncSource::new();
		assert_eq!(source.poll_interval(), Duration::from_secs(1800));
	}
}
