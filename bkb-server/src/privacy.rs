use axum::response::Html;

/// Handler for `GET /privacy` -- serves the privacy policy.
pub async fn privacy_policy() -> Html<&'static str> {
	Html(PRIVACY_HTML)
}

const PRIVACY_HTML: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Privacy Policy - Bitcoin Knowledge Base</title>
<style>
:root {
	--bg: #fafafa; --fg: #1a1a1a; --muted: #666; --border: #ddd;
	--accent2: #4a90d9;
}
@media (prefers-color-scheme: dark) {
	:root {
		--bg: #1a1a2e; --fg: #e0e0e0; --muted: #999; --border: #333;
		--accent2: #6db3f2;
	}
}
* { box-sizing: border-box; margin: 0; padding: 0; }
body {
	font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
	background: var(--bg); color: var(--fg); line-height: 1.7;
	max-width: 700px; margin: 0 auto; padding: 2rem 1rem;
}
h1 { margin-bottom: 0.5rem; }
h2 { margin-top: 1.5rem; margin-bottom: 0.5rem; }
p, ul { margin-bottom: 1rem; }
ul { padding-left: 1.5rem; }
a { color: var(--accent2); text-decoration: none; }
a:hover { text-decoration: underline; }
.updated { color: var(--muted); font-size: 0.9rem; margin-bottom: 1.5rem; }
</style>
</head>
<body>
<h1>Privacy Policy</h1>
<p class="updated">Last updated: March 17, 2026</p>

<h2>Overview</h2>
<p>
The Bitcoin Knowledge Base ("BKB", hosted at
<a href="https://bitcoinknowledge.dev">bitcoinknowledge.dev</a>) is a
read-only search service for publicly available Bitcoin and Lightning Network
development resources. This privacy policy applies to all access methods,
including the web interface, the HTTP API, the MCP server, and ChatGPT
Actions (OpenAI).
</p>

<h2>No User Data Collection</h2>
<p>
BKB does not collect, store, or process any personal data. Specifically:
</p>
<ul>
<li>No user accounts or authentication are required.</li>
<li>No cookies, tracking pixels, or analytics scripts are used.</li>
<li>No search queries or API requests are logged with user-identifying
    information.</li>
<li>No IP addresses are stored or associated with requests.</li>
<li>No data is shared with or sold to third parties.</li>
</ul>

<h2>Data Served</h2>
<p>
All content served by BKB is sourced exclusively from publicly available
resources: GitHub repositories, public mailing list archives, IRC logs,
Delving Bitcoin forum posts, BitcoinTalk forum posts, Bitcoin Optech
newsletters, and specification documents (BIPs, BOLTs, bLIPs, LUDs, NUTs).
No proprietary or user-generated content is stored.
</p>

<h2>ChatGPT / OpenAI Actions</h2>
<p>
When accessed via a ChatGPT Custom GPT, OpenAI may send API requests to
BKB on behalf of the user. BKB processes these requests identically to any
other API call: no user-identifying information from OpenAI is stored or
logged. For information about how OpenAI handles your data, please refer
to <a href="https://openai.com/policies/privacy-policy">OpenAI's privacy
policy</a>.
</p>

<h2>Server Logs</h2>
<p>
Standard web server access logs (containing IP addresses, timestamps, and
request paths) may be retained transiently for operational purposes such as
debugging and abuse prevention. These logs are not used for tracking or
analytics and are rotated and deleted automatically.
</p>

<h2>Contact</h2>
<p>
For questions about this privacy policy, please open an issue at the
<a href="https://github.com/tnull/bitcoin-knowledge-base">project's
GitHub repository</a>.
</p>

</body>
</html>
"##;
