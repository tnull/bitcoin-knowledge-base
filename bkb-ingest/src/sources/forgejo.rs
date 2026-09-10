//! Forgejo repository conversations and pull request reviews.
use super::http::{more_pages, timestamp, PollCursor};
use super::{SyncPage, SyncSource};
use crate::rate_limiter::RateLimiter;
use anyhow::{Context, Result};
use async_trait::async_trait;
use bkb_core::model::{Document, SourceType};
use serde_json::Value;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

pub struct ForgejoClient {
	base: String,
	client: reqwest::Client,
	token: Option<String>,
	next_request: Mutex<Instant>,
	interval: Duration,
}

impl ForgejoClient {
	pub fn new(base: &str, token: Option<String>) -> Result<Self> {
		let url = url::Url::parse(base)?;
		anyhow::ensure!(
			url.scheme() == "https"
				|| (url.scheme() == "http" && url.host_str() == Some("127.0.0.1")),
			"Forgejo requires HTTPS"
		);
		anyhow::ensure!(
			url.username().is_empty()
				&& url.password().is_none()
				&& url.query().is_none()
				&& url.fragment().is_none(),
			"invalid Forgejo base URL"
		);
		Ok(Self {
			base: base.trim_end_matches('/').into(),
			client: super::http::client(),
			token,
			next_request: Mutex::new(Instant::now()),
			interval: Duration::from_secs(1),
		})
	}

	/// Read repository metadata using the same credentials and quota as feeds.
	pub async fn repository_metadata(&self, repo: &str) -> Result<Value> {
		Ok(self.get(repo, "", &[]).await?.0)
	}

	async fn get(
		&self, repo: &str, path: &str, query: &[(String, String)],
	) -> Result<(Value, reqwest::header::HeaderMap)> {
		{
			let mut next = self.next_request.lock().await;
			let now = Instant::now();
			if *next > now {
				return Err(crate::rate_limiter::Deferred(*next - now).into());
			}
			*next = now + self.interval;
		}
		let mut url = format!("{}/api/v1/repos/{repo}", self.base);
		if !path.is_empty() {
			url.push('/');
			url.push_str(path);
		}
		let mut request = self
			.client
			.get(url)
			.header("User-Agent", "bkb/0.2")
			.header("Accept", "application/json")
			.query(query);
		if let Some(token) = &self.token {
			request = request.header("Authorization", format!("token {token}"));
		}
		let response = request.send().await.context("Forgejo request failed")?;
		if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS
			|| response.headers().contains_key("retry-after")
		{
			let delay = super::http::retry_after(response.headers());
			*self.next_request.lock().await = Instant::now() + delay;
			return Err(crate::rate_limiter::Deferred(delay).into());
		}
		let response = response.error_for_status().context("Forgejo API error")?;
		let headers = response.headers().clone();
		Ok((response.json().await.context("invalid Forgejo response")?, headers))
	}

	async fn list(
		&self, repo: &str, path: &str, mut query: Vec<(String, String)>, page: u32,
	) -> Result<(Vec<Value>, bool)> {
		query.extend([("page".into(), page.to_string()), ("limit".into(), "50".into())]);
		let (value, headers) = self.get(repo, path, &query).await?;
		let items = value.as_array().context("expected Forgejo list")?.clone();
		let more = more_pages(&headers, page, 50, items.len());
		Ok((items, more))
	}
}

#[derive(Clone, Copy)]
pub enum ForgejoFeed {
	Issues,
	Comments,
	Reviews,
}

pub struct ForgejoSyncSource {
	client: Arc<ForgejoClient>,
	repo: String,
	identity: String,
	feed: ForgejoFeed,
	name: String,
}

impl ForgejoSyncSource {
	pub fn new(client: Arc<ForgejoClient>, owner: &str, repo: &str, feed: ForgejoFeed) -> Self {
		let repo = format!("{owner}/{repo}");
		let identity = format!(
			"{}/{repo}",
			client.base.trim_start_matches("https://").trim_start_matches("http://")
		);
		let kind = match feed {
			ForgejoFeed::Issues => "issues",
			ForgejoFeed::Comments => "comments",
			ForgejoFeed::Reviews => "reviews",
		};
		let name = format!("forgejo:{identity}:{kind}");
		Self { client, repo, identity, feed, name }
	}
}

#[async_trait]
impl SyncSource for ForgejoSyncSource {
	async fn fetch_page(&self, previous: Option<&str>, _: &RateLimiter) -> Result<SyncPage> {
		let mut cursor = PollCursor::resume(previous)?;
		if let Some(&pr) = cursor.pending.first() {
			if matches!(self.feed, ForgejoFeed::Issues) {
				let result = self.client.get(&self.repo, &format!("pulls/{pr}"), &[]).await;
				let documents = match result {
					Ok((item, _)) => vec![self.document(&item, SourceType::ForgejoPr, None)?],
					Err(e) if is_not_found(&e) => {
						tracing::warn!(repo = %self.repo, pr, "PR details unavailable; retaining issue-feed record");
						vec![]
					},
					Err(e) => return Err(e),
				};
				cursor.pending.remove(0);
				let more = !cursor.pending.is_empty() || cursor.more_parents;
				return cursor.result(documents, more);
			}
			if let Some(&review) = cursor.review_ids.first() {
				let result = self
					.client
					.get(&self.repo, &format!("pulls/{pr}/reviews/{review}/comments"), &[])
					.await;
				let items = match result {
					Ok((items, _)) => items,
					Err(e) if is_not_found(&e) => {
						tracing::warn!(repo = %self.repo, pr, review, "Review deleted; retaining stored comments");
						Value::Array(vec![])
					},
					Err(e) => return Err(e),
				};
				let documents = items
					.as_array()
					.context("expected review comment list")?
					.iter()
					.map(|item| self.document(item, SourceType::ForgejoReviewComment, Some(pr)))
					.collect::<Result<Vec<_>>>()?;
				cursor.review_ids.remove(0);
				if cursor.review_ids.is_empty() && cursor.child_page == 0 {
					cursor.pending.remove(0);
					cursor.child_page = 1;
				}
				let more = !cursor.pending.is_empty() || cursor.more_parents;
				return cursor.result(documents, more);
			}
			let result = self
				.client
				.list(&self.repo, &format!("pulls/{pr}/reviews"), vec![], cursor.child_page)
				.await;
			let (items, more) = match result {
				Ok(page) => page,
				Err(e) if is_not_found(&e) => {
					tracing::warn!(repo = %self.repo, pr, "PR deleted; retaining stored reviews");
					(vec![], false)
				},
				Err(e) => return Err(e),
			};
			let mut documents = vec![];
			for item in items {
				if item["state"] == "PENDING" || !item["submitted_at"].is_string() {
					continue;
				}
				cursor.review_ids.push(item["id"].as_u64().context("missing review ID")?);
				documents.push(self.document(&item, SourceType::ForgejoReview, Some(pr))?);
			}
			cursor.child_page = if more { cursor.child_page + 1 } else { 0 };
			if cursor.review_ids.is_empty() && !more {
				cursor.pending.remove(0);
				cursor.child_page = 1;
			}
			let more = !cursor.pending.is_empty() || cursor.more_parents;
			return cursor.result(documents, more);
		}
		let comments = matches!(self.feed, ForgejoFeed::Comments);
		let mut query = vec![("before".into(), cursor.until.to_rfc3339())];
		if let Some(since) = &cursor.since {
			query.push(("since".into(), since.clone()));
		}
		if !comments {
			query.extend([("state".into(), "all".into()), ("sort".into(), "leastupdate".into())]);
		}
		let (items, more) = self
			.client
			.list(
				&self.repo,
				if comments { "issues/comments" } else { "issues" },
				query,
				cursor.page,
			)
			.await?;
		cursor.more_parents = more;
		cursor.page += 1;
		let mut documents = vec![];
		for item in items {
			let is_pr = item["pull_request"].is_object();
			if is_pr {
				cursor.pending.push(item["number"].as_u64().context("missing PR number")?);
			}
			if !matches!(self.feed, ForgejoFeed::Reviews) {
				let kind = if comments {
					SourceType::ForgejoComment
				} else if is_pr {
					SourceType::ForgejoPr
				} else {
					SourceType::ForgejoIssue
				};
				documents.push(self.document(&item, kind, None)?);
			}
		}
		let more = !cursor.pending.is_empty() || cursor.more_parents;
		cursor.result(documents, more)
	}
	fn name(&self) -> &str {
		&self.name
	}
	fn poll_interval(&self) -> Duration {
		Duration::from_secs(3600)
	}
}

fn is_not_found(error: &anyhow::Error) -> bool {
	error.downcast_ref::<reqwest::Error>().and_then(|e| e.status())
		== Some(reqwest::StatusCode::NOT_FOUND)
}

impl ForgejoSyncSource {
	fn document(&self, item: &Value, kind: SourceType, parent: Option<u64>) -> Result<Document> {
		let thread = matches!(kind, SourceType::ForgejoIssue | SourceType::ForgejoPr);
		let source_id = item[if thread { "number" } else { "id" }]
			.as_u64()
			.context("missing Forgejo document ID")?
			.to_string();
		let created_at = timestamp(
			item,
			if kind == SourceType::ForgejoReview { "submitted_at" } else { "created_at" },
		)?;
		let parent_id = if let Some(pr) = parent {
			Some(Document::make_id(&SourceType::ForgejoPr, Some(&self.identity), &pr.to_string()))
		} else if !thread {
			let (url, parent_kind) =
				if item["pull_request_url"].as_str().is_some_and(|s| !s.is_empty()) {
					(item["pull_request_url"].as_str(), SourceType::ForgejoPr)
				} else {
					(item["issue_url"].as_str(), SourceType::ForgejoIssue)
				};
			let number = url
				.and_then(|u| u.rsplit('/').next())
				.and_then(|n| n.parse::<u64>().ok())
				.context("missing comment parent")?;
			Some(Document::make_id(&parent_kind, Some(&self.identity), &number.to_string()))
		} else {
			None
		};
		let mut metadata = serde_json::Map::new();
		for field in [
			"state",
			"labels",
			"merged",
			"merged_at",
			"merge_commit_sha",
			"head",
			"base",
			"draft",
			"pull_request",
			"original_author",
			"original_author_id",
			"commit_id",
			"original_commit_id",
			"path",
			"diff_hunk",
			"position",
			"original_position",
			"pull_request_review_id",
			"resolver",
			"dismissed",
			"stale",
			"comments_count",
		] {
			if let Some(value) = item.get(field) {
				metadata.insert(field.into(), value.clone());
			}
		}
		metadata.insert("url".into(), item["html_url"].clone());
		Ok(Document {
			id: Document::make_id(&kind, Some(&self.identity), &source_id),
			source_type: kind,
			source_repo: Some(self.identity.clone()),
			source_id,
			title: item["title"].as_str().map(str::to_owned),
			body: item["body"].as_str().map(str::to_owned),
			author: item["original_author"]
				.as_str()
				.filter(|s| !s.is_empty())
				.or_else(|| item["user"]["login"].as_str())
				.map(str::to_owned),
			author_id: item["user"]["id"].as_u64().map(|id| id.to_string()),
			created_at,
			updated_at: if item["updated_at"].is_string() {
				Some(timestamp(item, "updated_at")?)
			} else {
				None
			},
			parent_id,
			metadata: Some(Value::Object(metadata)),
			seq: None,
		})
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::{json, Value};
	use tokio::io::{AsyncReadExt, AsyncWriteExt};

	async fn server(
		replies: Vec<Value>,
	) -> (Arc<ForgejoClient>, tokio::task::JoinHandle<Vec<String>>) {
		server_with_status(replies.into_iter().map(|body| (200, body)).collect()).await
	}

	async fn server_with_status(
		replies: Vec<(u16, Value)>,
	) -> (Arc<ForgejoClient>, tokio::task::JoinHandle<Vec<String>>) {
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let addr = listener.local_addr().unwrap();
		let handle = tokio::spawn(async move {
			let mut requests = vec![];
			for (status, body) in replies {
				let (mut socket, _) = listener.accept().await.unwrap();
				let mut buf = [0; 8192];
				let n = socket.read(&mut buf).await.unwrap();
				requests.push(String::from_utf8_lossy(&buf[..n]).into_owned());
				let body = body.to_string();
				socket.write_all(format!("HTTP/1.1 {status} Mock\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
			}
			requests
		});
		let mut client = ForgejoClient::new(&format!("http://{addr}"), None).unwrap();
		client.interval = Duration::ZERO;
		(Arc::new(client), handle)
	}

	fn issue(number: u64) -> Value {
		json!({"id": 1000 + number, "number": number, "title": "An issue", "body": "HTLC timeout",
			"state": "open", "created_at": "2020-01-01T00:00:00Z", "updated_at": "2020-01-02T00:00:00Z",
			"user": {"id": 1, "login": "alice"}, "labels": [], "original_author": "",
			"html_url": format!("https://git.rust-bitcoin.org/test/repo/issues/{number}")})
	}

	#[tokio::test]
	async fn deleted_review_does_not_block_later_review_comments() {
		let inline =
			json!({"id": 91, "body": "Keep this discussion", "created_at": "2020-01-02T00:00:00Z"});
		let (client, server) =
			server_with_status(vec![(404, json!({})), (200, json!([inline]))]).await;
		let source = ForgejoSyncSource::new(client, "test", "repo", ForgejoFeed::Reviews);
		let mut cursor = PollCursor::resume(None).unwrap();
		cursor.pending = vec![7];
		cursor.review_ids = vec![8, 9];
		cursor.child_page = 0;
		cursor.more_parents = false;
		let previous = serde_json::to_string(&cursor).unwrap();
		let result = source.fetch_page(Some(&previous), &RateLimiter::new(0)).await;
		assert!(
			result.is_ok(),
			"a deleted review must advance its checkpoint: {:?}",
			result.as_ref().err()
		);
		let page = result.unwrap();
		assert!(page.documents.is_empty());
		let page =
			source.fetch_page(page.next_cursor.as_deref(), &RateLimiter::new(0)).await.unwrap();
		assert_eq!(page.documents[0].body.as_deref(), Some("Keep this discussion"));
		assert!(page.next_cursor.is_none());
		let requests = server.await.unwrap();
		assert!(requests[1].contains("/reviews/9/comments"));
	}

	#[tokio::test]
	async fn deleted_pr_does_not_block_later_reviews() {
		let review = json!({"id": 9, "body": "ACK", "state": "APPROVED", "submitted_at": "2020-01-02T00:00:00Z"});
		let (client, server) =
			server_with_status(vec![(404, json!({})), (200, json!([review]))]).await;
		let source = ForgejoSyncSource::new(client, "test", "repo", ForgejoFeed::Reviews);
		let mut cursor = PollCursor::resume(None).unwrap();
		cursor.pending = vec![7, 8];
		cursor.more_parents = false;
		let previous = serde_json::to_string(&cursor).unwrap();
		let result = source.fetch_page(Some(&previous), &RateLimiter::new(0)).await;
		assert!(
			result.is_ok(),
			"a deleted PR must advance its review checkpoint: {:?}",
			result.as_ref().err()
		);
		let page = result.unwrap();
		let page =
			source.fetch_page(page.next_cursor.as_deref(), &RateLimiter::new(0)).await.unwrap();
		assert_eq!(page.documents[0].source_id, "9");
		let requests = server.await.unwrap();
		assert!(requests[1].contains("/pulls/8/reviews?"));
	}

	#[tokio::test]
	async fn nested_review_auth_and_server_errors_remain_retryable() {
		for status in [401, 403, 500] {
			for inline in [false, true] {
				let (client, server) = server_with_status(vec![(status, json!({}))]).await;
				let source = ForgejoSyncSource::new(client, "test", "repo", ForgejoFeed::Reviews);
				let mut cursor = PollCursor::resume(None).unwrap();
				cursor.pending = vec![7];
				if inline {
					cursor.review_ids = vec![8];
				}
				let previous = serde_json::to_string(&cursor).unwrap();
				assert!(
					source.fetch_page(Some(&previous), &RateLimiter::new(0)).await.is_err(),
					"HTTP {status} must not skip a review"
				);
				server.await.unwrap();
			}
		}
	}

	#[tokio::test]
	async fn ingest_forgejo_issue_without_github_identity() {
		let (client, server) = server(vec![json!([issue(4760)])]).await;
		let source = ForgejoSyncSource::new(client, "test", "repo", ForgejoFeed::Issues);
		let page = source.fetch_page(None, &RateLimiter::new(0)).await.unwrap();
		assert_eq!(page.documents.len(), 1, "must ingest Forgejo issues");
		let doc = &page.documents[0];
		assert_eq!(doc.source_type.as_str(), "forgejo_issue");
		assert!(doc.id.contains("127.0.0.1:"), "source identity must include the host");
		assert_eq!(
			doc.url().as_deref(),
			Some("https://git.rust-bitcoin.org/test/repo/issues/4760")
		);
		let requests = server.await.unwrap();
		assert!(requests[0].contains("page=1") && requests[0].contains("limit=50"));
		assert!(page.checkpoint.is_some());
	}

	#[tokio::test]
	async fn resume_nested_forgejo_reviews_and_inline_comments() {
		let mut pr = issue(7);
		pr["pull_request"] = json!({"merged": false});
		let review = json!({"id": 8, "body": "", "state": "APPROVED", "user": null,
			"submitted_at": "2020-01-02T00:00:00Z", "updated_at": "2020-01-02T00:00:00Z",
			"html_url": "https://git.rust-bitcoin.org/test/repo/pulls/7#issuecomment-90"});
		let inline = json!({"id": 91, "body": "ACK", "user": null,
			"created_at": "2020-01-02T00:00:00Z", "updated_at": "2020-01-02T00:00:00Z",
			"path": "src/lib.rs", "position": 4, "pull_request_review_id": 8,
			"pull_request_url": "https://git.rust-bitcoin.org/test/repo/pulls/7",
			"html_url": "https://git.rust-bitcoin.org/test/repo/pulls/7#issuecomment-91"});
		let (client, server) = server(vec![json!([pr]), json!([review]), json!([inline])]).await;
		let mut cursor = None;
		let mut docs = vec![];
		for _ in 0..3 {
			let source =
				ForgejoSyncSource::new(client.clone(), "test", "repo", ForgejoFeed::Reviews);
			let page = source.fetch_page(cursor.as_deref(), &RateLimiter::new(0)).await.unwrap();
			docs.extend(page.documents);
			cursor = page.next_cursor;
		}
		assert_eq!(docs.len(), 2, "must ingest review and inline discussion");
		assert_eq!(docs[0].source_type.as_str(), "forgejo_review");
		assert_eq!(docs[1].source_type.as_str(), "forgejo_review_comment");
		assert!(docs.iter().all(|d| d.parent_id.as_ref().unwrap().starts_with("forgejo_pr:")));
		assert!(cursor.is_none());
		let requests = server.await.unwrap();
		assert!(
			requests[2].contains("/pulls/7/reviews/8/comments"),
			"must fetch review-specific inline comments"
		);
	}
}
