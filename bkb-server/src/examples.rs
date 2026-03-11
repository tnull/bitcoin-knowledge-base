use axum::response::Html;

/// Handler for `GET /examples` -- serves the example prompts page.
pub async fn examples_page() -> Html<&'static str> {
	Html(EXAMPLES_HTML)
}

const EXAMPLES_HTML: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Example Prompts &ndash; Bitcoin Knowledge Base</title>
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
.logo img { height: 110px; }
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
.prompt-card {
	border: 1px solid var(--border); border-radius: 8px; padding: 1rem;
	margin-bottom: 1rem; background: var(--card-bg);
}
.prompt-card .prompt {
	font-family: monospace; font-size: 0.95rem; padding: 0.75rem;
	background: var(--code-bg); border-radius: 6px; margin-bottom: 0.5rem;
	white-space: pre-wrap; line-height: 1.5;
}
.prompt-card .description {
	font-size: 0.85rem; color: var(--muted);
}
.tools-used {
	display: flex; flex-wrap: wrap; gap: 0.3rem; margin-top: 0.4rem;
}
.tool-tag {
	display: inline-block; padding: 0.1rem 0.4rem; border-radius: 3px;
	background: var(--accent); color: #fff; font-size: 0.7rem;
	font-family: monospace;
}
.note {
	font-size: 0.85rem; color: var(--muted); font-style: italic;
	margin-bottom: 1.5rem;
}
footer {
	margin-top: 2rem; padding-top: 1rem; border-top: 1px solid var(--border);
	font-size: 0.8rem; color: var(--muted); text-align: center;
}
</style>
</head>
<body>
<a href="/" style="text-decoration:none;color:inherit"><div class="logo"><img src="/logo.png" alt="BKB"></div></a>
<h1>Example Agent Prompts</h1>
<p class="subtitle">Copy any of these into your AI agent to see what BKB can do.</p>

<div class="links">
	<a href="/">&larr; Home</a>
	<a href="https://github.com/tnull/bitcoin-knowledge-base">GitHub</a>
	<a href="https://github.com/tnull/bitcoin-knowledge-base/blob/master/SKILL.md">Agent Setup (SKILL.md)</a>
</div>

<p class="note">These prompts work with any MCP-capable AI agent (e.g., Claude Code, Claude Desktop) that has the <code>bkb-mcp</code> server configured. The agent will automatically call the right BKB tools to answer the question.</p>

<div class="section">
<h2>Understanding Specifications</h2>

<div class="prompt-card">
<div class="prompt">Explain BIP-340 (Schnorr signatures) to me. What problem does it solve, what discussions led to its design, and which implementations reference it?</div>
<div class="description">Looks up the BIP spec, gathers cross-references from GitHub PRs, mailing list threads, and Optech coverage to build a complete picture.</div>
<div class="tools-used"><span class="tool-tag">bkb_lookup_bip</span><span class="tool-tag">bkb_get_references</span></div>
</div>

<div class="prompt-card">
<div class="prompt">Compare BOLT-11 and BOLT-12 invoices. What are the key differences, and what was the motivation for BOLT 12?</div>
<div class="description">Retrieves both BOLT specs and searches for related discussions to synthesize a comparison.</div>
<div class="tools-used"><span class="tool-tag">bkb_lookup_bolt</span><span class="tool-tag">bkb_search</span></div>
</div>

<div class="prompt-card">
<div class="prompt">What is bLIP-39? Summarize the proposal and show me any PRs that implement it.</div>
<div class="description">Fetches the bLIP spec text and finds referencing implementation PRs across Lightning repos.</div>
<div class="tools-used"><span class="tool-tag">bkb_lookup_blip</span><span class="tool-tag">bkb_get_references</span></div>
</div>
</div>

<div class="section">
<h2>Researching a Concept End-to-End</h2>

<div class="prompt-card">
<div class="prompt">Give me a timeline of how Taproot went from proposal to activation. Include mailing list posts, BIPs, key PRs, and Optech coverage.</div>
<div class="description">Uses the timeline tool to trace the full chronological history of a concept across all sources.</div>
<div class="tools-used"><span class="tool-tag">bkb_timeline</span><span class="tool-tag">bkb_lookup_bip</span><span class="tool-tag">bkb_get_document</span></div>
</div>

<div class="prompt-card">
<div class="prompt">What is package relay, why does Bitcoin Core need it, and what's the current status? Show me the most important discussions and PRs.</div>
<div class="description">Combines timeline, search, and document retrieval to produce a research summary with primary sources.</div>
<div class="tools-used"><span class="tool-tag">bkb_timeline</span><span class="tool-tag">bkb_search</span><span class="tool-tag">bkb_get_document</span></div>
</div>

<div class="prompt-card">
<div class="prompt">Trace the history of channel jamming mitigations in the Lightning Network. What approaches have been proposed on the mailing list and Delving Bitcoin?</div>
<div class="description">Searches across mailing lists and Delving Bitcoin filtered by topic, then follows up on key threads.</div>
<div class="tools-used"><span class="tool-tag">bkb_timeline</span><span class="tool-tag">bkb_search</span></div>
</div>
</div>

<div class="section">
<h2>Investigating Code Changes</h2>

<div class="prompt-card">
<div class="prompt">Find the PR that added MuSig2 support to rust-bitcoin and summarize the discussion around it.</div>
<div class="description">Searches for commits/PRs matching the description in a specific repo, then retrieves the full document with comments.</div>
<div class="tools-used"><span class="tool-tag">bkb_find_commit</span><span class="tool-tag">bkb_get_document</span></div>
</div>

<div class="prompt-card">
<div class="prompt">What changes were made to LDK's channel management code in Q4 2024? List the major PRs.</div>
<div class="description">Uses wildcard search with repo and date filters to find all matching PRs in a time window.</div>
<div class="tools-used"><span class="tool-tag">bkb_search</span></div>
</div>

<div class="prompt-card">
<div class="prompt">Show me all the PRs that reference or fix issue bitcoin/bitcoin#25038.</div>
<div class="description">Finds all documents that cross-reference a specific issue, including &ldquo;Fixes&rdquo; and &ldquo;Closes&rdquo; links.</div>
<div class="tools-used"><span class="tool-tag">bkb_get_references</span><span class="tool-tag">bkb_get_document</span></div>
</div>
</div>

<div class="section">
<h2>Cross-Source Research</h2>

<div class="prompt-card">
<div class="prompt">What has BlueMatt written about across the Bitcoin ecosystem? Show me his mailing list posts, GitHub PRs, and Delving Bitcoin topics.</div>
<div class="description">Searches with an author filter across all source types to build an author profile.</div>
<div class="tools-used"><span class="tool-tag">bkb_search</span></div>
</div>

<div class="prompt-card">
<div class="prompt">Find all discussions about covenant proposals (OP_CTV, OP_CAT, TXHASH) across the mailing list, Delving Bitcoin, and IRC. What are the main camps and arguments?</div>
<div class="description">Runs multiple targeted searches and retrieves key documents to synthesize a balanced overview.</div>
<div class="tools-used"><span class="tool-tag">bkb_search</span><span class="tool-tag">bkb_get_document</span><span class="tool-tag">bkb_timeline</span></div>
</div>

<div class="prompt-card">
<div class="prompt">Summarize this week's Optech newsletter and link me to the primary sources it references.</div>
<div class="description">Searches for the latest Optech newsletter, then follows BIP/BOLT/PR references to their source documents.</div>
<div class="tools-used"><span class="tool-tag">bkb_search</span><span class="tool-tag">bkb_get_document</span><span class="tool-tag">bkb_get_references</span></div>
</div>
</div>

<div class="section">
<h2>Quick Lookups</h2>

<div class="prompt-card">
<div class="prompt">What does BIP-141 specify?</div>
<div class="description">Straight spec lookup &mdash; returns the BIP content and a list of documents that reference it.</div>
<div class="tools-used"><span class="tool-tag">bkb_lookup_bip</span></div>
</div>

<div class="prompt-card">
<div class="prompt">Find IRC discussions about assumeUTXO from the last 6 months.</div>
<div class="description">Targeted search with source type and date filters.</div>
<div class="tools-used"><span class="tool-tag">bkb_search</span></div>
</div>

<div class="prompt-card">
<div class="prompt">Look up the trampoline routing commit in Eclair.</div>
<div class="description">Finds the specific commit with associated PR context in a Lightning implementation.</div>
<div class="tools-used"><span class="tool-tag">bkb_find_commit</span></div>
</div>
</div>

<footer>Bitcoin Knowledge Base &mdash; <a href="/" style="color:var(--accent2)">Home</a></footer>
</body>
</html>
"##;
