use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;
use tracing::{debug, info};

use bkb_core::model::{Document, RefType, Reference, SourceType};

use super::{SyncPage, SyncSource};
use crate::rate_limiter::RateLimiter;

const GITHUB_API_BASE: &str = "https://api.github.com";
const PER_PAGE: u32 = 100;

/// Sync source for GitHub issues and pull requests.
///
/// Uses `GET /repos/{owner}/{repo}/issues?state=all&sort=updated&since=...`
/// which returns both issues and PRs. The cursor is the `since` timestamp.
pub struct GitHubIssueSyncSource {
	client: Client,
	owner: String,
	repo: String,
	token: Option<String>,
}

impl GitHubIssueSyncSource {
	pub fn new(owner: &str, repo: &str, token: Option<String>) -> Self {
		Self { client: Client::new(), owner: owner.to_string(), repo: repo.to_string(), token }
	}

	fn build_request(&self, url: &str) -> reqwest::RequestBuilder {
		let mut req = self
			.client
			.get(url)
			.header("User-Agent", "bkb/0.1")
			.header("Accept", "application/vnd.github+json");

		if let Some(ref token) = self.token {
			req = req.header("Authorization", format!("Bearer {}", token));
		}

		req
	}
}

#[async_trait]
impl SyncSource for GitHubIssueSyncSource {
	async fn fetch_page(
		&self, cursor: Option<&str>, rate_limiter: &RateLimiter,
	) -> Result<SyncPage> {
		rate_limiter.acquire().await;

		let mut url = format!(
			"{}/repos/{}/{}/issues?state=all&sort=updated&direction=asc&per_page={}",
			GITHUB_API_BASE, self.owner, self.repo, PER_PAGE,
		);

		if let Some(since) = cursor {
			url.push_str(&format!("&since={}", since));
		}

		debug!(url = %url, "fetching GitHub issues page");

		let response =
			self.build_request(&url).send().await.context("failed to fetch GitHub issues")?;

		rate_limiter.update_from_response(response.headers());

		let status = response.status();
		if !status.is_success() {
			let body = response.text().await.unwrap_or_default();
			anyhow::bail!("GitHub API returned {}: {}", status, body);
		}

		let issues: Vec<GitHubIssue> =
			response.json().await.context("failed to parse GitHub issues response")?;

		info!(
			source = %self.name(),
			count = issues.len(),
			"fetched issues page"
		);

		let next_cursor = if issues.len() == PER_PAGE as usize {
			// More pages available; use the last issue's updated_at as cursor
			issues.last().map(|i| i.updated_at.to_rfc3339())
		} else {
			None
		};

		let mut documents = Vec::with_capacity(issues.len());
		let mut references = Vec::new();

		for issue in issues {
			let is_pr = issue.pull_request.is_some();
			let source_type = if is_pr { SourceType::GithubPr } else { SourceType::GithubIssue };
			let source_repo = format!("{}/{}", self.owner, self.repo);
			let source_id = issue.number.to_string();
			let id = Document::make_id(&source_type, Some(&source_repo), &source_id);

			let metadata = serde_json::json!({
				"state": issue.state,
				"labels": issue.labels.iter().map(|l| &l.name).collect::<Vec<_>>(),
				"is_pr": is_pr,
			});

			documents.push(Document {
				id: id.clone(),
				source_type,
				source_repo: Some(source_repo),
				source_id,
				title: Some(issue.title),
				body: issue.body,
				author: Some(issue.user.login.clone()),
				author_id: Some(issue.user.id.to_string()),
				created_at: issue.created_at,
				updated_at: Some(issue.updated_at),
				parent_id: None,
				metadata: Some(metadata),
				seq: None,
			});

			// Extract parent reference for PRs that reference issues
			if let Some(ref body) = documents.last().unwrap().body {
				let doc_id = id.clone();
				let repo_str = format!("{}/{}", self.owner, self.repo);
				references.extend(extract_issue_refs(body, &doc_id, &repo_str));
			}
		}

		Ok(SyncPage { documents, references, next_cursor })
	}

	fn poll_interval(&self) -> Duration {
		Duration::from_secs(3600) // 1 hour
	}

	fn name(&self) -> &str {
		// This leaks a string, which is fine for a long-lived source name
		Box::leak(format!("github:{}/{}:issues", self.owner, self.repo).into_boxed_str())
	}
}

/// Sync source for GitHub issue/PR comments.
///
/// Uses `GET /repos/{owner}/{repo}/issues/comments?sort=updated&since=...`
pub struct GitHubCommentSyncSource {
	client: Client,
	owner: String,
	repo: String,
	token: Option<String>,
}

impl GitHubCommentSyncSource {
	pub fn new(owner: &str, repo: &str, token: Option<String>) -> Self {
		Self { client: Client::new(), owner: owner.to_string(), repo: repo.to_string(), token }
	}

	fn build_request(&self, url: &str) -> reqwest::RequestBuilder {
		let mut req = self
			.client
			.get(url)
			.header("User-Agent", "bkb/0.1")
			.header("Accept", "application/vnd.github+json");

		if let Some(ref token) = self.token {
			req = req.header("Authorization", format!("Bearer {}", token));
		}

		req
	}
}

#[async_trait]
impl SyncSource for GitHubCommentSyncSource {
	async fn fetch_page(
		&self, cursor: Option<&str>, rate_limiter: &RateLimiter,
	) -> Result<SyncPage> {
		rate_limiter.acquire().await;

		let mut url = format!(
			"{}/repos/{}/{}/issues/comments?sort=updated&direction=asc&per_page={}",
			GITHUB_API_BASE, self.owner, self.repo, PER_PAGE,
		);

		if let Some(since) = cursor {
			url.push_str(&format!("&since={}", since));
		}

		debug!(url = %url, "fetching GitHub comments page");

		let response =
			self.build_request(&url).send().await.context("failed to fetch GitHub comments")?;

		rate_limiter.update_from_response(response.headers());

		let status = response.status();
		if !status.is_success() {
			let body = response.text().await.unwrap_or_default();
			anyhow::bail!("GitHub API returned {}: {}", status, body);
		}

		let comments: Vec<GitHubComment> =
			response.json().await.context("failed to parse GitHub comments response")?;

		info!(
			source = %self.name(),
			count = comments.len(),
			"fetched comments page"
		);

		let next_cursor = if comments.len() == PER_PAGE as usize {
			comments.last().map(|c| c.updated_at.to_rfc3339())
		} else {
			None
		};

		let mut documents = Vec::with_capacity(comments.len());
		let mut references = Vec::new();

		for comment in comments {
			let source_repo = format!("{}/{}", self.owner, self.repo);
			let source_id = comment.id.to_string();
			let id = Document::make_id(&SourceType::GithubComment, Some(&source_repo), &source_id);

			// Extract the parent issue/PR number from the issue_url
			let parent_id = comment.issue_url.rsplit('/').next().and_then(|num_str| {
				num_str.parse::<u64>().ok().map(|num| {
					// We don't know if it's an issue or PR from the comment API;
					// use issue as default (will be resolved later).
					Document::make_id(
						&SourceType::GithubIssue,
						Some(&source_repo),
						&num.to_string(),
					)
				})
			});

			documents.push(Document {
				id: id.clone(),
				source_type: SourceType::GithubComment,
				source_repo: Some(source_repo.clone()),
				source_id,
				title: None,
				body: Some(comment.body.clone()),
				author: Some(comment.user.login.clone()),
				author_id: Some(comment.user.id.to_string()),
				created_at: comment.created_at,
				updated_at: Some(comment.updated_at),
				parent_id,
				metadata: None,
				seq: None,
			});

			references.extend(extract_issue_refs(&comment.body, &id, &source_repo));
		}

		Ok(SyncPage { documents, references, next_cursor })
	}

	fn poll_interval(&self) -> Duration {
		Duration::from_secs(3600)
	}

	fn name(&self) -> &str {
		Box::leak(format!("github:{}/{}:comments", self.owner, self.repo).into_boxed_str())
	}
}

/// Extract cross-references from text (issue mentions, BIP/BOLT references, etc.).
pub fn extract_issue_refs(text: &str, from_doc_id: &str, source_repo: &str) -> Vec<Reference> {
	use regex::Regex;

	// We use thread-local cached regexes for performance
	thread_local! {
		// #1234 - same-repo issue/PR reference
		static RE_ISSUE: Regex = Regex::new(r"(?:^|[^&\w])#(\d+)").unwrap();
		// owner/repo#1234 - cross-repo reference
		static RE_CROSS_REPO: Regex = Regex::new(r"(\w[\w.-]*/\w[\w.-]*)#(\d+)").unwrap();
		// BIP-340, BIP 340, bip340
		static RE_BIP: Regex = Regex::new(r"(?i)\bBIP[- ]?(\d{1,4})\b").unwrap();
		// BOLT-11, BOLT 11, bolt11
		static RE_BOLT: Regex = Regex::new(r"(?i)\bBOLT[- ]?(\d{1,2})\b").unwrap();
		// bLIP-1, bLIP 1, blip-1, blip1
		static RE_BLIP: Regex = Regex::new(r"(?i)\bbLIP[- ]?(\d{1,4})\b").unwrap();
		// LUD-06, LUD 06, lud-6, lud6
		static RE_LUD: Regex = Regex::new(r"(?i)\bLUD[- ]?(\d{1,2})\b").unwrap();
		// NUT-00, NUT 00, nut-0, nut0
		static RE_NUT: Regex = Regex::new(r"(?i)\bNUT[- ]?(\d{1,2})\b").unwrap();
		// Fixes #1234, Closes #1234
		static RE_FIXES: Regex = Regex::new(r"(?i)(?:fix(?:es|ed)?|clos(?:es|ed)?|resolv(?:es|ed)?)\s+#(\d+)").unwrap();
		// Commit SHA references: 7-40 hex chars on a word boundary, not preceded
		// by common false-positive contexts (color codes, variable names).
		// We require at least one digit AND at least one letter [a-f] to avoid
		// matching pure numbers or words.
		static RE_COMMIT_SHA: Regex = Regex::new(r"(?:^|[\s(,])([0-9a-f]{7,40})\b").unwrap();
	}

	let mut refs = Vec::new();

	// Fixes/Closes references (check first so they take priority)
	RE_FIXES.with(|re| {
		for cap in re.captures_iter(text) {
			let num = &cap[1];
			refs.push(Reference {
				id: None,
				from_doc_id: from_doc_id.to_string(),
				to_doc_id: None,
				ref_type: RefType::Fixes,
				to_external: Some(format!("{}#{}", source_repo, num)),
				context: Some(cap[0].to_string()),
			});
		}
	});

	// Same-repo issue references
	RE_ISSUE.with(|re| {
		for cap in re.captures_iter(text) {
			let num = &cap[1];
			refs.push(Reference {
				id: None,
				from_doc_id: from_doc_id.to_string(),
				to_doc_id: None,
				ref_type: RefType::MentionsIssue,
				to_external: Some(format!("{}#{}", source_repo, num)),
				context: Some(cap[0].trim().to_string()),
			});
		}
	});

	// Cross-repo references
	RE_CROSS_REPO.with(|re| {
		for cap in re.captures_iter(text) {
			let repo = &cap[1];
			let num = &cap[2];
			if repo != source_repo {
				refs.push(Reference {
					id: None,
					from_doc_id: from_doc_id.to_string(),
					to_doc_id: None,
					ref_type: RefType::MentionsIssue,
					to_external: Some(format!("{}#{}", repo, num)),
					context: Some(cap[0].to_string()),
				});
			}
		}
	});

	// BIP references
	RE_BIP.with(|re| {
		for cap in re.captures_iter(text) {
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

	// BOLT references
	RE_BOLT.with(|re| {
		for cap in re.captures_iter(text) {
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

	// bLIP references
	RE_BLIP.with(|re| {
		for cap in re.captures_iter(text) {
			refs.push(Reference {
				id: None,
				from_doc_id: from_doc_id.to_string(),
				to_doc_id: None,
				ref_type: RefType::ReferencesBlip,
				to_external: Some(format!("bLIP-{}", &cap[1])),
				context: Some(cap[0].to_string()),
			});
		}
	});

	// LUD references
	RE_LUD.with(|re| {
		for cap in re.captures_iter(text) {
			refs.push(Reference {
				id: None,
				from_doc_id: from_doc_id.to_string(),
				to_doc_id: None,
				ref_type: RefType::ReferencesLud,
				to_external: Some(format!("LUD-{}", &cap[1])),
				context: Some(cap[0].to_string()),
			});
		}
	});

	// NUT references
	RE_NUT.with(|re| {
		for cap in re.captures_iter(text) {
			refs.push(Reference {
				id: None,
				from_doc_id: from_doc_id.to_string(),
				to_doc_id: None,
				ref_type: RefType::ReferencesNut,
				to_external: Some(format!("NUT-{}", &cap[1])),
				context: Some(cap[0].to_string()),
			});
		}
	});

	// Commit SHA references
	RE_COMMIT_SHA.with(|re| {
		for cap in re.captures_iter(text) {
			let sha = &cap[1];
			// Require mixed hex: at least one digit and at least one a-f letter
			// to avoid matching pure decimal numbers or dictionary words
			let has_digit = sha.bytes().any(|b| b.is_ascii_digit());
			let has_alpha = sha.bytes().any(|b| matches!(b, b'a'..=b'f'));
			if has_digit && has_alpha {
				refs.push(Reference {
					id: None,
					from_doc_id: from_doc_id.to_string(),
					to_doc_id: None,
					ref_type: RefType::ReferencesCommit,
					to_external: Some(sha.to_string()),
					context: Some(cap[0].trim().to_string()),
				});
			}
		}
	});

	refs
}

// --- GitHub API response types ---

#[derive(Debug, Deserialize)]
struct GitHubIssue {
	number: u64,
	title: String,
	body: Option<String>,
	state: String,
	user: GitHubUser,
	labels: Vec<GitHubLabel>,
	created_at: DateTime<Utc>,
	updated_at: DateTime<Utc>,
	pull_request: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct GitHubComment {
	id: u64,
	body: String,
	user: GitHubUser,
	created_at: DateTime<Utc>,
	updated_at: DateTime<Utc>,
	issue_url: String,
}

#[derive(Debug, Deserialize)]
struct GitHubUser {
	login: String,
	id: u64,
}

#[derive(Debug, Deserialize)]
struct GitHubLabel {
	name: String,
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_extract_same_repo_issue_ref() {
		let refs = extract_issue_refs("See #1234 for details", "doc:1", "bitcoin/bitcoin");
		assert!(refs.iter().any(|r| r.to_external.as_deref() == Some("bitcoin/bitcoin#1234")));
	}

	#[test]
	fn test_extract_cross_repo_ref() {
		let refs = extract_issue_refs("Related to lightning/bolts#123", "doc:1", "bitcoin/bitcoin");
		assert!(refs.iter().any(|r| r.to_external.as_deref() == Some("lightning/bolts#123")));
	}

	#[test]
	fn test_extract_bip_ref() {
		let refs =
			extract_issue_refs("This implements BIP-340 and BIP 341", "doc:1", "bitcoin/bitcoin");
		let bip_refs: Vec<_> =
			refs.iter().filter(|r| r.ref_type == RefType::ReferencesBip).collect();
		assert_eq!(bip_refs.len(), 2);
	}

	#[test]
	fn test_extract_bolt_ref() {
		let refs = extract_issue_refs(
			"See BOLT-11 for invoice format",
			"doc:1",
			"lightningdevkit/rust-lightning",
		);
		assert!(refs.iter().any(|r| r.ref_type == RefType::ReferencesBolt
			&& r.to_external.as_deref() == Some("BOLT-11")));
	}

	#[test]
	fn test_extract_fixes_ref() {
		let refs = extract_issue_refs("Fixes #5678", "doc:1", "bitcoin/bitcoin");
		assert!(refs.iter().any(|r| r.ref_type == RefType::Fixes));
	}

	#[test]
	fn test_extract_blip_ref() {
		let refs = extract_issue_refs(
			"See bLIP-1 for keysend and blip 2 for hosted channels",
			"doc:1",
			"lightningdevkit/rust-lightning",
		);
		let blip_refs: Vec<_> =
			refs.iter().filter(|r| r.ref_type == RefType::ReferencesBlip).collect();
		assert_eq!(blip_refs.len(), 2);
		assert!(blip_refs.iter().any(|r| r.to_external.as_deref() == Some("bLIP-1")));
		assert!(blip_refs.iter().any(|r| r.to_external.as_deref() == Some("bLIP-2")));
	}

	#[test]
	fn test_no_false_positive_html_entities() {
		// &#1234; should not match as #1234
		let refs = extract_issue_refs("Use &#1234; entity", "doc:1", "bitcoin/bitcoin");
		let issue_refs: Vec<_> =
			refs.iter().filter(|r| r.ref_type == RefType::MentionsIssue).collect();
		assert!(issue_refs.is_empty());
	}

	#[test]
	fn test_extract_commit_sha_full() {
		let refs = extract_issue_refs(
			"Cherry-picked from abc123def456789012345678901234567890abcd",
			"doc:1",
			"bitcoin/bitcoin",
		);
		let sha_refs: Vec<_> =
			refs.iter().filter(|r| r.ref_type == RefType::ReferencesCommit).collect();
		assert_eq!(sha_refs.len(), 1);
		assert_eq!(
			sha_refs[0].to_external.as_deref(),
			Some("abc123def456789012345678901234567890abcd")
		);
	}

	#[test]
	fn test_extract_commit_sha_short() {
		let refs = extract_issue_refs(
			"Reverts abc123f in the previous release",
			"doc:1",
			"bitcoin/bitcoin",
		);
		let sha_refs: Vec<_> =
			refs.iter().filter(|r| r.ref_type == RefType::ReferencesCommit).collect();
		assert_eq!(sha_refs.len(), 1);
		assert_eq!(sha_refs[0].to_external.as_deref(), Some("abc123f"));
	}

	#[test]
	fn test_no_false_positive_pure_numbers() {
		// Pure decimal digits should not match as a commit SHA
		let refs = extract_issue_refs("Error code 1234567", "doc:1", "bitcoin/bitcoin");
		let sha_refs: Vec<_> =
			refs.iter().filter(|r| r.ref_type == RefType::ReferencesCommit).collect();
		assert!(sha_refs.is_empty(), "pure numbers should not match as SHA");
	}

	#[test]
	fn test_no_false_positive_short_hex() {
		// 6-char hex is too short
		let refs = extract_issue_refs("value abc123 is used", "doc:1", "bitcoin/bitcoin");
		let sha_refs: Vec<_> =
			refs.iter().filter(|r| r.ref_type == RefType::ReferencesCommit).collect();
		assert!(sha_refs.is_empty(), "6-char hex should not match as SHA");
	}

	#[test]
	fn test_extract_multiple_commit_shas() {
		let refs = extract_issue_refs(
			"Compare abc123f and def456a for the regression",
			"doc:1",
			"bitcoin/bitcoin",
		);
		let sha_refs: Vec<_> =
			refs.iter().filter(|r| r.ref_type == RefType::ReferencesCommit).collect();
		assert_eq!(sha_refs.len(), 2, "should find two commit SHAs");
	}
}
