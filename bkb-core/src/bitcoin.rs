/// Bitcoin concept vocabulary, seeded from Optech topics.
///
/// Each concept has a slug (primary key), display name, category,
/// and a list of aliases/keywords that trigger matching.

/// A Bitcoin/Lightning concept definition.
pub struct ConceptDef {
	pub slug: &'static str,
	pub name: &'static str,
	pub category: &'static str,
	pub aliases: &'static [&'static str],
}

/// The curated concept vocabulary.
///
/// Sourced from Bitcoin Optech's topic index with additional terms.
/// This list covers the most commonly referenced Bitcoin and Lightning
/// concepts across the ecosystem.
pub const CONCEPTS: &[ConceptDef] = &[
	// -- Soft forks & consensus --
	ConceptDef {
		slug: "taproot",
		name: "Taproot",
		category: "soft-fork",
		aliases: &[
			"taproot", "bip-340", "bip-341", "bip-342", "bip340", "bip341", "bip342", "schnorr",
		],
	},
	ConceptDef {
		slug: "segwit",
		name: "Segregated Witness",
		category: "soft-fork",
		aliases: &["segwit", "segregated witness", "bip-141", "bip-143", "bip-144", "bip141"],
	},
	ConceptDef {
		slug: "op-cat",
		name: "OP_CAT",
		category: "soft-fork",
		aliases: &["op_cat", "op cat", "bip-347", "bip347"],
	},
	ConceptDef {
		slug: "op-checktemplateverify",
		name: "OP_CHECKTEMPLATEVERIFY",
		category: "soft-fork",
		aliases: &[
			"op_checktemplateverify",
			"op_ctv",
			"ctv",
			"bip-119",
			"bip119",
			"checktemplateverify",
		],
	},
	ConceptDef {
		slug: "op-checksigfromstack",
		name: "OP_CHECKSIGFROMSTACK",
		category: "soft-fork",
		aliases: &["op_checksigfromstack", "op_csfs", "csfs", "checksigfromstack"],
	},
	ConceptDef {
		slug: "covenants",
		name: "Covenants",
		category: "soft-fork",
		aliases: &["covenant", "covenants"],
	},
	// -- Transactions & scripting --
	ConceptDef {
		slug: "miniscript",
		name: "Miniscript",
		category: "scripting",
		aliases: &["miniscript", "mini script"],
	},
	ConceptDef {
		slug: "descriptors",
		name: "Output Script Descriptors",
		category: "scripting",
		aliases: &["descriptor", "descriptors", "output descriptor", "output descriptors"],
	},
	ConceptDef {
		slug: "psbt",
		name: "Partially Signed Bitcoin Transactions",
		category: "transaction",
		aliases: &["psbt", "bip-174", "bip174", "partially signed"],
	},
	ConceptDef {
		slug: "rbf",
		name: "Replace-By-Fee",
		category: "transaction",
		aliases: &["replace-by-fee", "replace by fee", "rbf", "bip-125", "bip125"],
	},
	ConceptDef {
		slug: "cpfp",
		name: "Child Pays for Parent",
		category: "transaction",
		aliases: &["cpfp", "child pays for parent", "child-pays-for-parent"],
	},
	ConceptDef {
		slug: "package-relay",
		name: "Package Relay",
		category: "transaction",
		aliases: &["package relay", "package-relay"],
	},
	ConceptDef {
		slug: "cluster-mempool",
		name: "Cluster Mempool",
		category: "mempool",
		aliases: &["cluster mempool", "cluster-mempool"],
	},
	ConceptDef {
		slug: "v3-transactions",
		name: "v3 Transactions",
		category: "transaction",
		aliases: &[
			"v3 transaction",
			"v3 transactions",
			"topologically restricted until confirmation",
			"truc",
		],
	},
	ConceptDef {
		slug: "ephemeral-anchors",
		name: "Ephemeral Anchors",
		category: "transaction",
		aliases: &["ephemeral anchor", "ephemeral anchors", "ephemeral-anchors"],
	},
	// -- Lightning --
	ConceptDef {
		slug: "lightning",
		name: "Lightning Network",
		category: "lightning",
		aliases: &["lightning network", "lightning"],
	},
	ConceptDef {
		slug: "htlc",
		name: "Hash Time-Locked Contract",
		category: "lightning",
		aliases: &["htlc", "hash time-locked contract", "hash time locked contract"],
	},
	ConceptDef {
		slug: "ptlc",
		name: "Point Time-Locked Contract",
		category: "lightning",
		aliases: &["ptlc", "point time-locked contract"],
	},
	ConceptDef {
		slug: "channel-splicing",
		name: "Channel Splicing",
		category: "lightning",
		aliases: &["splicing", "splice", "splice-in", "splice-out", "channel splicing"],
	},
	ConceptDef {
		slug: "anchor-outputs",
		name: "Anchor Outputs",
		category: "lightning",
		aliases: &["anchor output", "anchor outputs", "anchor channels", "anchor-outputs"],
	},
	ConceptDef {
		slug: "bolt11",
		name: "BOLT11 Invoices",
		category: "lightning",
		aliases: &["bolt11", "bolt-11", "bolt 11", "lightning invoice"],
	},
	ConceptDef {
		slug: "bolt12",
		name: "BOLT12 Offers",
		category: "lightning",
		aliases: &["bolt12", "bolt-12", "bolt 12", "offers", "lightning offers"],
	},
	ConceptDef {
		slug: "onion-messages",
		name: "Onion Messages",
		category: "lightning",
		aliases: &["onion message", "onion messages", "onion-messages"],
	},
	ConceptDef {
		slug: "blinded-paths",
		name: "Blinded Paths",
		category: "lightning",
		aliases: &["blinded path", "blinded paths", "blinded-paths", "route blinding"],
	},
	ConceptDef {
		slug: "dual-funding",
		name: "Dual Funding",
		category: "lightning",
		aliases: &["dual funding", "dual-funding", "interactive-tx"],
	},
	ConceptDef {
		slug: "async-payments",
		name: "Async Payments",
		category: "lightning",
		aliases: &["async payment", "async payments", "asynchronous payment", "async-payments"],
	},
	ConceptDef {
		slug: "trampoline-routing",
		name: "Trampoline Routing",
		category: "lightning",
		aliases: &["trampoline", "trampoline routing", "trampoline-routing"],
	},
	ConceptDef {
		slug: "channel-jamming",
		name: "Channel Jamming",
		category: "lightning",
		aliases: &["channel jamming", "channel-jamming", "jamming"],
	},
	// -- Privacy --
	ConceptDef {
		slug: "payjoin",
		name: "Payjoin",
		category: "privacy",
		aliases: &["payjoin", "pay-join", "bip-78", "bip78", "p2ep"],
	},
	ConceptDef {
		slug: "silent-payments",
		name: "Silent Payments",
		category: "privacy",
		aliases: &["silent payment", "silent payments", "bip-352", "bip352"],
	},
	ConceptDef {
		slug: "coinswap",
		name: "Coinswap",
		category: "privacy",
		aliases: &["coinswap", "coin swap"],
	},
	// -- Wallet & key management --
	ConceptDef {
		slug: "musig2",
		name: "MuSig2",
		category: "cryptography",
		aliases: &["musig", "musig2", "multi-signature", "multisig"],
	},
	ConceptDef {
		slug: "frost",
		name: "FROST",
		category: "cryptography",
		aliases: &["frost", "flexible round-optimized schnorr threshold"],
	},
	ConceptDef {
		slug: "bip32",
		name: "HD Wallets",
		category: "wallet",
		aliases: &["bip32", "bip-32", "hierarchical deterministic", "hd wallet"],
	},
	// -- P2P & network --
	ConceptDef { slug: "erlay", name: "Erlay", category: "p2p", aliases: &["erlay"] },
	ConceptDef {
		slug: "compact-block-filters",
		name: "Compact Block Filters",
		category: "p2p",
		aliases: &[
			"compact block filter",
			"compact block filters",
			"bip-157",
			"bip-158",
			"bip157",
			"bip158",
			"neutrino",
		],
	},
	ConceptDef {
		slug: "assumeutxo",
		name: "AssumeUTXO",
		category: "validation",
		aliases: &["assumeutxo", "assume utxo", "assume-utxo"],
	},
	// -- Additional Lightning --
	ConceptDef {
		slug: "keysend",
		name: "Keysend",
		category: "lightning",
		aliases: &["keysend", "spontaneous payment", "spontaneous payments"],
	},
	ConceptDef {
		slug: "watchtower",
		name: "Watchtowers",
		category: "lightning",
		aliases: &["watchtower", "watchtowers", "breach remedy"],
	},
	ConceptDef {
		slug: "eltoo",
		name: "Eltoo",
		category: "lightning",
		aliases: &["eltoo", "sighash_anyprevout", "anyprevout", "bip-118", "bip118"],
	},
	ConceptDef {
		slug: "lsp",
		name: "Lightning Service Provider",
		category: "lightning",
		aliases: &["lsp", "lightning service provider", "lsps"],
	},
	ConceptDef {
		slug: "channel-factory",
		name: "Channel Factories",
		category: "lightning",
		aliases: &["channel factory", "channel factories", "joinpool", "joinpools"],
	},
	// -- Additional scripting --
	ConceptDef {
		slug: "timelock",
		name: "Timelocks",
		category: "scripting",
		aliases: &[
			"timelock",
			"timelocks",
			"cltv",
			"csv",
			"bip-65",
			"bip65",
			"bip-112",
			"bip112",
			"checklocktimeverify",
			"checksequenceverify",
		],
	},
	ConceptDef {
		slug: "vaults",
		name: "Vaults",
		category: "scripting",
		aliases: &["vault", "vaults", "bitcoin vault", "bitcoin vaults"],
	},
	// -- Protocols --
	ConceptDef {
		slug: "lnurl",
		name: "LNURL",
		category: "protocols",
		aliases: &["lnurl", "lnurl-pay", "lnurl-withdraw", "lnurl-auth", "lnurl-channel"],
	},
	ConceptDef {
		slug: "ecash",
		name: "Ecash",
		category: "protocols",
		aliases: &["ecash", "e-cash", "cashu", "chaumian"],
	},
	ConceptDef {
		slug: "dlc",
		name: "Discreet Log Contracts",
		category: "protocols",
		aliases: &["dlc", "discreet log contract", "discreet log contracts"],
	},
	ConceptDef {
		slug: "submarine-swap",
		name: "Submarine Swaps",
		category: "protocols",
		aliases: &["submarine swap", "submarine swaps", "atomic swap", "atomic swaps"],
	},
	ConceptDef { slug: "nostr", name: "Nostr", category: "protocols", aliases: &["nostr", "nip"] },
	// -- Additional wallet --
	ConceptDef {
		slug: "coin-selection",
		name: "Coin Selection",
		category: "wallet",
		aliases: &["coin selection", "coin-selection", "utxo selection"],
	},
	ConceptDef {
		slug: "codex32",
		name: "Codex32",
		category: "wallet",
		aliases: &["codex32", "bip-93", "bip93"],
	},
	// -- Additional cryptography --
	ConceptDef {
		slug: "threshold-signature",
		name: "Threshold Signatures",
		category: "cryptography",
		aliases: &["threshold signature", "threshold signatures", "tss"],
	},
];
