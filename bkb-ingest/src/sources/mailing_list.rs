use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Datelike, Utc};
use reqwest::Client;
use tracing::{debug, info, warn};

use bkb_core::model::{Document, SourceType};

use super::{SyncPage, SyncSource};
use crate::rate_limiter::RateLimiter;

/// The first month to start indexing from. The bitcoin-dev mailing list
/// archive on gnusha.org goes back to around 2011.
const ARCHIVE_START: &str = "2011-06";

/// Sync source for a public-inbox mailing list archive at gnusha.org.
///
/// Uses monthly date windows with offset-based pagination (`o=N`) to walk
/// the entire archive chronologically, since the Atom feed returns results
/// newest-first regardless of the date range filter.
///
/// Cursor format: `YYYY-MM:OFFSET` (e.g., `2024-06:50`). An offset of 0
/// is written as just `YYYY-MM`. On initial sync, starts from
/// [`ARCHIVE_START`].
pub struct MailingListSyncSource {
	base_url: String,
	list_name: String,
	client: Client,
}

impl MailingListSyncSource {
	pub fn new() -> Self {
		Self::with_list("bitcoindev")
	}

	pub fn with_list(list: &str) -> Self {
		Self {
			base_url: format!("https://gnusha.org/pi/{}", list),
			list_name: list.to_string(),
			client: Client::new(),
		}
	}

	/// Build the Atom feed URL for a month window with offset.
	///
	/// Queries `d:YYYY-MM-01..YYYY-MM+1-01` to bound results to a single
	/// month, and uses `o=OFFSET` for pagination within that month.
	fn feed_url(&self, month: &str, offset: usize) -> String {
		let (start, end) = month_date_range(month);
		if offset > 0 {
			format!("{}/?q=d:{}..{}&x=A&o={}", self.base_url, start, end, offset)
		} else {
			format!("{}/?q=d:{}..{}&x=A", self.base_url, start, end)
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

	/// Fetch and parse the Atom feed, returning a list of entries.
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
		// Parse cursor: "YYYY-MM:OFFSET" or "YYYY-MM" (offset 0)
		let (month, offset) = parse_cursor(cursor);

		let feed_url = self.feed_url(&month, offset);
		let entries = self.fetch_feed(&feed_url, rate_limiter).await?;

		if entries.is_empty() {
			// No entries in this month window. If this month is in the past,
			// advance to the next month. If it's the current or future month,
			// we're caught up.
			let today = Utc::now().date_naive();
			let current_month = format!("{:04}-{:02}", today.year(), today.month());
			if month < current_month {
				let next = next_month(&month);
				info!(
					source = %self.list_name,
					month = %month,
					next_month = %next,
					"empty month, advancing"
				);
				return Ok(SyncPage {
					documents: vec![],
					references: vec![],
					next_cursor: Some(next),
				});
			}

			debug!(source = %self.list_name, "caught up with current month");
			return Ok(SyncPage { documents: vec![], references: vec![], next_cursor: None });
		}

		let entry_count = entries.len();
		let mut documents = Vec::new();

		for entry in &entries {
			let message_id = extract_message_id(&entry.link, &self.list_name);
			if message_id.is_empty() {
				warn!(link = %entry.link, "could not extract message ID from link, skipping");
				continue;
			}

			let raw_url = Self::raw_message_url(&entry.link);
			let raw_text = match self.fetch_raw_message(&raw_url, rate_limiter).await {
				Ok(text) => text,
				Err(e) => {
					warn!(url = %raw_url, error = %e, "failed to fetch raw message, skipping");
					continue;
				},
			};

			let parsed = parse_email_headers(&raw_text);

			let created_at = parsed
				.date
				.as_deref()
				.and_then(parse_email_date)
				.or_else(|| {
					DateTime::parse_from_rfc3339(&entry.updated)
						.ok()
						.map(|dt| dt.with_timezone(&Utc))
				})
				.unwrap_or_else(Utc::now);

			let source_id = message_id.to_string();
			let id = Document::make_id(&SourceType::MailingListMsg, None, &source_id);

			documents.push(Document {
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
			});
		}

		// Determine the next cursor: bump offset within same month, or
		// advance to the next month if we got a small (likely final) page.
		// Public-inbox Atom feeds return ~15-30 entries per page.
		let next_cursor = if entry_count >= 10 {
			// Likely more entries in this month -- bump offset.
			let new_offset = offset + entry_count;
			Some(format!("{}:{}", month, new_offset))
		} else {
			// Small page -- probably the last page for this month.
			// Advance to the next month.
			let today = Utc::now().date_naive();
			let current_month = format!("{:04}-{:02}", today.year(), today.month());
			if month < current_month {
				Some(next_month(&month))
			} else {
				// Current month -- we'll re-check on next poll cycle.
				Some(month.clone())
			}
		};

		info!(
			source = %self.list_name,
			month = %month,
			offset = offset,
			entries = entry_count,
			documents = documents.len(),
			next_cursor = ?next_cursor,
			"fetched mailing list messages"
		);

		Ok(SyncPage { documents, references: vec![], next_cursor })
	}

	fn poll_interval(&self) -> Duration {
		Duration::from_secs(1800) // 30 minutes
	}

	fn name(&self) -> &str {
		Box::leak(format!("mailing_list:{}", self.list_name).into_boxed_str())
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
fn extract_message_id(link: &str, list_name: &str) -> String {
	let trimmed = link.trim_end_matches('/');
	let needle = format!("/pi/{}/", list_name);
	if let Some(base_pos) = trimmed.find(&needle) {
		let after_base = &trimmed[base_pos + needle.len()..];
		if !after_base.is_empty() {
			return after_base.to_string();
		}
	}
	// Fallback: just use the last path segment.
	trimmed.rsplit('/').next().unwrap_or("").to_string()
}

/// Parse cursor string into (month, offset).
///
/// Cursor format: `"YYYY-MM:OFFSET"` or `"YYYY-MM"` (offset 0).
/// Returns `(ARCHIVE_START, 0)` for `None`.
fn parse_cursor(cursor: Option<&str>) -> (String, usize) {
	match cursor {
		Some(c) => {
			if let Some((month, off_str)) = c.split_once(':') {
				let offset = off_str.parse::<usize>().unwrap_or(0);
				(month.to_string(), offset)
			} else {
				(c.to_string(), 0)
			}
		},
		None => (ARCHIVE_START.to_string(), 0),
	}
}

/// Compute the start and end dates for a `YYYY-MM` month string.
///
/// Returns `("YYYY-MM-01", "YYYY-MM+1-01")`.
fn month_date_range(month: &str) -> (String, String) {
	let parts: Vec<&str> = month.split('-').collect();
	let year: i32 = parts.first().and_then(|s| s.parse().ok()).unwrap_or(2011);
	let mon: u32 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(1);

	let start = format!("{:04}-{:02}-01", year, mon);
	let (ny, nm) = if mon >= 12 { (year + 1, 1) } else { (year, mon + 1) };
	let end = format!("{:04}-{:02}-01", ny, nm);
	(start, end)
}

/// Advance to the next month: `"2024-12"` -> `"2025-01"`.
fn next_month(month: &str) -> String {
	let parts: Vec<&str> = month.split('-').collect();
	let year: i32 = parts.first().and_then(|s| s.parse().ok()).unwrap_or(2011);
	let mon: u32 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(1);
	let (ny, nm) = if mon >= 12 { (year + 1, 1) } else { (year, mon + 1) };
	format!("{:04}-{:02}", ny, nm)
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
	use chrono::NaiveDate;

	use super::*;

	#[test]
	fn test_feed_url_no_offset() {
		let source = MailingListSyncSource::new();
		let url = source.feed_url("2024-06", 0);
		assert_eq!(url, "https://gnusha.org/pi/bitcoindev/?q=d:2024-06-01..2024-07-01&x=A");
	}

	#[test]
	fn test_feed_url_with_offset() {
		let source = MailingListSyncSource::new();
		let url = source.feed_url("2024-12", 50);
		assert_eq!(url, "https://gnusha.org/pi/bitcoindev/?q=d:2024-12-01..2025-01-01&x=A&o=50");
	}

	#[test]
	fn test_parse_cursor_none() {
		let (month, offset) = parse_cursor(None);
		assert_eq!(month, ARCHIVE_START);
		assert_eq!(offset, 0);
	}

	#[test]
	fn test_parse_cursor_month_only() {
		let (month, offset) = parse_cursor(Some("2024-06"));
		assert_eq!(month, "2024-06");
		assert_eq!(offset, 0);
	}

	#[test]
	fn test_parse_cursor_with_offset() {
		let (month, offset) = parse_cursor(Some("2024-06:75"));
		assert_eq!(month, "2024-06");
		assert_eq!(offset, 75);
	}

	#[test]
	fn test_month_date_range() {
		assert_eq!(
			month_date_range("2024-06"),
			("2024-06-01".to_string(), "2024-07-01".to_string())
		);
		assert_eq!(
			month_date_range("2024-12"),
			("2024-12-01".to_string(), "2025-01-01".to_string())
		);
	}

	#[test]
	fn test_next_month() {
		assert_eq!(next_month("2024-06"), "2024-07");
		assert_eq!(next_month("2024-12"), "2025-01");
		assert_eq!(next_month("2023-01"), "2023-02");
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
		let id = extract_message_id(
			"https://gnusha.org/pi/bitcoindev/CABaSBaz7E@mail.gmail.com/",
			"bitcoindev",
		);
		assert_eq!(id, "CABaSBaz7E@mail.gmail.com");
	}

	#[test]
	fn test_extract_message_id_no_trailing_slash() {
		let id = extract_message_id(
			"https://gnusha.org/pi/bitcoindev/some-id@example.com",
			"bitcoindev",
		);
		assert_eq!(id, "some-id@example.com");
	}

	#[test]
	fn test_extract_message_id_lightning_dev() {
		let id = extract_message_id(
			"https://gnusha.org/pi/lightning-dev/some-id@example.com/",
			"lightning-dev",
		);
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
		let source2 = MailingListSyncSource::with_list("lightning-dev");
		assert_eq!(source2.name(), "mailing_list:lightning-dev");
	}

	#[test]
	fn test_poll_interval() {
		let source = MailingListSyncSource::new();
		assert_eq!(source.poll_interval(), Duration::from_secs(1800));
	}
}
