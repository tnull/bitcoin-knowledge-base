use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use anyhow::{Context, Result};
use reqwest::Client;
use serde::Deserialize;
use tracing::{debug, info, warn};

use crate::rate_limiter::RateLimiter;

const GITHUB_API_BASE: &str = "https://api.github.com";

/// Cache for bare git clones, with LRU eviction and per-repo size gating.
pub struct RepoCache {
	cache_dir: PathBuf,
	max_cache_bytes: u64,
	max_repo_size_kb: u64,
	access_log: Mutex<HashMap<String, SystemTime>>,
	client: Client,
}

impl RepoCache {
	/// Create a new repo cache, creating the cache directory if needed.
	///
	/// Scans existing repos to populate the access log from filesystem mtime.
	pub fn new(cache_dir: PathBuf, max_cache_bytes: u64, max_repo_size_kb: u64) -> Result<Self> {
		std::fs::create_dir_all(&cache_dir)
			.with_context(|| format!("failed to create cache dir: {}", cache_dir.display()))?;

		let mut access_log = HashMap::new();

		// Scan existing cached repos to populate access log
		if let Ok(owners) = std::fs::read_dir(&cache_dir) {
			for owner_entry in owners.flatten() {
				if !owner_entry.path().is_dir() {
					continue;
				}
				let owner_name = owner_entry.file_name().to_string_lossy().to_string();
				if let Ok(repos) = std::fs::read_dir(owner_entry.path()) {
					for repo_entry in repos.flatten() {
						let repo_name = repo_entry.file_name().to_string_lossy().to_string();
						if repo_name.ends_with(".git") && repo_entry.path().is_dir() {
							let repo_stem = &repo_name[..repo_name.len() - 4];
							let key = format!("{}/{}", owner_name, repo_stem);
							let mtime = repo_entry
								.metadata()
								.ok()
								.and_then(|m| m.modified().ok())
								.unwrap_or(SystemTime::UNIX_EPOCH);
							access_log.insert(key, mtime);
						}
					}
				}
			}
		}

		debug!(cache_dir = %cache_dir.display(), repos = access_log.len(), "repo cache initialized");

		Ok(Self {
			cache_dir,
			max_cache_bytes,
			max_repo_size_kb,
			access_log: Mutex::new(access_log),
			client: Client::new(),
		})
	}

	/// Return the path where a bare clone for `owner/repo` would live.
	pub fn repo_path(&self, owner: &str, repo: &str) -> PathBuf {
		self.cache_dir.join(owner).join(format!("{}.git", repo))
	}

	/// Check the repo size via GitHub API. Returns size in KB.
	/// Errors if the repo exceeds `max_repo_size_kb`.
	pub async fn check_repo_size(
		&self, owner: &str, repo: &str, token: Option<&str>, rate_limiter: &RateLimiter,
	) -> Result<u64> {
		rate_limiter.acquire().await;

		let url = format!("{}/repos/{}/{}", GITHUB_API_BASE, owner, repo);
		let mut req = self
			.client
			.get(&url)
			.header("User-Agent", "bkb/0.1")
			.header("Accept", "application/vnd.github+json");

		if let Some(token) = token {
			req = req.header("Authorization", format!("Bearer {}", token));
		}

		let response = req.send().await.context("failed to fetch repo metadata")?;
		rate_limiter.update_from_response(response.headers());

		let status = response.status();
		if !status.is_success() {
			let body = response.text().await.unwrap_or_default();
			anyhow::bail!("GitHub API returned {} for {}/{}: {}", status, owner, repo, body);
		}

		let meta: GitHubRepoMeta =
			response.json().await.context("failed to parse repo metadata")?;

		if meta.size > self.max_repo_size_kb {
			anyhow::bail!(
				"repo {}/{} is {} KB, exceeding limit of {} KB",
				owner,
				repo,
				meta.size,
				self.max_repo_size_kb,
			);
		}

		debug!(repo = %format!("{}/{}", owner, repo), size_kb = meta.size, "repo size check passed");
		Ok(meta.size)
	}

	/// Ensure the repo is cloned (or fetched if already cached).
	/// Returns the path to the bare repo.
	pub async fn ensure_repo(
		&self, owner: &str, repo: &str, token: Option<&str>,
	) -> Result<PathBuf> {
		let path = self.repo_path(owner, repo);
		let key = format!("{}/{}", owner, repo);
		let token_owned = token.map(String::from);

		if path.exists() {
			// Try to open and fetch; if corrupted, delete and re-clone
			let fetch_path = path.clone();
			let fetch_token = token_owned.clone();
			let fetch_owner = owner.to_string();
			let fetch_repo = repo.to_string();
			let result = tokio::task::spawn_blocking(move || {
				Self::fetch_bare(&fetch_path, &fetch_owner, &fetch_repo, fetch_token.as_deref())
			})
			.await?;

			match result {
				Ok(()) => {
					debug!(repo = %key, "fetched updates for cached repo");
				},
				Err(e) => {
					warn!(repo = %key, error = %e, "cached repo corrupted, re-cloning");
					let _ = std::fs::remove_dir_all(&path);
					let clone_path = path.clone();
					let clone_token = token_owned;
					let clone_owner = owner.to_string();
					let clone_repo = repo.to_string();
					tokio::task::spawn_blocking(move || {
						Self::clone_bare(
							&clone_path,
							&clone_owner,
							&clone_repo,
							clone_token.as_deref(),
						)
					})
					.await??;
					info!(repo = %key, "re-cloned after corruption");
				},
			}
		} else {
			// Evict if needed, then clone
			self.evict_if_needed()?;

			// Create parent directory
			if let Some(parent) = path.parent() {
				std::fs::create_dir_all(parent)?;
			}

			let clone_path = path.clone();
			let clone_token = token_owned;
			let clone_owner = owner.to_string();
			let clone_repo = repo.to_string();
			tokio::task::spawn_blocking(move || {
				Self::clone_bare(&clone_path, &clone_owner, &clone_repo, clone_token.as_deref())
			})
			.await??;
			info!(repo = %key, "cloned new bare repo");
		}

		// Update access log
		if let Ok(mut log) = self.access_log.lock() {
			log.insert(key, SystemTime::now());
		}

		Ok(path)
	}

	/// Read the cursor (last processed HEAD SHA) for a repo.
	pub fn read_cursor(&self, owner: &str, repo: &str) -> Option<String> {
		let cursor_path = self.repo_path(owner, repo).join(".bkb_cursor");
		std::fs::read_to_string(cursor_path)
			.ok()
			.map(|s| s.trim().to_string())
			.filter(|s| !s.is_empty())
	}

	/// Write the cursor (last processed HEAD SHA) for a repo.
	pub fn write_cursor(&self, owner: &str, repo: &str, sha: &str) -> Result<()> {
		let cursor_path = self.repo_path(owner, repo).join(".bkb_cursor");
		std::fs::write(&cursor_path, sha)
			.with_context(|| format!("failed to write cursor to {}", cursor_path.display()))?;
		Ok(())
	}

	/// Clone a repo as a bare clone. Runs in a blocking context.
	fn clone_bare(path: &Path, owner: &str, repo: &str, token: Option<&str>) -> Result<()> {
		let url = format!("https://github.com/{}/{}.git", owner, repo);

		let mut callbacks = git2::RemoteCallbacks::new();
		if let Some(token) = token {
			let token = token.to_string();
			callbacks.credentials(move |_url, _username, _allowed| {
				git2::Cred::userpass_plaintext("x-access-token", &token)
			});
		}

		let mut fetch_opts = git2::FetchOptions::new();
		fetch_opts.remote_callbacks(callbacks);

		let mut builder = git2::build::RepoBuilder::new();
		builder.bare(true);
		builder.fetch_options(fetch_opts);

		builder.clone(&url, path).map_err(|e| {
			tracing::error!(
				repo = %format!("{}/{}", owner, repo),
				error_class = e.class() as i32,
				error_code = e.code() as i32,
				error_message = %e.message(),
				"git2 clone failed"
			);
			anyhow::anyhow!(
				"failed to clone {}/{}: {} (class={}, code={})",
				owner,
				repo,
				e.message(),
				e.class() as i32,
				e.code() as i32
			)
		})?;

		Ok(())
	}

	/// Fetch updates for an existing bare clone. Runs in a blocking context.
	fn fetch_bare(path: &Path, owner: &str, repo: &str, token: Option<&str>) -> Result<()> {
		let git_repo = git2::Repository::open_bare(path)
			.with_context(|| format!("failed to open bare repo at {}", path.display()))?;

		let mut remote = git_repo
			.find_remote("origin")
			.with_context(|| format!("no 'origin' remote in {}/{}", owner, repo))?;

		let mut callbacks = git2::RemoteCallbacks::new();
		if let Some(token) = token {
			let token = token.to_string();
			callbacks.credentials(move |_url, _username, _allowed| {
				git2::Cred::userpass_plaintext("x-access-token", &token)
			});
		}

		let mut fetch_opts = git2::FetchOptions::new();
		fetch_opts.remote_callbacks(callbacks);

		// Fetch all branches
		let refspecs: Vec<String> =
			remote.refspecs().filter_map(|r| r.str().map(String::from)).collect();
		let refspec_strs: Vec<&str> = refspecs.iter().map(|s| s.as_str()).collect();

		remote
			.fetch(&refspec_strs, Some(&mut fetch_opts), None)
			.with_context(|| format!("failed to fetch {}/{}", owner, repo))?;

		Ok(())
	}

	/// Evict least-recently-used repos if the cache exceeds `max_cache_bytes`.
	fn evict_if_needed(&self) -> Result<()> {
		let total_size = dir_size(&self.cache_dir);
		if total_size <= self.max_cache_bytes {
			return Ok(());
		}

		let mut entries: Vec<(String, SystemTime, u64)> = Vec::new();

		if let Ok(log) = self.access_log.lock() {
			for (key, &access_time) in log.iter() {
				let parts: Vec<&str> = key.splitn(2, '/').collect();
				if parts.len() == 2 {
					let path = self.repo_path(parts[0], parts[1]);
					if path.exists() {
						let size = dir_size(&path);
						entries.push((key.clone(), access_time, size));
					}
				}
			}
		}

		// Sort by access time ascending (oldest first)
		entries.sort_by_key(|(_, t, _)| *t);

		let mut freed = 0u64;
		let target = total_size.saturating_sub(self.max_cache_bytes);

		for (key, _, size) in &entries {
			if freed >= target {
				break;
			}

			let parts: Vec<&str> = key.splitn(2, '/').collect();
			if parts.len() == 2 {
				let path = self.repo_path(parts[0], parts[1]);
				if let Err(e) = std::fs::remove_dir_all(&path) {
					warn!(repo = %key, error = %e, "failed to evict cached repo");
					continue;
				}
				info!(repo = %key, size_mb = size / (1024 * 1024), "evicted cached repo");
				freed += size;

				if let Ok(mut log) = self.access_log.lock() {
					log.remove(key.as_str());
				}
			}
		}

		Ok(())
	}
}

/// Recursively compute the size of a directory in bytes.
pub fn dir_size(path: &Path) -> u64 {
	let mut total = 0u64;
	if let Ok(entries) = std::fs::read_dir(path) {
		for entry in entries.flatten() {
			let ft = entry.file_type();
			if let Ok(ft) = ft {
				if ft.is_file() {
					total += entry.metadata().map(|m| m.len()).unwrap_or(0);
				} else if ft.is_dir() {
					total += dir_size(&entry.path());
				}
			}
		}
	}
	total
}

#[derive(Debug, Deserialize)]
struct GitHubRepoMeta {
	size: u64, // KB
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_repo_path_structure() {
		let cache =
			RepoCache::new(PathBuf::from("/tmp/bkb-test-cache"), 10 * 1024 * 1024 * 1024, 4096)
				.unwrap();
		let path = cache.repo_path("lightningdevkit", "rust-lightning");
		assert_eq!(path, PathBuf::from("/tmp/bkb-test-cache/lightningdevkit/rust-lightning.git"));
	}

	#[test]
	fn test_cursor_round_trip() {
		let tmp = tempfile::tempdir().unwrap();
		let cache_dir = tmp.path().to_path_buf();
		let cache = RepoCache::new(cache_dir, 10 * 1024 * 1024 * 1024, 4096).unwrap();

		// Create the repo directory so we can write a cursor file
		let repo_dir = cache.repo_path("test-owner", "test-repo");
		std::fs::create_dir_all(&repo_dir).unwrap();

		assert!(cache.read_cursor("test-owner", "test-repo").is_none());

		let sha = "abc123def456789012345678901234567890abcd";
		cache.write_cursor("test-owner", "test-repo", sha).unwrap();

		let cursor = cache.read_cursor("test-owner", "test-repo");
		assert_eq!(cursor.as_deref(), Some(sha));
	}

	#[test]
	fn test_evict_if_needed_no_eviction_needed() {
		let tmp = tempfile::tempdir().unwrap();
		let cache_dir = tmp.path().to_path_buf();
		// Very large limit -- no eviction should happen
		let cache = RepoCache::new(cache_dir, u64::MAX, 4096).unwrap();
		assert!(cache.evict_if_needed().is_ok());
	}

	#[test]
	fn test_evict_if_needed_evicts_lru() {
		let tmp = tempfile::tempdir().unwrap();
		let cache_dir = tmp.path().to_path_buf();

		// Create two fake repo dirs with some data
		let repo1_dir = cache_dir.join("owner1").join("repo1.git");
		let repo2_dir = cache_dir.join("owner2").join("repo2.git");
		std::fs::create_dir_all(&repo1_dir).unwrap();
		std::fs::create_dir_all(&repo2_dir).unwrap();

		// Write some data to each
		std::fs::write(repo1_dir.join("data"), vec![0u8; 1000]).unwrap();
		std::fs::write(repo2_dir.join("data"), vec![0u8; 1000]).unwrap();

		// Set max to 1500 bytes (both repos together are ~2000)
		let cache = RepoCache::new(cache_dir.clone(), 1500, 4096).unwrap();

		// Make repo1 older than repo2 in the access log
		{
			let mut log = cache.access_log.lock().unwrap();
			log.insert("owner1/repo1".to_string(), SystemTime::UNIX_EPOCH);
			log.insert("owner2/repo2".to_string(), SystemTime::now());
		}

		cache.evict_if_needed().unwrap();

		// repo1 (LRU) should have been evicted
		assert!(!repo1_dir.exists(), "repo1 should have been evicted");
		// repo2 (more recent) should still exist
		assert!(repo2_dir.exists(), "repo2 should still exist");
	}

	#[test]
	fn test_dir_size() {
		let tmp = tempfile::tempdir().unwrap();
		let dir = tmp.path();

		std::fs::write(dir.join("file1"), vec![0u8; 100]).unwrap();
		std::fs::create_dir(dir.join("subdir")).unwrap();
		std::fs::write(dir.join("subdir").join("file2"), vec![0u8; 200]).unwrap();

		let size = dir_size(dir);
		assert!(size >= 300, "expected at least 300 bytes, got {}", size);
	}
}
