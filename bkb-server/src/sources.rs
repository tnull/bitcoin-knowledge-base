use axum::response::Html;

/// Handler for `GET /sources` -- serves the data sources overview page.
pub async fn sources_page() -> Html<&'static str> {
	Html(SOURCES_HTML)
}

const SOURCES_HTML: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Data Sources &ndash; Bitcoin Knowledge Base</title>
<style>
:root {
	--bg: #fafafa; --fg: #1a1a1a; --muted: #666; --border: #ddd;
	--card-bg: #fff; --accent: #f7931a; --accent2: #4a90d9;
	--badge-bg: #eee; --code-bg: #f0f0f0;
}
@media (prefers-color-scheme: dark) {
	:root {
		--bg: #1a1a2e; --fg: #e0e0e0; --muted: #999; --border: #333;
		--card-bg: #16213e; --accent: #f7931a; --accent2: #6db3f2;
		--badge-bg: #2a2a4a; --code-bg: #16213e;
	}
}
* { box-sizing: border-box; margin: 0; padding: 0; }
body {
	font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
	background: var(--bg); color: var(--fg); line-height: 1.6;
	max-width: 900px; margin: 0 auto; padding: 2rem 1rem;
}
.logo { margin-bottom: 0.25rem; }
.logo a { text-decoration: none; }
.logo img { display: block; max-width: 100%; height: auto; max-height: 110px; }
h1 { font-size: 1.5rem; margin-bottom: 0.25rem; }
.subtitle { color: var(--muted); margin-bottom: 1.5rem; }
.links { margin-bottom: 2rem; }
.links a {
	color: var(--accent2); text-decoration: none; margin-right: 1.5rem;
	font-size: 0.9rem;
}
.links a:hover { text-decoration: underline; }
.section { margin-bottom: 2.5rem; }
.section h2 {
	font-size: 1.15rem; margin-bottom: 0.75rem; color: var(--accent2);
	border-bottom: 1px solid var(--border); padding-bottom: 0.3rem;
}
.source-card {
	border: 1px solid var(--border); border-radius: 8px; padding: 1rem;
	margin-bottom: 1rem; background: var(--card-bg);
}
.source-card h3 {
	font-size: 1rem; margin-bottom: 0.3rem;
}
.source-card h3 a { color: var(--accent2); text-decoration: none; }
.source-card h3 a:hover { text-decoration: underline; }
.source-card .description {
	font-size: 0.9rem; margin-bottom: 0.5rem;
}
.source-card .meta {
	font-size: 0.8rem; color: var(--muted);
}
.source-card .meta code {
	background: var(--code-bg); padding: 0.1rem 0.35rem; border-radius: 3px;
	font-size: 0.8rem;
}
.repo-list {
	display: flex; flex-wrap: wrap; gap: 0.35rem; margin-top: 0.5rem;
}
.repo-tag {
	display: inline-block; padding: 0.1rem 0.45rem; border-radius: 3px;
	background: var(--badge-bg); font-size: 0.75rem; font-family: monospace;
	color: var(--fg);
}
.repo-tag a { color: var(--accent2); text-decoration: none; }
.repo-tag a:hover { text-decoration: underline; }
.source-type-tag {
	display: inline-block; padding: 0.1rem 0.4rem; border-radius: 3px;
	background: var(--accent); color: #fff; font-size: 0.7rem;
	font-family: monospace; margin-right: 0.25rem;
}
footer {
	margin-top: 2rem; padding-top: 1rem; border-top: 1px solid var(--border);
	font-size: 0.8rem; color: var(--muted); text-align: center;
}
</style>
</head>
<body>
<a href="/" style="text-decoration:none;color:inherit"><div class="logo"><img src="/logo.png" alt="BKB"></div></a>
<h1>Data Sources</h1>
<p class="subtitle">All sources indexed by the Bitcoin Knowledge Base, with document types, repositories, and search filter keys.</p>

<div class="links">
	<a href="/">&larr; Home</a>
	<a href="https://github.com/tnull/bitcoin-knowledge-base">GitHub</a>
	<a href="/examples">Example Prompts</a>
</div>

<!-- GitHub -->
<div class="section">
<h2>GitHub Repositories</h2>

<div class="source-card">
<h3>Issues &amp; Pull Requests</h3>
<div class="description">All issues and PRs (including title, body, labels, and state) from tracked repositories. Updated via the GitHub REST API with incremental sync based on <code>updated_at</code> timestamps.</div>
<div class="meta">
	Filter keys: <span class="source-type-tag">github_issue</span><span class="source-type-tag">github_pr</span>
</div>
</div>

<div class="source-card">
<h3>Comments &amp; Reviews</h3>
<div class="description">All issue comments, PR review comments, and review bodies from tracked repositories. Linked back to their parent issue or PR via cross-references.</div>
<div class="meta">
	Filter keys: <span class="source-type-tag">github_comment</span><span class="source-type-tag">github_review</span><span class="source-type-tag">github_review_comment</span>
</div>
</div>

<div class="source-card">
<h3>Discussions</h3>
<div class="description">GitHub Discussions and their comments from repositories that have the feature enabled.</div>
<div class="meta">
	Filter keys: <span class="source-type-tag">github_discussion</span><span class="source-type-tag">github_discussion_comment</span>
</div>
</div>

<div class="source-card">
<h3>Git Commits</h3>
<div class="description">Commit messages and metadata from all tracked repositories, ingested via cached bare clones. Useful for tracing code changes and finding the PR that introduced a feature or fix.</div>
<div class="meta">
	Filter key: <span class="source-type-tag">commit</span>
</div>
</div>

<div class="source-card">
<h3>Tracked Repositories</h3>
<div class="description">The full set of GitHub repositories indexed by BKB:</div>
<div class="repo-list">
<span class="repo-tag"><a href="https://github.com/bitcoin/bitcoin">bitcoin/bitcoin</a></span>
<span class="repo-tag"><a href="https://github.com/bitcoin/bips">bitcoin/bips</a></span>
<span class="repo-tag"><a href="https://github.com/lightning/bolts">lightning/bolts</a></span>
<span class="repo-tag"><a href="https://github.com/lightning/blips">lightning/blips</a></span>
<span class="repo-tag"><a href="https://github.com/lightningdevkit/rust-lightning">lightningdevkit/rust-lightning</a></span>
<span class="repo-tag"><a href="https://github.com/lightningdevkit/ldk-node">lightningdevkit/ldk-node</a></span>
<span class="repo-tag"><a href="https://github.com/lightningdevkit/ldk-sample">lightningdevkit/ldk-sample</a></span>
<span class="repo-tag"><a href="https://github.com/lightningdevkit/ldk-server">lightningdevkit/ldk-server</a></span>
<span class="repo-tag"><a href="https://github.com/lightningdevkit/ldk-c-bindings">lightningdevkit/ldk-c-bindings</a></span>
<span class="repo-tag"><a href="https://github.com/lightningdevkit/ldk-garbagecollected">lightningdevkit/ldk-garbagecollected</a></span>
<span class="repo-tag"><a href="https://github.com/lightningdevkit/vss-server">lightningdevkit/vss-server</a></span>
<span class="repo-tag"><a href="https://github.com/lightningdevkit/vss-client">lightningdevkit/vss-client</a></span>
<span class="repo-tag"><a href="https://github.com/lightningdevkit/rapid-gossip-sync-server">lightningdevkit/rapid-gossip-sync-server</a></span>
<span class="repo-tag"><a href="https://github.com/lightningdevkit/ldk-swift">lightningdevkit/ldk-swift</a></span>
<span class="repo-tag"><a href="https://github.com/lightningdevkit/ldk-review-club">lightningdevkit/ldk-review-club</a></span>
<span class="repo-tag"><a href="https://github.com/lightningdevkit/orange-sdk">lightningdevkit/orange-sdk</a></span>
<span class="repo-tag"><a href="https://github.com/rust-bitcoin/rust-bitcoin">rust-bitcoin/rust-bitcoin</a></span>
<span class="repo-tag"><a href="https://github.com/rust-bitcoin/rust-secp256k1">rust-bitcoin/rust-secp256k1</a></span>
<span class="repo-tag"><a href="https://github.com/rust-bitcoin/rust-miniscript">rust-bitcoin/rust-miniscript</a></span>
<span class="repo-tag"><a href="https://github.com/rust-bitcoin/rust-bech32">rust-bitcoin/rust-bech32</a></span>
<span class="repo-tag"><a href="https://github.com/rust-bitcoin/rust-bech32-bitcoin">rust-bitcoin/rust-bech32-bitcoin</a></span>
<span class="repo-tag"><a href="https://github.com/rust-bitcoin/rust-psbt">rust-bitcoin/rust-psbt</a></span>
<span class="repo-tag"><a href="https://github.com/rust-bitcoin/rust-psbt-v0">rust-bitcoin/rust-psbt-v0</a></span>
<span class="repo-tag"><a href="https://github.com/rust-bitcoin/corepc">rust-bitcoin/corepc</a></span>
<span class="repo-tag"><a href="https://github.com/rust-bitcoin/hex-conservative">rust-bitcoin/hex-conservative</a></span>
<span class="repo-tag"><a href="https://github.com/rust-bitcoin/bip322">rust-bitcoin/bip322</a></span>
<span class="repo-tag"><a href="https://github.com/rust-bitcoin/bip324">rust-bitcoin/bip324</a></span>
<span class="repo-tag"><a href="https://github.com/rust-bitcoin/bitcoin-payment-instructions">rust-bitcoin/bitcoin-payment-instructions</a></span>
<span class="repo-tag"><a href="https://github.com/rust-bitcoin/rust-bip39">rust-bitcoin/rust-bip39</a></span>
<span class="repo-tag"><a href="https://github.com/rust-bitcoin/bitcoind">rust-bitcoin/bitcoind</a></span>
<span class="repo-tag"><a href="https://github.com/rust-bitcoin/rust-bitcoinconsensus">rust-bitcoin/rust-bitcoinconsensus</a></span>
<span class="repo-tag"><a href="https://github.com/rust-bitcoin/constants">rust-bitcoin/constants</a></span>
<span class="repo-tag"><a href="https://github.com/bitcoindevkit/bdk">bitcoindevkit/bdk</a></span>
<span class="repo-tag"><a href="https://github.com/bitcoindevkit/bdk-ffi">bitcoindevkit/bdk-ffi</a></span>
<span class="repo-tag"><a href="https://github.com/bitcoindevkit/bdk-cli">bitcoindevkit/bdk-cli</a></span>
<span class="repo-tag"><a href="https://github.com/bitcoindevkit/bdk-kyoto">bitcoindevkit/bdk-kyoto</a></span>
<span class="repo-tag"><a href="https://github.com/bitcoindevkit/bdk_wallet">bitcoindevkit/bdk_wallet</a></span>
<span class="repo-tag"><a href="https://github.com/bitcoindevkit/bdk-tx">bitcoindevkit/bdk-tx</a></span>
<span class="repo-tag"><a href="https://github.com/bitcoindevkit/bdk-sp">bitcoindevkit/bdk-sp</a></span>
<span class="repo-tag"><a href="https://github.com/bitcoindevkit/bdk-reserves">bitcoindevkit/bdk-reserves</a></span>
<span class="repo-tag"><a href="https://github.com/bitcoindevkit/bdk-sqlite">bitcoindevkit/bdk-sqlite</a></span>
<span class="repo-tag"><a href="https://github.com/bitcoindevkit/bdk-sqlx">bitcoindevkit/bdk-sqlx</a></span>
<span class="repo-tag"><a href="https://github.com/bitcoindevkit/bdk-bitcoind-client">bitcoindevkit/bdk-bitcoind-client</a></span>
<span class="repo-tag"><a href="https://github.com/bitcoindevkit/bdk-swift">bitcoindevkit/bdk-swift</a></span>
<span class="repo-tag"><a href="https://github.com/bitcoindevkit/bdk-jvm">bitcoindevkit/bdk-jvm</a></span>
<span class="repo-tag"><a href="https://github.com/bitcoindevkit/bdk-python">bitcoindevkit/bdk-python</a></span>
<span class="repo-tag"><a href="https://github.com/bitcoindevkit/bdk-dart">bitcoindevkit/bdk-dart</a></span>
<span class="repo-tag"><a href="https://github.com/bitcoindevkit/bdk-rn">bitcoindevkit/bdk-rn</a></span>
<span class="repo-tag"><a href="https://github.com/bitcoindevkit/coin-select">bitcoindevkit/coin-select</a></span>
<span class="repo-tag"><a href="https://github.com/bitcoindevkit/rust-esplora-client">bitcoindevkit/rust-esplora-client</a></span>
<span class="repo-tag"><a href="https://github.com/bitcoindevkit/rust-electrum-client">bitcoindevkit/rust-electrum-client</a></span>
<span class="repo-tag"><a href="https://github.com/bitcoindevkit/bitcoin-ffi">bitcoindevkit/bitcoin-ffi</a></span>
<span class="repo-tag"><a href="https://github.com/bitcoindevkit/rust-cktap">bitcoindevkit/rust-cktap</a></span>
<span class="repo-tag"><a href="https://github.com/bitcoindevkit/electrum_streaming_client">bitcoindevkit/electrum_streaming_client</a></span>
<span class="repo-tag"><a href="https://github.com/bitcoindevkit/devkit-wallet">bitcoindevkit/devkit-wallet</a></span>
<span class="repo-tag"><a href="https://github.com/payjoin/rust-payjoin">payjoin/rust-payjoin</a></span>
<span class="repo-tag"><a href="https://github.com/payjoin/nolooking">payjoin/nolooking</a></span>
<span class="repo-tag"><a href="https://github.com/payjoin/btsim">payjoin/btsim</a></span>
<span class="repo-tag"><a href="https://github.com/payjoin/cja">payjoin/cja</a></span>
<span class="repo-tag"><a href="https://github.com/payjoin/cja-2">payjoin/cja-2</a></span>
<span class="repo-tag"><a href="https://github.com/payjoin/multiparty-protocol-docs">payjoin/multiparty-protocol-docs</a></span>
<span class="repo-tag"><a href="https://github.com/payjoin/bitcoin-hpke">payjoin/bitcoin-hpke</a></span>
<span class="repo-tag"><a href="https://github.com/payjoin/tx-indexer">payjoin/tx-indexer</a></span>
<span class="repo-tag"><a href="https://github.com/payjoin/receive-payjoin-v2">payjoin/receive-payjoin-v2</a></span>
<span class="repo-tag"><a href="https://github.com/payjoin/batch-plot">payjoin/batch-plot</a></span>
<span class="repo-tag"><a href="https://github.com/lightningnetwork/lnd">lightningnetwork/lnd</a></span>
<span class="repo-tag"><a href="https://github.com/ElementsProject/lightning">ElementsProject/lightning</a></span>
<span class="repo-tag"><a href="https://github.com/ACINQ/eclair">ACINQ/eclair</a></span>
<span class="repo-tag"><a href="https://github.com/lnurl/luds">lnurl/luds</a></span>
<span class="repo-tag"><a href="https://github.com/cashubtc/nuts">cashubtc/nuts</a></span>
<span class="repo-tag"><a href="https://github.com/bitcoinops/bitcoinops.github.io">bitcoinops/bitcoinops.github.io</a></span>
</div>
</div>

</div>

<!-- Specifications -->
<div class="section">
<h2>Specifications</h2>

<div class="source-card">
<h3><a href="https://github.com/bitcoin/bips">BIPs (Bitcoin Improvement Proposals)</a></h3>
<div class="description">The full set of BIPs from the <code>bitcoin/bips</code> repository. Each BIP is stored as a separate document with its full text, parsed from the mediawiki/markdown source files. Supports direct lookup by number (e.g., BIP-340) and cross-reference extraction (e.g., mentions of <code>BIP-NNN</code> in other documents).</div>
<div class="meta">
	Filter key: <span class="source-type-tag">bip</span> &middot;
	Lookup: <code>/bip/{number}</code> &middot;
	MCP tool: <code>bkb_lookup_bip</code>
</div>
</div>

<div class="source-card">
<h3><a href="https://github.com/lightning/bolts">BOLTs (Basis of Lightning Technology)</a></h3>
<div class="description">The Lightning Network protocol specifications from <code>lightning/bolts</code>. Each BOLT is indexed as a document with full text. Cross-references to BOLTs found in other documents are extracted automatically.</div>
<div class="meta">
	Filter key: <span class="source-type-tag">bolt</span> &middot;
	Lookup: <code>/bolt/{number}</code> &middot;
	MCP tool: <code>bkb_lookup_bolt</code>
</div>
</div>

<div class="source-card">
<h3><a href="https://github.com/lightning/blips">bLIPs (Bitcoin Lightning Improvement Proposals)</a></h3>
<div class="description">Optional Lightning extensions from <code>lightning/blips</code>. These are community-driven proposals that build on top of the BOLTs. Supports direct lookup and cross-referencing like BIPs and BOLTs.</div>
<div class="meta">
	Filter key: <span class="source-type-tag">blip</span> &middot;
	Lookup: <code>/blip/{number}</code> &middot;
	MCP tool: <code>bkb_lookup_blip</code>
</div>
</div>

<div class="source-card">
<h3><a href="https://github.com/lnurl/luds">LUDs (LNURL Documents)</a></h3>
<div class="description">LNURL protocol specifications from <code>lnurl/luds</code>. LUDs define a set of HTTP-based protocols for interacting with Lightning wallets and services, including LNURL-pay, LNURL-withdraw, LNURL-auth, and more. Supports direct lookup by number and cross-reference extraction (e.g., mentions of <code>LUD-NN</code> in other documents).</div>
<div class="meta">
	Filter key: <span class="source-type-tag">lud</span> &middot;
	Lookup: <code>/lud/{number}</code> &middot;
	MCP tool: <code>bkb_lookup_lud</code>
</div>
</div>

<div class="source-card">
<h3><a href="https://github.com/cashubtc/nuts">NUTs (Notation, Usage, and Terminology)</a></h3>
<div class="description">Cashu protocol specifications from <code>cashubtc/nuts</code>. NUTs define the Cashu ecash protocol for Bitcoin, including minting, melting, token serialization, and Lightning integration. Supports direct lookup by number and cross-reference extraction (e.g., mentions of <code>NUT-NN</code> in other documents).</div>
<div class="meta">
	Filter key: <span class="source-type-tag">nut</span> &middot;
	Lookup: <code>/nut/{number}</code> &middot;
	MCP tool: <code>bkb_lookup_nut</code>
</div>
</div>

</div>

<!-- Mailing Lists -->
<div class="section">
<h2>Mailing Lists</h2>

<div class="source-card">
<h3><a href="https://groups.google.com/g/bitcoindev">bitcoin-dev Mailing List</a></h3>
<div class="description">Messages from the bitcoin-dev (formerly bitcoindev) mailing list, the primary venue for Bitcoin protocol discussion. Ingested from the Google Groups archive. Covers proposals, protocol changes, soft fork discussions, and more.</div>
<div class="meta">
	Filter key: <span class="source-type-tag">mailing_list_msg</span> &middot;
	Filter by source: <code>source_repo</code> not applicable (use author/date filters)
</div>
</div>

<div class="source-card">
<h3><a href="https://lists.linuxfoundation.org/pipermail/lightning-dev/">lightning-dev Mailing List</a></h3>
<div class="description">The archived lightning-dev mailing list from the Linux Foundation. Contains Lightning Network protocol discussions, proposals, and design debates. Ingested from the mail-archive.com mirror.</div>
<div class="meta">
	Filter key: <span class="source-type-tag">mailing_list_msg</span>
</div>
</div>

</div>

<!-- Delving Bitcoin -->
<div class="section">
<h2>Discussion Forums</h2>

<div class="source-card">
<h3><a href="https://bitcointalk.org">BitcoinTalk</a></h3>
<div class="description">Topics and posts from the technically relevant boards of BitcoinTalk, the oldest and largest Bitcoin discussion forum. Includes Bitcoin Discussion, Development &amp; Technical Discussion, Bitcoin Technical Support, Project Development, Mining, and Economics. Contains historically significant discussions including Satoshi's original posts.</div>
<div class="meta">
	Filter keys: <span class="source-type-tag">bitcointalk_topic</span><span class="source-type-tag">bitcointalk_post</span>
</div>
<div class="repo-list" style="margin-top:0.5rem">
	<span class="repo-tag">Board 1: Bitcoin Discussion</span>
	<span class="repo-tag">Board 6: Dev &amp; Technical</span>
	<span class="repo-tag">Board 4: Technical Support</span>
	<span class="repo-tag">Board 12: Project Development</span>
	<span class="repo-tag">Board 14: Mining</span>
	<span class="repo-tag">Board 7: Economics</span>
</div>
</div>

<div class="source-card">
<h3><a href="https://delvingbitcoin.org">Delving Bitcoin</a></h3>
<div class="description">Topics and posts from delvingbitcoin.org, the Discourse-based forum for technical Bitcoin discussion. Topics are indexed as top-level documents; individual posts are stored separately with linkage to their parent topic. Covers covenant proposals, mempool policy, protocol research, and implementation discussions.</div>
<div class="meta">
	Filter keys: <span class="source-type-tag">delving_topic</span><span class="source-type-tag">delving_post</span>
</div>
</div>

</div>

<!-- IRC -->
<div class="section">
<h2>IRC Logs</h2>

<div class="source-card">
<h3>IRC Channel Logs</h3>
<div class="description">Daily logs from Bitcoin and Lightning IRC channels on Libera.Chat. Each day's log is stored as a single document. Useful for finding real-time developer discussions, meeting notes, and informal protocol debates.</div>
<div class="meta">
	Filter key: <span class="source-type-tag">irc_log</span>
</div>
<div class="repo-list" style="margin-top:0.5rem">
	<span class="repo-tag">#bitcoin-core-dev</span>
	<span class="repo-tag">#lightning-dev</span>
	<span class="repo-tag">#bitcoin-wizards</span>
</div>
</div>

</div>

<!-- Optech -->
<div class="section">
<h2>Bitcoin Optech</h2>

<div class="source-card">
<h3><a href="https://bitcoinops.org/en/newsletters/">Optech Newsletters</a></h3>
<div class="description">Weekly newsletters from Bitcoin Optech that summarize notable developments across the Bitcoin ecosystem. Each newsletter is indexed as a full document with concept tagging. Optech coverage often links to BIPs, BOLTs, PRs, and mailing list threads &mdash; these cross-references are extracted automatically.</div>
<div class="meta">
	Filter key: <span class="source-type-tag">optech_newsletter</span>
</div>
</div>

<div class="source-card">
<h3><a href="https://bitcoinops.org/en/topics/">Optech Topics</a></h3>
<div class="description">Optech's curated topic pages, each providing a summary and link collection for a specific Bitcoin/Lightning concept (e.g., Taproot, channel jamming, PTLCs). These also feed the concept vocabulary used for automatic tagging across all sources.</div>
<div class="meta">
	Filter key: <span class="source-type-tag">optech_topic</span>
</div>
</div>

<div class="source-card">
<h3><a href="https://bitcoinops.org/en/blog/">Optech Blog Posts</a></h3>
<div class="description">Longer-form Optech blog posts, field reports, and special series (e.g., "Preparing for Taproot", compatibility matrices).</div>
<div class="meta">
	Filter key: <span class="source-type-tag">optech_blog</span>
</div>
</div>

</div>

<footer>Bitcoin Knowledge Base &mdash; <a href="/" style="color:var(--accent2)">Home</a></footer>
</body>
</html>
"##;
