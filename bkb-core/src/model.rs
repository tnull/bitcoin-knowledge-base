use std::fmt;

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

/// Parse a date/datetime string flexibly, accepting both full RFC 3339
/// timestamps (e.g., `2023-01-01T00:00:00Z`) and plain ISO 8601 dates
/// (e.g., `2023-01-01`).
pub fn parse_datetime(s: &str) -> Option<DateTime<Utc>> {
	// Try full RFC 3339 first.
	if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
		return Some(dt.with_timezone(&Utc));
	}

	// Fall back to plain YYYY-MM-DD date, interpreting as midnight UTC.
	if let Ok(date) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
		return date.and_hms_opt(0, 0, 0).map(|naive| naive.and_utc());
	}

	None
}

/// The type of source a document originated from.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceType {
	GithubIssue,
	GithubPr,
	GithubComment,
	GithubReview,
	GithubReviewComment,
	GithubDiscussion,
	GithubDiscussionComment,
	Commit,
	MailingListMsg,
	IrcLog,
	DelvingTopic,
	DelvingPost,
	Bip,
	Bolt,
	Blip,
	Lud,
	Nut,
	OptechNewsletter,
	OptechTopic,
	OptechBlog,
	BitcointalkTopic,
	BitcointalkPost,
}

impl SourceType {
	pub fn as_str(&self) -> &'static str {
		match self {
			Self::GithubIssue => "github_issue",
			Self::GithubPr => "github_pr",
			Self::GithubComment => "github_comment",
			Self::GithubReview => "github_review",
			Self::GithubReviewComment => "github_review_comment",
			Self::GithubDiscussion => "github_discussion",
			Self::GithubDiscussionComment => "github_discussion_comment",
			Self::Commit => "commit",
			Self::MailingListMsg => "mailing_list_msg",
			Self::IrcLog => "irc_log",
			Self::DelvingTopic => "delving_topic",
			Self::DelvingPost => "delving_post",
			Self::Bip => "bip",
			Self::Bolt => "bolt",
			Self::Blip => "blip",
			Self::Lud => "lud",
			Self::Nut => "nut",
			Self::OptechNewsletter => "optech_newsletter",
			Self::OptechTopic => "optech_topic",
			Self::OptechBlog => "optech_blog",
			Self::BitcointalkTopic => "bitcointalk_topic",
			Self::BitcointalkPost => "bitcointalk_post",
		}
	}

	pub fn from_str(s: &str) -> Option<Self> {
		match s {
			"github_issue" => Some(Self::GithubIssue),
			"github_pr" => Some(Self::GithubPr),
			"github_comment" => Some(Self::GithubComment),
			"github_review" => Some(Self::GithubReview),
			"github_review_comment" => Some(Self::GithubReviewComment),
			"github_discussion" => Some(Self::GithubDiscussion),
			"github_discussion_comment" => Some(Self::GithubDiscussionComment),
			"commit" => Some(Self::Commit),
			"mailing_list_msg" => Some(Self::MailingListMsg),
			"irc_log" => Some(Self::IrcLog),
			"delving_topic" => Some(Self::DelvingTopic),
			"delving_post" => Some(Self::DelvingPost),
			"bip" => Some(Self::Bip),
			"bolt" => Some(Self::Bolt),
			"blip" => Some(Self::Blip),
			"lud" => Some(Self::Lud),
			"nut" => Some(Self::Nut),
			"optech_newsletter" => Some(Self::OptechNewsletter),
			"optech_topic" => Some(Self::OptechTopic),
			"optech_blog" => Some(Self::OptechBlog),
			"bitcointalk_topic" => Some(Self::BitcointalkTopic),
			"bitcointalk_post" => Some(Self::BitcointalkPost),
			_ => None,
		}
	}
}

impl fmt::Display for SourceType {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str(self.as_str())
	}
}

/// A normalized document from any source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
	pub id: String,
	pub source_type: SourceType,
	pub source_repo: Option<String>,
	pub source_id: String,
	pub title: Option<String>,
	pub body: Option<String>,
	pub author: Option<String>,
	pub author_id: Option<String>,
	pub created_at: DateTime<Utc>,
	pub updated_at: Option<DateTime<Utc>>,
	pub parent_id: Option<String>,
	pub metadata: Option<serde_json::Value>,
	pub seq: Option<i64>,
}

impl Document {
	/// Build the canonical document ID from its components.
	pub fn make_id(source_type: &SourceType, source_repo: Option<&str>, source_id: &str) -> String {
		match source_repo {
			Some(repo) => format!("{}:{}:{}", source_type, repo, source_id),
			None => format!("{}:{}", source_type, source_id),
		}
	}

	/// Derive the canonical URL for this document.
	pub fn url(&self) -> Option<String> {
		match self.source_type {
			SourceType::GithubIssue => Some(format!(
				"https://github.com/{}/issues/{}",
				self.source_repo.as_deref()?,
				self.source_id
			)),
			SourceType::GithubPr => Some(format!(
				"https://github.com/{}/pull/{}",
				self.source_repo.as_deref()?,
				self.source_id
			)),
			SourceType::GithubComment => {
				// Extract the issue/PR number from parent_id (e.g. "github_issue:owner/repo:123")
				// to build a proper permalink. GitHub redirects /issues/N to /pull/N for PRs.
				let issue_num = self
					.parent_id
					.as_deref()
					.and_then(|pid| pid.rsplit(':').next())
					.filter(|n| n.chars().all(|c| c.is_ascii_digit()));
				match issue_num {
					Some(num) => Some(format!(
						"https://github.com/{}/issues/{}#issuecomment-{}",
						self.source_repo.as_deref()?,
						num,
						self.source_id
					)),
					None => None,
				}
			},
			SourceType::Commit => Some(format!(
				"https://github.com/{}/commit/{}",
				self.source_repo.as_deref()?,
				self.source_id
			)),
			SourceType::Bip => Some(format!(
				"https://github.com/bitcoin/bips/blob/master/bip-{}.mediawiki",
				self.source_id
			)),
			SourceType::Bolt => Some(format!(
				"https://github.com/lightning/bolts/blob/master/{}.md",
				self.source_id
			)),
			SourceType::Blip => Some(format!(
				"https://github.com/lightning/blips/blob/master/blip-{}.md",
				self.source_id
			)),
			SourceType::Lud => {
				let num: u32 = self.source_id.parse().unwrap_or(0);
				Some(format!("https://github.com/lnurl/luds/blob/luds/{:02}.md", num))
			},
			SourceType::Nut => {
				let num: u32 = self.source_id.parse().unwrap_or(0);
				Some(format!("https://github.com/cashubtc/nuts/blob/main/{:02}.md", num))
			},
			SourceType::DelvingTopic => {
				Some(format!("https://delvingbitcoin.org/t/{}", self.source_id))
			},
			SourceType::DelvingPost => {
				Some(format!("https://delvingbitcoin.org/p/{}", self.source_id))
			},
			SourceType::OptechNewsletter => {
				// The slug (e.g. "2023-03-01-newsletter") is stored in metadata.
				// The URL format is /newsletters/YYYY/MM/DD/.
				let slug =
					self.metadata.as_ref().and_then(|m| m.get("slug")).and_then(|s| s.as_str());
				slug.and_then(|s| {
					let parts: Vec<&str> = s.splitn(4, '-').collect();
					if parts.len() >= 3 {
						Some(format!(
							"https://bitcoinops.org/en/newsletters/{}/{}/{}/",
							parts[0], parts[1], parts[2]
						))
					} else {
						None
					}
				})
			},
			SourceType::OptechTopic => {
				Some(format!("https://bitcoinops.org/en/topics/{}/", self.source_id))
			},
			SourceType::BitcointalkTopic => {
				Some(format!("https://bitcointalk.org/index.php?topic={}.0", self.source_id))
			},
			SourceType::BitcointalkPost => {
				// Extract topic_id from parent_id (e.g., "bitcointalk_topic::{topic_id}")
				let topic_id = self
					.parent_id
					.as_deref()
					.and_then(|pid| pid.strip_prefix("bitcointalk_topic::"));
				match topic_id {
					Some(tid) => Some(format!(
						"https://bitcointalk.org/index.php?topic={}.msg{}#msg{}",
						tid, self.source_id, self.source_id
					)),
					None => None,
				}
			},
			_ => None,
		}
	}
}

/// A cross-reference between documents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reference {
	pub id: Option<i64>,
	pub from_doc_id: String,
	pub to_doc_id: Option<String>,
	pub ref_type: RefType,
	pub to_external: Option<String>,
	pub context: Option<String>,
}

/// Type of cross-reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefType {
	MentionsIssue,
	MentionsPr,
	Fixes,
	ReferencesCommit,
	ReferencesBip,
	ReferencesBolt,
	ReferencesBlip,
	ReferencesLud,
	ReferencesNut,
	RepliesTo,
}

impl RefType {
	pub fn as_str(&self) -> &'static str {
		match self {
			Self::MentionsIssue => "mentions_issue",
			Self::MentionsPr => "mentions_pr",
			Self::Fixes => "fixes",
			Self::ReferencesCommit => "references_commit",
			Self::ReferencesBip => "references_bip",
			Self::ReferencesBolt => "references_bolt",
			Self::ReferencesBlip => "references_blip",
			Self::ReferencesLud => "references_lud",
			Self::ReferencesNut => "references_nut",
			Self::RepliesTo => "replies_to",
		}
	}

	pub fn from_str(s: &str) -> Option<Self> {
		match s {
			"mentions_issue" => Some(Self::MentionsIssue),
			"mentions_pr" => Some(Self::MentionsPr),
			"fixes" => Some(Self::Fixes),
			"references_commit" => Some(Self::ReferencesCommit),
			"references_bip" => Some(Self::ReferencesBip),
			"references_bolt" => Some(Self::ReferencesBolt),
			"references_blip" => Some(Self::ReferencesBlip),
			"references_lud" => Some(Self::ReferencesLud),
			"references_nut" => Some(Self::ReferencesNut),
			"replies_to" => Some(Self::RepliesTo),
			_ => None,
		}
	}
}

impl fmt::Display for RefType {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str(self.as_str())
	}
}

/// Parameters for a search query.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchParams {
	pub query: String,
	pub source_type: Option<Vec<SourceType>>,
	pub source_repo: Option<Vec<String>>,
	pub author: Option<String>,
	pub after: Option<DateTime<Utc>>,
	pub before: Option<DateTime<Utc>>,
	pub semantic: bool,
	pub limit: Option<u32>,
}

/// A single search result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
	pub id: String,
	pub source_type: SourceType,
	pub source_repo: Option<String>,
	pub title: Option<String>,
	pub snippet: Option<String>,
	pub author: Option<String>,
	pub created_at: DateTime<Utc>,
	pub score: f64,
	pub url: Option<String>,
	pub concepts: Vec<String>,
}

/// Container for search results with total count.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResults {
	pub results: Vec<SearchResult>,
	pub total_count: u32,
}

/// Full document context returned by `get_document`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentContext {
	pub document: Document,
	pub url: Option<String>,
	pub outgoing_refs: Vec<Reference>,
	pub incoming_refs: Vec<Reference>,
	pub concepts: Vec<String>,
}

/// A timeline event for a concept.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineEvent {
	pub date: String,
	#[serde(rename = "type")]
	pub source_type: SourceType,
	pub title: Option<String>,
	pub id: String,
	pub url: Option<String>,
}

/// Timeline of a concept across all sources.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Timeline {
	pub concept: String,
	pub events: Vec<TimelineEvent>,
}

/// Context for a commit search result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitContext {
	pub document: Document,
	pub url: Option<String>,
	pub associated_prs: Vec<SearchResult>,
}

/// Sync state for a source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncState {
	pub source_id: String,
	pub source_type: String,
	pub source_repo: Option<String>,
	pub last_cursor: Option<String>,
	pub last_synced_at: Option<DateTime<Utc>>,
	pub next_run_at: Option<DateTime<Utc>>,
	pub status: SyncStatus,
	pub error_message: Option<String>,
	pub retry_count: i32,
	pub items_found: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncStatus {
	Pending,
	Running,
	Ok,
	Error,
}

impl SyncStatus {
	pub fn as_str(&self) -> &'static str {
		match self {
			Self::Pending => "pending",
			Self::Running => "running",
			Self::Ok => "ok",
			Self::Error => "error",
		}
	}

	pub fn from_str(s: &str) -> Self {
		match s {
			"running" => Self::Running,
			"ok" => Self::Ok,
			"error" => Self::Error,
			_ => Self::Pending,
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn make_doc(source_type: SourceType, source_repo: Option<&str>, source_id: &str) -> Document {
		Document {
			id: Document::make_id(&source_type, source_repo, source_id),
			source_type,
			source_repo: source_repo.map(|s| s.to_string()),
			source_id: source_id.to_string(),
			title: None,
			body: None,
			author: None,
			author_id: None,
			created_at: chrono::Utc::now(),
			updated_at: None,
			parent_id: None,
			metadata: None,
			seq: None,
		}
	}

	#[test]
	fn test_comment_url_with_parent_id() {
		let mut doc =
			make_doc(SourceType::GithubComment, Some("lightningdevkit/ldk-sample"), "2135734193");
		doc.parent_id = Some("github_issue:lightningdevkit/ldk-sample:133".to_string());
		assert_eq!(
			doc.url().unwrap(),
			"https://github.com/lightningdevkit/ldk-sample/issues/133#issuecomment-2135734193"
		);
	}

	#[test]
	fn test_comment_url_without_parent_id() {
		let doc =
			make_doc(SourceType::GithubComment, Some("lightningdevkit/ldk-sample"), "2135734193");
		assert!(doc.url().is_none());
	}

	#[test]
	fn test_issue_url() {
		let doc = make_doc(SourceType::GithubIssue, Some("bitcoin/bitcoin"), "12345");
		assert_eq!(doc.url().unwrap(), "https://github.com/bitcoin/bitcoin/issues/12345");
	}

	#[test]
	fn test_bip_url() {
		let doc = make_doc(SourceType::Bip, None, "340");
		assert_eq!(
			doc.url().unwrap(),
			"https://github.com/bitcoin/bips/blob/master/bip-340.mediawiki"
		);
	}

	#[test]
	fn test_optech_newsletter_url_with_slug() {
		let mut doc = make_doc(SourceType::OptechNewsletter, None, "240");
		doc.metadata = Some(serde_json::json!({ "slug": "2023-03-01-newsletter" }));
		assert_eq!(doc.url().unwrap(), "https://bitcoinops.org/en/newsletters/2023/03/01/");
	}

	#[test]
	fn test_optech_newsletter_url_without_slug() {
		let doc = make_doc(SourceType::OptechNewsletter, None, "151");
		assert!(doc.url().is_none());
	}

	#[test]
	fn test_blip_url() {
		let doc = make_doc(SourceType::Blip, None, "1");
		assert_eq!(doc.url().unwrap(), "https://github.com/lightning/blips/blob/master/blip-1.md");
	}

	#[test]
	fn test_lud_url() {
		let doc = make_doc(SourceType::Lud, None, "6");
		assert_eq!(doc.url().unwrap(), "https://github.com/lnurl/luds/blob/luds/06.md");
	}

	#[test]
	fn test_nut_url() {
		let doc = make_doc(SourceType::Nut, None, "0");
		assert_eq!(doc.url().unwrap(), "https://github.com/cashubtc/nuts/blob/main/00.md");
	}

	#[test]
	fn test_bitcointalk_topic_url() {
		let doc = make_doc(SourceType::BitcointalkTopic, None, "5");
		assert_eq!(doc.url().unwrap(), "https://bitcointalk.org/index.php?topic=5.0");
	}

	#[test]
	fn test_bitcointalk_post_url_with_parent() {
		let mut doc = make_doc(SourceType::BitcointalkPost, None, "12345");
		doc.parent_id = Some("bitcointalk_topic::5".to_string());
		assert_eq!(
			doc.url().unwrap(),
			"https://bitcointalk.org/index.php?topic=5.msg12345#msg12345"
		);
	}

	#[test]
	fn test_bitcointalk_post_url_without_parent() {
		let doc = make_doc(SourceType::BitcointalkPost, None, "12345");
		assert!(doc.url().is_none());
	}

	#[test]
	fn test_parse_datetime_rfc3339() {
		let dt = parse_datetime("2023-06-01T00:00:00Z").unwrap();
		assert_eq!(
			dt,
			chrono::NaiveDate::from_ymd_opt(2023, 6, 1)
				.unwrap()
				.and_hms_opt(0, 0, 0)
				.unwrap()
				.and_utc()
		);
	}

	#[test]
	fn test_parse_datetime_rfc3339_with_offset() {
		let dt = parse_datetime("2023-06-01T12:00:00+02:00").unwrap();
		assert_eq!(
			dt,
			chrono::NaiveDate::from_ymd_opt(2023, 6, 1)
				.unwrap()
				.and_hms_opt(10, 0, 0)
				.unwrap()
				.and_utc()
		);
	}

	#[test]
	fn test_parse_datetime_plain_date() {
		let dt = parse_datetime("2023-06-01").unwrap();
		assert_eq!(
			dt,
			chrono::NaiveDate::from_ymd_opt(2023, 6, 1)
				.unwrap()
				.and_hms_opt(0, 0, 0)
				.unwrap()
				.and_utc()
		);
	}

	#[test]
	fn test_parse_datetime_invalid() {
		assert!(parse_datetime("not-a-date").is_none());
		assert!(parse_datetime("2023/06/01").is_none());
		assert!(parse_datetime("").is_none());
	}
}
