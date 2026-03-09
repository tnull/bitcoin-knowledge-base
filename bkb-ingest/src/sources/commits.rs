use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use tracing::{debug, info, warn};

use bkb_core::model::{Document, RefType, Reference, SourceType};

use super::{SyncPage, SyncSource};
use crate::rate_limiter::RateLimiter;
use crate::repo_cache::RepoCache;

/// Maximum size of the changeset diff appended to the commit body (bytes).
const MAX_DIFF_BYTES: usize = 8192;

/// Maximum number of files listed in the changeset summary and metadata.
const MAX_FILES_IN_METADATA: usize = 100;

/// Default number of commits to process per page.
const DEFAULT_PAGE_SIZE: usize = 200;

/// Sync source for git commits from a GitHub repository.
///
/// Clones/fetches bare repos into a local cache and walks commit history
/// to index commit messages and changesets.
pub struct GitCommitSyncSource {
	owner: String,
	repo: String,
	cache: Arc<RepoCache>,
	github_token: Option<String>,
	page_size: usize,
	size_checked: AtomicBool,
	name: String,
}

impl GitCommitSyncSource {
	pub fn new(owner: &str, repo: &str, cache: Arc<RepoCache>, token: Option<String>) -> Self {
		let name = format!("commits:{}/{}", owner, repo);
		Self {
			owner: owner.to_string(),
			repo: repo.to_string(),
			cache,
			github_token: token,
			page_size: DEFAULT_PAGE_SIZE,
			size_checked: AtomicBool::new(false),
			name,
		}
	}
}

#[async_trait]
impl SyncSource for GitCommitSyncSource {
	async fn fetch_page(
		&self, cursor: Option<&str>, rate_limiter: &RateLimiter,
	) -> Result<SyncPage> {
		// One-time repo size check via GitHub API
		if !self.size_checked.swap(true, Ordering::Relaxed) {
			match self
				.cache
				.check_repo_size(
					&self.owner,
					&self.repo,
					self.github_token.as_deref(),
					rate_limiter,
				)
				.await
			{
				Ok(size_kb) => {
					debug!(
						repo = %format!("{}/{}", self.owner, self.repo),
						size_kb, "repo size check passed"
					);
				},
				Err(e) => {
					warn!(
						repo = %format!("{}/{}", self.owner, self.repo),
						error = %e, "repo exceeds size limit, skipping"
					);
					return Ok(SyncPage {
						documents: Vec::new(),
						references: Vec::new(),
						next_cursor: None,
					});
				},
			}
		}

		// Clone or fetch the repo
		let repo_path =
			self.cache.ensure_repo(&self.owner, &self.repo, self.github_token.as_deref()).await?;

		// Determine effective cursor: explicit arg > persisted cursor > None
		let effective_cursor =
			cursor.map(String::from).or_else(|| self.cache.read_cursor(&self.owner, &self.repo));

		// Walk commits in a blocking task
		let owner = self.owner.clone();
		let repo_name = self.repo.clone();
		let page_size = self.page_size;
		let cursor_clone = effective_cursor.clone();

		let (documents, references, last_sha, has_more, head_sha) =
			tokio::task::spawn_blocking(move || {
				walk_commits(&repo_path, cursor_clone.as_deref(), page_size, &owner, &repo_name)
			})
			.await??;

		info!(
			source = %self.name,
			commits = documents.len(),
			head = %head_sha,
			cursor = ?effective_cursor,
			"fetched commits page"
		);

		let (next_cursor, final_cursor) = if has_more {
			// More commits to process -- paginate
			(last_sha.clone(), None)
		} else {
			// All done -- persist HEAD as the cursor for next cycle
			(None, Some(head_sha))
		};

		// Persist cursor if we've finished walking all commits
		if let Some(ref sha) = final_cursor {
			self.cache.write_cursor(&self.owner, &self.repo, sha)?;
		}

		Ok(SyncPage { documents, references, next_cursor })
	}

	fn poll_interval(&self) -> Duration {
		Duration::from_secs(3600)
	}

	fn name(&self) -> &str {
		// Leak the name for the 'static lifetime the trait expects
		Box::leak(self.name.clone().into_boxed_str())
	}
}

/// Walk commits from HEAD backwards, skipping those already processed (cursor).
///
/// Returns `(documents, references, last_processed_sha, has_more, head_sha)`.
fn walk_commits(
	repo_path: &std::path::Path, cursor: Option<&str>, page_size: usize, owner: &str,
	repo_name: &str,
) -> Result<(Vec<Document>, Vec<Reference>, Option<String>, bool, String)> {
	let git_repo = git2::Repository::open_bare(repo_path)
		.with_context(|| format!("failed to open bare repo at {}", repo_path.display()))?;

	// Resolve HEAD: try main, then master, then HEAD
	let head_oid = resolve_default_branch(&git_repo)?;
	let head_sha = head_oid.to_string();

	// If cursor matches HEAD, nothing new
	if cursor == Some(head_sha.as_str()) {
		debug!(head = %head_sha, "cursor matches HEAD, nothing to do");
		return Ok((Vec::new(), Vec::new(), None, false, head_sha));
	}

	let mut revwalk = git_repo.revwalk()?;
	revwalk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::TIME)?;
	revwalk.push(head_oid)?;

	// Hide the cursor commit and all its ancestors
	if let Some(cursor_sha) = cursor {
		if let Ok(cursor_oid) = git2::Oid::from_str(cursor_sha) {
			if let Err(e) = revwalk.hide(cursor_oid) {
				warn!(
					cursor = %cursor_sha,
					error = %e,
					"failed to hide cursor commit (may have been force-pushed away)"
				);
				// Continue anyway -- we'll just re-process some commits
			}
		}
	}

	let source_repo = format!("{}/{}", owner, repo_name);
	let mut documents = Vec::with_capacity(page_size);
	let mut references = Vec::new();
	let mut last_sha = None;
	let mut count = 0;

	for oid_result in revwalk {
		let oid = oid_result?;

		if count >= page_size {
			// More commits remain
			return Ok((documents, references, last_sha, true, head_sha));
		}

		let commit = git_repo.find_commit(oid)?;
		let sha = oid.to_string();

		let doc = build_commit_document(&git_repo, &commit, &sha, &source_repo)?;

		// Extract merge commit → PR reference
		if let Some(pr_ref) = extract_merge_pr_ref(&commit, &doc.id, &source_repo) {
			references.push(pr_ref);
		}

		last_sha = Some(sha);
		documents.push(doc);
		count += 1;
	}

	Ok((documents, references, last_sha, false, head_sha))
}

/// Resolve the default branch OID (main > master > HEAD).
fn resolve_default_branch(repo: &git2::Repository) -> Result<git2::Oid> {
	// Try refs/heads/main first
	if let Ok(reference) = repo.find_reference("refs/heads/main") {
		if let Some(oid) = reference.target() {
			return Ok(oid);
		}
	}

	// Fall back to refs/heads/master
	if let Ok(reference) = repo.find_reference("refs/heads/master") {
		if let Some(oid) = reference.target() {
			return Ok(oid);
		}
	}

	// Fall back to HEAD
	let head = repo.head().context("failed to resolve HEAD")?;
	head.target().context("HEAD does not point to a commit")
}

/// Build a `Document` from a git commit, including changeset summary and
/// truncated diff.
///
/// For merge commits, the diff is skipped entirely (merge diffs are huge
/// and rarely useful) -- only the commit message and parent SHAs are recorded.
fn build_commit_document(
	repo: &git2::Repository, commit: &git2::Commit, sha: &str, source_repo: &str,
) -> Result<Document> {
	let message = commit.message().unwrap_or("").to_string();
	let title = message.lines().next().unwrap_or("").to_string();

	// Author info
	let author_sig = commit.author();
	let author_name = author_sig.name().unwrap_or("unknown").to_string();
	let author_email = author_sig.email().unwrap_or("").to_string();
	let author_time = author_sig.when();
	let created_at = git_time_to_chrono(&author_time);

	let parents: Vec<String> = commit.parent_ids().map(|oid| oid.to_string()).collect();
	let is_merge = parents.len() > 1;

	// Compute diff once (skip entirely for merge commits)
	let diff_info = if is_merge { None } else { compute_diff(repo, commit) };

	// Build body: commit message + changeset (non-merge only)
	let body = match diff_info {
		Some(ref info) if !info.changeset_text.is_empty() => {
			format!("{}\n\n---\nChangeset:\n{}", message, info.changeset_text)
		},
		_ => message.clone(),
	};

	// Build metadata
	let (files_meta, files_changed, insertions, deletions) = match diff_info {
		Some(ref info) => {
			let meta: Vec<serde_json::Value> = info
				.file_stats
				.iter()
				.take(MAX_FILES_IN_METADATA)
				.map(|f| {
					serde_json::json!({
						"path": f.path,
						"status": f.status,
					})
				})
				.collect();
			(meta, info.files_changed, info.insertions, info.deletions)
		},
		None => (Vec::new(), 0, 0, 0),
	};

	let metadata = serde_json::json!({
		"is_merge": is_merge,
		"parents": parents,
		"files_changed": files_changed,
		"insertions": insertions,
		"deletions": deletions,
		"files": files_meta,
	});

	let id = Document::make_id(&SourceType::Commit, Some(source_repo), sha);

	Ok(Document {
		id,
		source_type: SourceType::Commit,
		source_repo: Some(source_repo.to_string()),
		source_id: sha.to_string(),
		title: Some(title),
		body: Some(body),
		author: Some(author_name),
		author_id: Some(author_email),
		created_at,
		updated_at: None,
		parent_id: None,
		metadata: Some(metadata),
		seq: None,
	})
}

/// All diff-derived information for a commit, computed in a single pass.
struct DiffInfo {
	changeset_text: String,
	file_stats: Vec<FileStat>,
	files_changed: usize,
	insertions: usize,
	deletions: usize,
}

/// File-level diff stat info.
struct FileStat {
	path: String,
	status: String,
}

/// Compute the diff for a commit against its first parent (or empty tree for
/// root commits). Returns `None` if the diff cannot be computed.
///
/// Extracts changeset text (file list + truncated unified diff), file stats,
/// and aggregate insertion/deletion counts -- all from a single diff.
fn compute_diff(repo: &git2::Repository, commit: &git2::Commit) -> Option<DiffInfo> {
	let tree = commit.tree().ok()?;

	let parent_tree = if commit.parent_count() > 0 {
		commit.parent(0).ok().and_then(|p| p.tree().ok())
	} else {
		None
	};

	let diff = repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), None).ok()?;

	// Aggregate stats
	let stats = diff.stats().ok();
	let files_changed = stats.as_ref().map(|s| s.files_changed()).unwrap_or(0);
	let insertions = stats.as_ref().map(|s| s.insertions()).unwrap_or(0);
	let deletions = stats.as_ref().map(|s| s.deletions()).unwrap_or(0);

	// File stats + changeset text (file list portion)
	let mut changeset_text = String::new();
	let mut file_stats = Vec::new();

	// Summary line
	if let Some(ref stats) = stats {
		changeset_text.push_str(&format!(
			"{} file(s) changed, {} insertion(s)(+), {} deletion(s)(-)\n",
			stats.files_changed(),
			stats.insertions(),
			stats.deletions(),
		));
	}

	// File list with status
	let num_deltas = diff.deltas().len();
	for (i, delta) in diff.deltas().enumerate() {
		let (status_char, status_str) = match delta.status() {
			git2::Delta::Added => ('A', "added"),
			git2::Delta::Deleted => ('D', "deleted"),
			git2::Delta::Modified => ('M', "modified"),
			git2::Delta::Renamed => ('R', "renamed"),
			git2::Delta::Copied => ('C', "copied"),
			git2::Delta::Typechange => ('T', "typechange"),
			_ => ('?', "unknown"),
		};

		let path = delta
			.new_file()
			.path()
			.or_else(|| delta.old_file().path())
			.map(|p| p.to_string_lossy().to_string())
			.unwrap_or_else(|| "<unknown>".to_string());

		if i < MAX_FILES_IN_METADATA {
			changeset_text.push_str(&format!("{} {}\n", status_char, path));
			file_stats.push(FileStat { path, status: status_str.to_string() });
		} else if i == MAX_FILES_IN_METADATA {
			changeset_text.push_str(&format!("... and {} more files\n", num_deltas - i));
		}
	}

	// Truncated unified diff
	changeset_text.push('\n');

	let mut diff_bytes = 0usize;
	let mut truncated = false;

	diff.print(git2::DiffFormat::Patch, |_delta, _hunk, line| {
		if truncated {
			return false;
		}

		let content = std::str::from_utf8(line.content()).unwrap_or("");
		let prefix = match line.origin() {
			'+' | '-' | ' ' => {
				let mut s = String::with_capacity(1);
				s.push(line.origin());
				s
			},
			_ => String::new(),
		};

		let line_str = format!("{}{}", prefix, content);
		diff_bytes += line_str.len();

		if diff_bytes > MAX_DIFF_BYTES {
			truncated = true;
			return false;
		}

		changeset_text.push_str(&line_str);
		true
	})
	.ok();

	if truncated {
		changeset_text.push_str(&format!("\n[truncated at {} bytes]\n", MAX_DIFF_BYTES));
	}

	Some(DiffInfo { changeset_text, file_stats, files_changed, insertions, deletions })
}

/// Convert a `git2::Time` to `chrono::DateTime<Utc>`.
fn git_time_to_chrono(time: &git2::Time) -> DateTime<Utc> {
	Utc.timestamp_opt(time.seconds(), 0).single().unwrap_or_else(Utc::now)
}

/// Extract a PR reference from a merge commit message.
///
/// GitHub merge commits follow the pattern:
///   "Merge pull request #N from owner/branch"
/// This creates a `MentionsPr` reference linking the merge commit to the PR.
fn extract_merge_pr_ref(
	commit: &git2::Commit, doc_id: &str, source_repo: &str,
) -> Option<Reference> {
	use regex::Regex;

	thread_local! {
		static RE_MERGE_PR: Regex = Regex::new(
			r"^Merge pull request #(\d+) from "
		).unwrap();
	}

	let message = commit.message()?;

	RE_MERGE_PR.with(|re| {
		re.captures(message).map(|cap| {
			let pr_num = &cap[1];
			Reference {
				id: None,
				from_doc_id: doc_id.to_string(),
				to_doc_id: None,
				ref_type: RefType::MentionsPr,
				to_external: Some(format!("{}#{}", source_repo, pr_num)),
				context: Some(cap[0].to_string()),
			}
		})
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Create a temp bare repo with a few commits for testing.
	fn create_test_repo() -> (tempfile::TempDir, std::path::PathBuf) {
		let tmp = tempfile::tempdir().unwrap();
		let repo_path = tmp.path().join("test-repo.git");

		let repo = git2::Repository::init_bare(&repo_path).unwrap();

		// Create initial commit
		let sig = git2::Signature::new(
			"Test Author",
			"test@example.com",
			&git2::Time::new(1700000000, 0),
		)
		.unwrap();

		// Build tree with a file
		let mut index = repo.index().unwrap();
		let blob_oid = repo.blob(b"fn main() {\n    println!(\"hello\");\n}\n").unwrap();
		index
			.add(&git2::IndexEntry {
				ctime: git2::IndexTime::new(0, 0),
				mtime: git2::IndexTime::new(0, 0),
				dev: 0,
				ino: 0,
				mode: 0o100644,
				uid: 0,
				gid: 0,
				file_size: 0,
				id: blob_oid,
				flags: 0,
				flags_extended: 0,
				path: b"src/main.rs".to_vec(),
			})
			.unwrap();
		let tree_oid = index.write_tree().unwrap();
		let tree = repo.find_tree(tree_oid).unwrap();

		let commit1_oid = repo
			.commit(
				Some("refs/heads/main"),
				&sig,
				&sig,
				"Initial commit\n\nSet up the project with a simple main function.",
				&tree,
				&[],
			)
			.unwrap();

		// Second commit: modify the file
		let blob2_oid = repo
			.blob(b"fn main() {\n    let config = Config::new();\n    println!(\"hello BIP-340\");\n}\n")
			.unwrap();
		let mut index2 = repo.index().unwrap();
		index2
			.add(&git2::IndexEntry {
				ctime: git2::IndexTime::new(0, 0),
				mtime: git2::IndexTime::new(0, 0),
				dev: 0,
				ino: 0,
				mode: 0o100644,
				uid: 0,
				gid: 0,
				file_size: 0,
				id: blob2_oid,
				flags: 0,
				flags_extended: 0,
				path: b"src/main.rs".to_vec(),
			})
			.unwrap();
		let tree2_oid = index2.write_tree().unwrap();
		let tree2 = repo.find_tree(tree2_oid).unwrap();
		let commit1 = repo.find_commit(commit1_oid).unwrap();

		let sig2 =
			git2::Signature::new("Another Dev", "dev@example.com", &git2::Time::new(1700001000, 0))
				.unwrap();
		let _commit2_oid = repo
			.commit(
				Some("refs/heads/main"),
				&sig2,
				&sig2,
				"Add config and BIP-340 reference\n\nThis implements schnorr signature handling.\nFixes #42",
				&tree2,
				&[&commit1],
			)
			.unwrap();

		// Third commit: add a new file
		let blob3_oid = repo.blob(b"# README\nA simple project.\n").unwrap();
		let mut index3 = repo.index().unwrap();
		// Keep existing file
		index3
			.add(&git2::IndexEntry {
				ctime: git2::IndexTime::new(0, 0),
				mtime: git2::IndexTime::new(0, 0),
				dev: 0,
				ino: 0,
				mode: 0o100644,
				uid: 0,
				gid: 0,
				file_size: 0,
				id: blob2_oid,
				flags: 0,
				flags_extended: 0,
				path: b"src/main.rs".to_vec(),
			})
			.unwrap();
		index3
			.add(&git2::IndexEntry {
				ctime: git2::IndexTime::new(0, 0),
				mtime: git2::IndexTime::new(0, 0),
				dev: 0,
				ino: 0,
				mode: 0o100644,
				uid: 0,
				gid: 0,
				file_size: 0,
				id: blob3_oid,
				flags: 0,
				flags_extended: 0,
				path: b"README.md".to_vec(),
			})
			.unwrap();
		let tree3_oid = index3.write_tree().unwrap();
		let tree3 = repo.find_tree(tree3_oid).unwrap();
		let commit2 = repo.find_commit(_commit2_oid).unwrap();

		let sig3 = git2::Signature::new(
			"Test Author",
			"test@example.com",
			&git2::Time::new(1700002000, 0),
		)
		.unwrap();
		let _commit3_oid = repo
			.commit(Some("refs/heads/main"), &sig3, &sig3, "Add README", &tree3, &[&commit2])
			.unwrap();

		(tmp, repo_path)
	}

	#[test]
	fn test_walk_commits_full_history() {
		let (_tmp, repo_path) = create_test_repo();

		let (docs, _refs, _last_sha, has_more, _head_sha) =
			walk_commits(&repo_path, None, 100, "test-owner", "test-repo").unwrap();

		assert_eq!(docs.len(), 3, "should have 3 commits");
		assert!(!has_more, "should not have more commits");

		// Commits are in reverse chronological order (newest first)
		assert_eq!(docs[0].title.as_deref(), Some("Add README"));
		assert_eq!(docs[1].title.as_deref(), Some("Add config and BIP-340 reference"));
		assert_eq!(docs[2].title.as_deref(), Some("Initial commit"));

		// Check source type and repo
		for doc in &docs {
			assert_eq!(doc.source_type, SourceType::Commit);
			assert_eq!(doc.source_repo.as_deref(), Some("test-owner/test-repo"));
			assert_eq!(doc.source_id.len(), 40, "SHA should be 40 chars");
		}
	}

	#[test]
	fn test_walk_commits_incremental() {
		let (_tmp, repo_path) = create_test_repo();

		// Get all commits first
		let (all_docs, _, _, _, head_sha) =
			walk_commits(&repo_path, None, 100, "test-owner", "test-repo").unwrap();
		assert_eq!(all_docs.len(), 3);

		// Now use the first commit's SHA as cursor -- should get only 2 newer commits
		let first_commit_sha = &all_docs[2].source_id; // oldest commit
		let (new_docs, _, _, has_more, _) =
			walk_commits(&repo_path, Some(first_commit_sha), 100, "test-owner", "test-repo")
				.unwrap();

		assert_eq!(new_docs.len(), 2, "should have 2 new commits after cursor");
		assert!(!has_more);

		// Using HEAD as cursor should yield nothing
		let (empty_docs, _, _, _, _) =
			walk_commits(&repo_path, Some(&head_sha), 100, "test-owner", "test-repo").unwrap();
		assert!(empty_docs.is_empty(), "should have no new commits when cursor == HEAD");
	}

	#[test]
	fn test_walk_commits_pagination() {
		let (_tmp, repo_path) = create_test_repo();

		// Fetch only 2 commits per page
		let (page1, _, last_sha, has_more, _head_sha) =
			walk_commits(&repo_path, None, 2, "test-owner", "test-repo").unwrap();

		assert_eq!(page1.len(), 2);
		assert!(has_more, "should indicate more commits");
		assert!(last_sha.is_some());

		// Fetch remaining
		let (page2, _, _, has_more2, _) =
			walk_commits(&repo_path, last_sha.as_deref(), 100, "test-owner", "test-repo").unwrap();

		assert_eq!(page2.len(), 1, "should have 1 remaining commit");
		assert!(!has_more2);
	}

	#[test]
	fn test_commit_document_fields() {
		let (_tmp, repo_path) = create_test_repo();

		let (docs, _, _, _, _) =
			walk_commits(&repo_path, None, 100, "test-owner", "test-repo").unwrap();

		// Check the second commit (has BIP reference and Fixes)
		let bip_commit = &docs[1]; // "Add config and BIP-340 reference"
		assert_eq!(bip_commit.author.as_deref(), Some("Another Dev"));
		assert_eq!(bip_commit.author_id.as_deref(), Some("dev@example.com"));

		// Body should contain changeset
		let body = bip_commit.body.as_deref().unwrap();
		assert!(body.contains("---\nChangeset:"), "body should contain changeset separator");
		assert!(body.contains("src/main.rs"), "body should list changed files");
		assert!(body.contains("BIP-340"), "body should contain BIP-340 reference");

		// Metadata should have diff stats
		let meta = bip_commit.metadata.as_ref().unwrap();
		assert_eq!(meta["is_merge"], false);
		assert!(meta["files_changed"].as_u64().unwrap() >= 1);

		// ID should be properly formed
		let expected_id = format!("commit:test-owner/test-repo:{}", bip_commit.source_id);
		assert_eq!(bip_commit.id, expected_id);
	}

	#[test]
	fn test_changeset_includes_diff() {
		let (_tmp, repo_path) = create_test_repo();

		let (docs, _, _, _, _) =
			walk_commits(&repo_path, None, 100, "test-owner", "test-repo").unwrap();

		// The "Add README" commit adds a new file
		let readme_commit = &docs[0];
		let body = readme_commit.body.as_deref().unwrap();
		assert!(body.contains("A README.md"), "should show added file");

		// The second commit modifies main.rs
		let modify_commit = &docs[1];
		let body = modify_commit.body.as_deref().unwrap();
		assert!(body.contains("M src/main.rs"), "should show modified file");
	}

	#[test]
	fn test_root_commit_has_changeset() {
		let (_tmp, repo_path) = create_test_repo();

		let (docs, _, _, _, _) =
			walk_commits(&repo_path, None, 100, "test-owner", "test-repo").unwrap();

		// Initial commit (root) should also have a changeset
		let root = &docs[2];
		let body = root.body.as_deref().unwrap();
		assert!(body.contains("---\nChangeset:"), "root commit should have changeset");
		assert!(body.contains("src/main.rs"), "root commit should list files");
	}

	#[test]
	fn test_merge_commit_pr_reference() {
		let tmp = tempfile::tempdir().unwrap();
		let repo_path = tmp.path().join("merge-test.git");
		let repo = git2::Repository::init_bare(&repo_path).unwrap();

		let sig =
			git2::Signature::new("Dev", "dev@test.com", &git2::Time::new(1700000000, 0)).unwrap();

		// Create initial commit on main
		let mut index = repo.index().unwrap();
		let blob_oid = repo.blob(b"initial content\n").unwrap();
		index
			.add(&git2::IndexEntry {
				ctime: git2::IndexTime::new(0, 0),
				mtime: git2::IndexTime::new(0, 0),
				dev: 0,
				ino: 0,
				mode: 0o100644,
				uid: 0,
				gid: 0,
				file_size: 0,
				id: blob_oid,
				flags: 0,
				flags_extended: 0,
				path: b"file.txt".to_vec(),
			})
			.unwrap();
		let tree_oid = index.write_tree().unwrap();
		let tree = repo.find_tree(tree_oid).unwrap();
		let base_oid =
			repo.commit(Some("refs/heads/main"), &sig, &sig, "Initial commit", &tree, &[]).unwrap();
		let base_commit = repo.find_commit(base_oid).unwrap();

		// Create a "feature branch" commit
		let blob2_oid = repo.blob(b"feature content\n").unwrap();
		let mut index2 = repo.index().unwrap();
		index2
			.add(&git2::IndexEntry {
				ctime: git2::IndexTime::new(0, 0),
				mtime: git2::IndexTime::new(0, 0),
				dev: 0,
				ino: 0,
				mode: 0o100644,
				uid: 0,
				gid: 0,
				file_size: 0,
				id: blob2_oid,
				flags: 0,
				flags_extended: 0,
				path: b"file.txt".to_vec(),
			})
			.unwrap();
		let tree2_oid = index2.write_tree().unwrap();
		let tree2 = repo.find_tree(tree2_oid).unwrap();
		let feature_oid =
			repo.commit(None, &sig, &sig, "Add feature", &tree2, &[&base_commit]).unwrap();
		let feature_commit = repo.find_commit(feature_oid).unwrap();

		// Create a merge commit with GitHub-style message
		let merge_oid = repo
			.commit(
				Some("refs/heads/main"),
				&sig,
				&sig,
				"Merge pull request #42 from user/feature-branch\n\nAdd feature",
				&tree2,
				&[&base_commit, &feature_commit],
			)
			.unwrap();

		let (_docs, refs, _, _, _) =
			walk_commits(&repo_path, None, 100, "test-owner", "test-repo").unwrap();

		// Should have a MentionsPr reference from the merge commit
		let pr_refs: Vec<_> = refs.iter().filter(|r| r.ref_type == RefType::MentionsPr).collect();
		assert_eq!(pr_refs.len(), 1, "should have 1 merge PR reference");
		assert_eq!(pr_refs[0].to_external.as_deref(), Some("test-owner/test-repo#42"));

		// The merge commit should also have is_merge=true in metadata
		let merge_doc = _docs
			.iter()
			.find(|d| d.source_id == merge_oid.to_string())
			.expect("merge commit should be in documents");
		let meta = merge_doc.metadata.as_ref().unwrap();
		assert_eq!(meta["is_merge"], true);
	}

	#[test]
	fn test_extract_merge_pr_ref_parses_github_format() {
		let tmp = tempfile::tempdir().unwrap();
		let repo_path = tmp.path().join("ref-test.git");
		let repo = git2::Repository::init_bare(&repo_path).unwrap();

		let sig =
			git2::Signature::new("Dev", "dev@test.com", &git2::Time::new(1700000000, 0)).unwrap();

		let blob_oid = repo.blob(b"content").unwrap();
		let mut index = repo.index().unwrap();
		index
			.add(&git2::IndexEntry {
				ctime: git2::IndexTime::new(0, 0),
				mtime: git2::IndexTime::new(0, 0),
				dev: 0,
				ino: 0,
				mode: 0o100644,
				uid: 0,
				gid: 0,
				file_size: 0,
				id: blob_oid,
				flags: 0,
				flags_extended: 0,
				path: b"f.txt".to_vec(),
			})
			.unwrap();
		let tree_oid = index.write_tree().unwrap();
		let tree = repo.find_tree(tree_oid).unwrap();

		let oid = repo
			.commit(
				Some("refs/heads/main"),
				&sig,
				&sig,
				"Merge pull request #123 from user/branch\n\nSome description",
				&tree,
				&[],
			)
			.unwrap();
		let commit = repo.find_commit(oid).unwrap();
		let doc_id = "commit:owner/repo:abc";

		let pr_ref = extract_merge_pr_ref(&commit, doc_id, "owner/repo");
		assert!(pr_ref.is_some(), "should extract merge PR reference");

		let r = pr_ref.unwrap();
		assert_eq!(r.ref_type, RefType::MentionsPr);
		assert_eq!(r.to_external.as_deref(), Some("owner/repo#123"));
		assert_eq!(r.from_doc_id, doc_id);
	}

	#[test]
	fn test_extract_merge_pr_ref_non_merge() {
		let tmp = tempfile::tempdir().unwrap();
		let repo_path = tmp.path().join("noref-test.git");
		let repo = git2::Repository::init_bare(&repo_path).unwrap();

		let sig =
			git2::Signature::new("Dev", "dev@test.com", &git2::Time::new(1700000000, 0)).unwrap();

		let blob_oid = repo.blob(b"content").unwrap();
		let mut index = repo.index().unwrap();
		index
			.add(&git2::IndexEntry {
				ctime: git2::IndexTime::new(0, 0),
				mtime: git2::IndexTime::new(0, 0),
				dev: 0,
				ino: 0,
				mode: 0o100644,
				uid: 0,
				gid: 0,
				file_size: 0,
				id: blob_oid,
				flags: 0,
				flags_extended: 0,
				path: b"f.txt".to_vec(),
			})
			.unwrap();
		let tree_oid = index.write_tree().unwrap();
		let tree = repo.find_tree(tree_oid).unwrap();

		let oid = repo
			.commit(Some("refs/heads/main"), &sig, &sig, "Just a regular commit", &tree, &[])
			.unwrap();
		let commit = repo.find_commit(oid).unwrap();

		let pr_ref = extract_merge_pr_ref(&commit, "doc:1", "owner/repo");
		assert!(pr_ref.is_none(), "regular commit should not have merge PR reference");
	}
}
