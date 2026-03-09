use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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
	OptechNewsletter,
	OptechTopic,
	OptechBlog,
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
			Self::OptechNewsletter => "optech_newsletter",
			Self::OptechTopic => "optech_topic",
			Self::OptechBlog => "optech_blog",
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
			"optech_newsletter" => Some(Self::OptechNewsletter),
			"optech_topic" => Some(Self::OptechTopic),
			"optech_blog" => Some(Self::OptechBlog),
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
				// Comment IDs don't directly map to URLs without the issue number,
				// but we can use the GitHub API URL pattern.
				Some(format!(
					"https://github.com/{}/issues/comments/{}",
					self.source_repo.as_deref()?,
					self.source_id
				))
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
			SourceType::DelvingTopic => {
				Some(format!("https://delvingbitcoin.org/t/{}", self.source_id))
			},
			SourceType::DelvingPost => {
				Some(format!("https://delvingbitcoin.org/p/{}", self.source_id))
			},
			SourceType::OptechNewsletter => {
				Some(format!("https://bitcoinops.org/en/newsletters/{}/", self.source_id))
			},
			SourceType::OptechTopic => {
				Some(format!("https://bitcoinops.org/en/topics/{}/", self.source_id))
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
