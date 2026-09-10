//! GitHub review ingestion, independent of repository migrations.
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use bkb_core::model::{Document, SourceType};
use serde_json::Value;

use super::http::{client, github_get, timestamp, PollCursor};
use super::{SyncPage, SyncSource};
use crate::rate_limiter::RateLimiter;

pub struct GitHubReviewSyncSource {
	api_base: String,
	client: reqwest::Client,
	repo: String,
	token: Option<String>,
	inline: bool,
	name: String,
}

impl GitHubReviewSyncSource {
	pub fn new(owner: &str, repo: &str, token: Option<String>, inline: bool) -> Self {
		let repo = format!("{owner}/{repo}");
		let name = format!("github:{repo}:{}", if inline { "review_comments" } else { "reviews" });
		Self {
			api_base: "https://api.github.com".into(),
			client: client(),
			repo,
			token,
			inline,
			name,
		}
	}
}

#[async_trait]
impl SyncSource for GitHubReviewSyncSource {
	async fn fetch_page(&self, cursor: Option<&str>, rate: &RateLimiter) -> Result<SyncPage> {
		let mut cursor = PollCursor::resume(cursor)?;
		if self.inline {
			let mut query =
				vec![("sort".into(), "updated".into()), ("direction".into(), "asc".into())];
			if let Some(since) = &cursor.since {
				query.push(("since".into(), since.clone()));
			}
			let (items, more) = github_get(
				&self.client,
				&self.api_base,
				&self.repo,
				"pulls/comments",
				self.token.as_deref(),
				rate,
				&query,
				cursor.page,
			)
			.await?;
			let mut documents = Vec::new();
			let mut reached_end = false;
			for item in items {
				if timestamp(&item, "updated_at")? > cursor.until {
					reached_end = true;
					continue;
				}
				let parent = item["pull_request_url"]
					.as_str()
					.and_then(|s| s.rsplit('/').next())
					.and_then(|s| s.parse::<u64>().ok())
					.context("missing PR number on review comment")?;
				documents.push(review_document(&item, &self.repo, parent, true)?);
			}
			cursor.page += 1;
			return cursor.result(documents, more && !reached_end);
		}
		if let Some(&pr) = cursor.pending.first() {
			let (items, more) = github_get(
				&self.client,
				&self.api_base,
				&self.repo,
				&format!("pulls/{pr}/reviews"),
				self.token.as_deref(),
				rate,
				&[],
				cursor.child_page,
			)
			.await?;
			let documents = items
				.iter()
				.filter(|item| item["submitted_at"].is_string())
				.map(|item| review_document(item, &self.repo, pr, false))
				.collect::<Result<Vec<_>>>()?;
			if more {
				cursor.child_page += 1;
			} else {
				cursor.pending.remove(0);
				cursor.child_page = 1;
			}
			let more = !cursor.pending.is_empty() || cursor.more_parents;
			return cursor.result(documents, more);
		}
		let mut query = vec![
			("state".into(), "all".into()),
			("sort".into(), "updated".into()),
			("direction".into(), "asc".into()),
		];
		if let Some(since) = &cursor.since {
			query.push(("since".into(), since.clone()));
		}
		let (items, more) = github_get(
			&self.client,
			&self.api_base,
			&self.repo,
			"issues",
			self.token.as_deref(),
			rate,
			&query,
			cursor.page,
		)
		.await?;
		cursor.more_parents = more;
		for item in items {
			if timestamp(&item, "updated_at")? > cursor.until {
				cursor.more_parents = false;
				continue;
			}
			if item["pull_request"].is_object() {
				cursor.pending.push(item["number"].as_u64().context("missing PR number")?);
			}
		}
		cursor.page += 1;
		let more = !cursor.pending.is_empty() || cursor.more_parents;
		cursor.result(vec![], more)
	}
	fn name(&self) -> &str {
		&self.name
	}
	fn poll_interval(&self) -> Duration {
		Duration::from_secs(3600)
	}
}

fn review_document(value: &Value, repo: &str, parent: u64, inline: bool) -> Result<Document> {
	let source_type =
		if inline { SourceType::GithubReviewComment } else { SourceType::GithubReview };
	let source_id = value["id"].as_u64().context("missing review ID")?.to_string();
	let created_at = timestamp(value, if inline { "created_at" } else { "submitted_at" })?;
	let mut metadata = serde_json::Map::new();
	for field in [
		"state",
		"commit_id",
		"original_commit_id",
		"path",
		"diff_hunk",
		"line",
		"original_line",
		"position",
		"original_position",
		"side",
		"start_line",
		"start_side",
		"pull_request_review_id",
		"in_reply_to_id",
	] {
		if let Some(v) = value.get(field) {
			metadata.insert(field.into(), v.clone());
		}
	}
	metadata.insert("url".into(), value["html_url"].clone());
	Ok(Document {
		id: Document::make_id(&source_type, Some(repo), &source_id),
		source_type,
		source_repo: Some(repo.into()),
		source_id,
		title: None,
		body: value["body"].as_str().map(str::to_owned),
		author: value["user"]["login"].as_str().map(str::to_owned),
		author_id: value["user"]["id"].as_u64().map(|id| id.to_string()),
		created_at,
		updated_at: if value["updated_at"].is_string() {
			Some(timestamp(value, "updated_at")?)
		} else {
			None
		},
		parent_id: Some(Document::make_id(&SourceType::GithubPr, Some(repo), &parent.to_string())),
		metadata: Some(Value::Object(metadata)),
		seq: None,
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use bkb_core::model::SourceType;
	use tokio::io::{AsyncReadExt, AsyncWriteExt};

	#[tokio::test]
	async fn resume_review_backfill_between_review_pages() {
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let addr = listener.local_addr().unwrap();
		let review = |id, body| {
			serde_json::json!({
				"id": id, "body": body, "state": "APPROVED", "user": {"login": "alice", "id": 8},
				"submitted_at": "2020-01-01T00:00:00Z", "html_url": "https://github.com/test/repo/pull/7#pullrequestreview-3"
			})
		};
		let replies = [
			(
				serde_json::json!([{"number": 7, "pull_request": {}, "updated_at": "2020-01-02T00:00:00Z"}]),
				"",
			),
			(
				serde_json::json!([review(3, "")]),
				"Link: <https://api.github.com/next>; rel=\"next\"\r\n",
			),
			(serde_json::json!([review(4, "ACK")]), ""),
		];
		let server = tokio::spawn(async move {
			let mut requests = Vec::new();
			for (body, headers) in replies {
				let (mut socket, _) = listener.accept().await.unwrap();
				let mut buf = [0; 8192];
				let n = socket.read(&mut buf).await.unwrap();
				requests.push(String::from_utf8_lossy(&buf[..n]).into_owned());
				let body = body.to_string();
				socket.write_all(format!("HTTP/1.1 200 OK\r\n{headers}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
			}
			requests
		});
		let mut cursor = None;
		let mut documents = Vec::new();
		for _ in 0..3 {
			// A new adapter on every call exercises persisted, not in-memory progress.
			let mut source = GitHubReviewSyncSource::new("test", "repo", None, false);
			source.api_base = format!("http://{addr}");
			let page = source.fetch_page(cursor.as_deref(), &RateLimiter::new(0)).await.unwrap();
			documents.extend(page.documents);
			cursor = page.next_cursor;
		}
		assert!(cursor.is_none(), "all reviews must complete the backfill");
		assert_eq!(documents.len(), 2, "empty approval reviews must be retained");
		assert_eq!(documents[0].source_type, SourceType::GithubReview);
		let requests = server.await.unwrap();
		assert!(
			requests[2].contains("/pulls/7/reviews?page=2"),
			"review pagination must survive restarts"
		);
	}

	#[tokio::test]
	async fn ingest_inline_review_with_parent_and_permalink() {
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let addr = listener.local_addr().unwrap();
		let response = serde_json::json!([{
			"id": 42, "body": "Check the HTLC timeout", "user": null,
			"created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-02T00:00:00Z",
			"pull_request_url": "https://api.github.com/repos/test/repo/pulls/7",
			"html_url": "https://github.com/test/repo/pull/7#discussion_r42",
			"pull_request_review_id": 3, "path": "src/lib.rs", "diff_hunk": "@@ -1 +1 @@",
			"commit_id": "abcdef1234567", "line": 9, "in_reply_to_id": 41
		}])
		.to_string();
		let server = tokio::spawn(async move {
			let (mut socket, _) = listener.accept().await.unwrap();
			let mut request = [0u8; 8192];
			let n = socket.read(&mut request).await.unwrap();
			let request = String::from_utf8_lossy(&request[..n]);
			assert!(
				request.contains("/repos/test/repo/pulls/comments?"),
				"must fetch inline review endpoint"
			);
			assert!(request.contains("page=1"), "pagination must start explicitly at page one");
			socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", response.len(), response).as_bytes()).await.unwrap();
		});
		let mut source = GitHubReviewSyncSource::new("test", "repo", None, true);
		source.api_base = format!("http://{}", addr);
		let page = source.fetch_page(None, &RateLimiter::new(0)).await.unwrap();
		assert_eq!(page.documents.len(), 1, "must ingest inline review comments");
		let doc = &page.documents[0];
		assert_eq!(doc.source_type, SourceType::GithubReviewComment);
		assert_eq!(doc.parent_id.as_deref(), Some("github_pr:test/repo:7"));
		assert_eq!(
			doc.url().as_deref(),
			Some("https://github.com/test/repo/pull/7#discussion_r42")
		);
		assert_eq!(doc.metadata.as_ref().unwrap()["path"], "src/lib.rs");
		assert!(doc.author.is_none(), "deleted authors must be accepted");
		assert!(page.checkpoint.is_some(), "final page must advance the polling checkpoint");
		server.await.unwrap();
	}
}
