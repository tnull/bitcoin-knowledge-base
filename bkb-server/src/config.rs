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
				("lightningdevkit".to_string(), "vss-server".to_string()),
				("lightningdevkit".to_string(), "vss-client".to_string()),
				("lightningdevkit".to_string(), "rapid-gossip-sync-server".to_string()),
				("lightningdevkit".to_string(), "ldk-swift".to_string()),
				("lightningdevkit".to_string(), "ldk-review-club".to_string()),
				("lightningdevkit".to_string(), "orange-sdk".to_string()),
				// Bitcoin Core
				("bitcoin".to_string(), "bitcoin".to_string()),
				// rust-bitcoin
				("rust-bitcoin".to_string(), "rust-bitcoin".to_string()),
				("rust-bitcoin".to_string(), "rust-secp256k1".to_string()),
				("rust-bitcoin".to_string(), "rust-miniscript".to_string()),
				("rust-bitcoin".to_string(), "rust-bech32".to_string()),
				("rust-bitcoin".to_string(), "rust-bech32-bitcoin".to_string()),
				("rust-bitcoin".to_string(), "rust-psbt".to_string()),
				("rust-bitcoin".to_string(), "rust-psbt-v0".to_string()),
				("rust-bitcoin".to_string(), "corepc".to_string()),
				("rust-bitcoin".to_string(), "hex-conservative".to_string()),
				("rust-bitcoin".to_string(), "bip322".to_string()),
				("rust-bitcoin".to_string(), "bip324".to_string()),
				("rust-bitcoin".to_string(), "bitcoin-payment-instructions".to_string()),
				("rust-bitcoin".to_string(), "rust-bip39".to_string()),
				("rust-bitcoin".to_string(), "bitcoind".to_string()),
				("rust-bitcoin".to_string(), "rust-bitcoinconsensus".to_string()),
				("rust-bitcoin".to_string(), "constants".to_string()),
				// BDK
				("bitcoindevkit".to_string(), "bdk".to_string()),
				("bitcoindevkit".to_string(), "bdk-ffi".to_string()),
				("bitcoindevkit".to_string(), "bdk-cli".to_string()),
				("bitcoindevkit".to_string(), "bdk-kyoto".to_string()),
				("bitcoindevkit".to_string(), "bdk_wallet".to_string()),
				("bitcoindevkit".to_string(), "bdk-tx".to_string()),
				("bitcoindevkit".to_string(), "bdk-sp".to_string()),
				("bitcoindevkit".to_string(), "bdk-reserves".to_string()),
				("bitcoindevkit".to_string(), "bdk-sqlite".to_string()),
				("bitcoindevkit".to_string(), "bdk-sqlx".to_string()),
				("bitcoindevkit".to_string(), "bdk-bitcoind-client".to_string()),
				("bitcoindevkit".to_string(), "bdk-swift".to_string()),
				("bitcoindevkit".to_string(), "bdk-jvm".to_string()),
				("bitcoindevkit".to_string(), "bdk-python".to_string()),
				("bitcoindevkit".to_string(), "bdk-dart".to_string()),
				("bitcoindevkit".to_string(), "bdk-rn".to_string()),
				("bitcoindevkit".to_string(), "coin-select".to_string()),
				("bitcoindevkit".to_string(), "rust-esplora-client".to_string()),
				("bitcoindevkit".to_string(), "rust-electrum-client".to_string()),
				("bitcoindevkit".to_string(), "bitcoin-ffi".to_string()),
				("bitcoindevkit".to_string(), "rust-cktap".to_string()),
				("bitcoindevkit".to_string(), "electrum_streaming_client".to_string()),
				("bitcoindevkit".to_string(), "devkit-wallet".to_string()),
				// Payjoin
				("payjoin".to_string(), "rust-payjoin".to_string()),
				("payjoin".to_string(), "nolooking".to_string()),
				("payjoin".to_string(), "btsim".to_string()),
				("payjoin".to_string(), "cja".to_string()),
				("payjoin".to_string(), "cja-2".to_string()),
				("payjoin".to_string(), "multiparty-protocol-docs".to_string()),
				("payjoin".to_string(), "bitcoin-hpke".to_string()),
				("payjoin".to_string(), "tx-indexer".to_string()),
				("payjoin".to_string(), "receive-payjoin-v2".to_string()),
				("payjoin".to_string(), "batch-plot".to_string()),
				// Specs
				("bitcoin".to_string(), "bips".to_string()),
				("lightning".to_string(), "bolts".to_string()),
				("lightning".to_string(), "blips".to_string()),
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
