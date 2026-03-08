/// Server configuration, including which repos to sync.
pub struct Config {
	dev_subset: bool,
}

impl Config {
	pub fn new(dev_subset: bool) -> Self {
		Self { dev_subset }
	}

	/// Return the list of GitHub repos to sync as (owner, repo) pairs.
	pub fn github_repos(&self) -> Vec<(String, String)> {
		if self.dev_subset {
			vec![("lightningdevkit".to_string(), "ldk-sample".to_string())]
		} else {
			vec![
				// LDK
				("lightningdevkit".to_string(), "rust-lightning".to_string()),
				("lightningdevkit".to_string(), "ldk-node".to_string()),
				("lightningdevkit".to_string(), "ldk-sample".to_string()),
				("lightningdevkit".to_string(), "ldk-server".to_string()),
				("lightningdevkit".to_string(), "ldk-c-bindings".to_string()),
				("lightningdevkit".to_string(), "ldk-garbagecollected".to_string()),
				// Bitcoin Core
				("bitcoin".to_string(), "bitcoin".to_string()),
				// rust-bitcoin
				("rust-bitcoin".to_string(), "rust-bitcoin".to_string()),
				("rust-bitcoin".to_string(), "rust-secp256k1".to_string()),
				("rust-bitcoin".to_string(), "rust-miniscript".to_string()),
				// BDK
				("bitcoindevkit".to_string(), "bdk".to_string()),
				// Payjoin
				("payjoin".to_string(), "rust-payjoin".to_string()),
				// Specs
				("bitcoin".to_string(), "bips".to_string()),
				("lightning".to_string(), "bolts".to_string()),
				// Optech
				("bitcoinops".to_string(), "bitcoinops.github.io".to_string()),
			]
		}
	}

	/// Return IRC channels to sync.
	pub fn irc_channels(&self) -> Vec<String> {
		if self.dev_subset {
			vec!["bitcoin-core-dev".to_string()]
		} else {
			vec![
				"bitcoin-core-dev".to_string(),
				"lightning-dev".to_string(),
				"bitcoin-wizards".to_string(),
			]
		}
	}

	/// Whether to sync Delving Bitcoin.
	pub fn sync_delving(&self) -> bool {
		true
	}
}
