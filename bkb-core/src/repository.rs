//! Explicit migration mappings. A forge mirror or matching name is not a migration.

pub const FORGEJO_BASE: &str = "https://git.rust-bitcoin.org";
pub const FORGEJO_HOST: &str = "git.rust-bitcoin.org";

pub const MIGRATED_REPOS: &[&str] = &[
	"lightningdevkit/rust-lightning",
	"lightningdevkit/ldk-sample",
	"rust-bitcoin/rust-secp256k1",
	"rust-bitcoin/rust-psbt",
	"rust-bitcoin/corepc",
	"rust-bitcoin/bitcoin-payment-instructions",
];

pub fn is_migrated(repo: &str) -> bool {
	MIGRATED_REPOS.contains(&repo)
}

pub fn forgejo_identity(repo: &str) -> String {
	format!("{FORGEJO_HOST}/{repo}")
}

pub fn logical_repo(repo: &str) -> &str {
	repo.strip_prefix("git.rust-bitcoin.org/").filter(|r| is_migrated(r)).unwrap_or(repo)
}

pub fn primary_git_url(repo: &str) -> String {
	if is_migrated(repo) {
		format!("{FORGEJO_BASE}/{repo}.git")
	} else {
		format!("https://github.com/{repo}.git")
	}
}
