use axum::http::header;
use axum::response::{Html, IntoResponse};

/// Handler for `GET /` -- serves the landing page.
pub async fn landing_page() -> Html<&'static str> {
	Html(LANDING_HTML)
}

/// Handler for `GET /logo.png` -- serves the embedded logo image.
pub async fn logo() -> impl IntoResponse {
	([(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "public, max-age=86400")], LOGO)
}

const LOGO: &[u8] = include_bytes!("../../bkb-logo.png");

const LANDING_HTML: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Bitcoin Knowledge Base</title>
<style>
:root {
	--bg: #fafafa; --fg: #1a1a1a; --muted: #666; --border: #ddd;
	--card-bg: #fff; --accent: #f7931a; --accent2: #4a90d9;
	--badge-bg: #eee; --input-bg: #fff; --code-bg: #f0f0f0;
}
@media (prefers-color-scheme: dark) {
	:root {
		--bg: #1a1a2e; --fg: #e0e0e0; --muted: #999; --border: #333;
		--card-bg: #16213e; --accent: #f7931a; --accent2: #6db3f2;
		--badge-bg: #2a2a4a; --input-bg: #1e2a45; --code-bg: #16213e;
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
.subtitle { color: var(--muted); margin-bottom: 1rem; }
.sources { display: flex; flex-wrap: wrap; gap: 0.4rem; margin-bottom: 1.5rem; }
.badge {
	display: inline-block; padding: 0.15rem 0.5rem; border-radius: 4px;
	background: var(--badge-bg); font-size: 0.8rem; color: var(--muted);
}
.links { margin-bottom: 1.5rem; }
.links a {
	color: var(--accent2); text-decoration: none; margin-right: 1.5rem;
	font-size: 0.9rem;
}
.links a:hover { text-decoration: underline; }
.search-box {
	display: flex; gap: 0.5rem; margin-bottom: 0.75rem; flex-wrap: wrap;
}
.search-box input[type="text"] {
	flex: 1; min-width: 200px; padding: 0.5rem 0.75rem; border: 1px solid var(--border);
	border-radius: 6px; background: var(--input-bg); color: var(--fg);
	font-size: 1rem;
}
.search-box select {
	padding: 0.5rem; border: 1px solid var(--border); border-radius: 6px;
	background: var(--input-bg); color: var(--fg); font-size: 0.9rem;
}
.search-box button {
	padding: 0.5rem 1rem; border: none; border-radius: 6px;
	background: var(--accent); color: #fff; font-weight: 600;
	cursor: pointer; font-size: 0.9rem;
}
.search-box button:hover { opacity: 0.9; }
.lookup-row {
	display: flex; gap: 0.5rem 1rem; margin-bottom: 1.5rem; flex-wrap: wrap;
	align-items: center; font-size: 0.9rem;
}
.lookup-row .lookup-heading { color: var(--muted); width: 100%; margin-bottom: -0.25rem; }
.lookup-group {
	display: inline-flex; gap: 0.3rem; align-items: center; white-space: nowrap;
}
.lookup-group label { color: var(--muted); }
.lookup-group input[type="number"] {
	width: 70px; padding: 0.4rem; border: 1px solid var(--border);
	border-radius: 6px; background: var(--input-bg); color: var(--fg);
}
.lookup-group input[type="text"] {
	width: 140px; padding: 0.4rem; border: 1px solid var(--border);
	border-radius: 6px; background: var(--input-bg); color: var(--fg);
}
.lookup-group button {
	padding: 0.4rem 0.75rem; border: 1px solid var(--border); border-radius: 6px;
	background: var(--badge-bg); color: var(--fg); cursor: pointer; font-size: 0.85rem;
}
.lookup-group button:hover { border-color: var(--accent); }
footer {
	margin-top: 2rem; padding-top: 1rem; border-top: 1px solid var(--border);
	font-size: 0.8rem; color: var(--muted); text-align: center;
}
#results { margin-top: 1rem; }
.result {
	border: 1px solid var(--border); border-radius: 8px; padding: 0.75rem 1rem;
	margin-bottom: 0.75rem; background: var(--card-bg);
}
.result-title {
	font-weight: 600; margin-bottom: 0.25rem;
}
.result-title a { color: var(--accent2); text-decoration: none; }
.result-title a:hover { text-decoration: underline; }
.result-meta { font-size: 0.8rem; color: var(--muted); margin-bottom: 0.3rem; }
.result-snippet { font-size: 0.9rem; }
.result-snippet mark { background: var(--accent); color: #fff; padding: 0 2px; border-radius: 2px; }
.result-concepts { margin-top: 0.3rem; }
.concept-tag {
	display: inline-block; padding: 0.1rem 0.4rem; border-radius: 3px;
	background: var(--accent); color: #fff; font-size: 0.7rem; margin-right: 0.3rem;
}
.error { color: #e74c3c; margin-top: 0.5rem; }
#detail { margin-top: 1rem; }
.detail-card {
	border: 1px solid var(--border); border-radius: 8px; padding: 1rem;
	background: var(--card-bg); margin-bottom: 1rem;
}
.detail-card h3 { margin-bottom: 0.5rem; color: var(--accent2); }
.detail-body {
	max-height: 400px; overflow-y: auto; white-space: pre-wrap;
	font-family: monospace; font-size: 0.85rem; padding: 0.5rem;
	background: var(--code-bg); border-radius: 4px;
}
</style>
</head>
<body>
<div class="logo"><a href="/"><img src="/logo.png" alt="BKB"></a></div>
<p class="subtitle">Indexed knowledge from across the Bitcoin and Lightning development ecosystem, queryable by AI agents via MCP.</p>


<div class="links">
	<a href="https://github.com/tnull/bitcoin-knowledge-base">GitHub</a>
	<a href="/examples">Example Prompts</a>
	<a href="/sources">Data Sources</a>
	<a href="https://github.com/tnull/bitcoin-knowledge-base/blob/master/SKILL.md">Agent Setup (SKILL.md)</a>
	<a href="https://github.com/tnull/bitcoin-knowledge-base/blob/master/docs/DESIGN.md">Design Doc</a>
	<a href="/health">API Health</a>
</div>

<div class="search-box">
	<input type="text" id="q" placeholder="Search the knowledge base..." autofocus>
	<select id="source-type">
		<option value="">All sources</option>
		<option value="github_issue">GitHub Issues</option>
		<option value="github_pr">GitHub PRs</option>
		<option value="github_comment">GitHub Comments</option>
		<option value="commit">Commits</option>
		<option value="bip">BIPs</option>
		<option value="bolt">BOLTs</option>
		<option value="blip">bLIPs</option>
		<option value="mailing_list_msg">Mailing List</option>
		<option value="irc_log">IRC Logs</option>
		<option value="delving_topic">Delving Topics</option>
		<option value="delving_post">Delving Posts</option>
		<option value="optech_newsletter">Optech Newsletters</option>
	</select>
	<button onclick="doSearch()">Search</button>
</div>

<div class="lookup-row">
	<span class="lookup-heading">Quick lookup:</span>
	<div class="lookup-group">
		<label>BIP</label><input type="number" id="bip-num" min="0" placeholder="#">
		<button onclick="lookupSpec('bip',document.getElementById('bip-num').value)">Go</button>
	</div>
	<div class="lookup-group">
		<label>BOLT</label><input type="number" id="bolt-num" min="0" placeholder="#">
		<button onclick="lookupSpec('bolt',document.getElementById('bolt-num').value)">Go</button>
	</div>
	<div class="lookup-group">
		<label>bLIP</label><input type="number" id="blip-num" min="0" placeholder="#">
		<button onclick="lookupSpec('blip',document.getElementById('blip-num').value)">Go</button>
	</div>
	<div class="lookup-group">
		<label>Timeline</label><input type="text" id="timeline-concept" placeholder="concept slug">
		<button onclick="lookupTimeline()">Go</button>
	</div>
</div>

<div id="results"></div>
<div id="detail"></div>

<footer id="stats">Loading stats...</footer>

<script>
async function loadStats() {
	try {
		const r = await fetch('/health');
		const d = await r.json();
		const t = d.documents?.total || 0;
		const types = d.documents?.by_type || {};
		const parts = Object.entries(types).map(([k,v]) => k + ': ' + v).join(', ');
		document.getElementById('stats').textContent = t + ' documents indexed' + (parts ? ' (' + parts + ')' : '');
	} catch(e) {
		document.getElementById('stats').textContent = 'Could not load stats';
	}
}
loadStats();

document.getElementById('q').addEventListener('keydown', e => { if (e.key === 'Enter') doSearch(); });
document.getElementById('bip-num').addEventListener('keydown', e => { if (e.key === 'Enter') lookupSpec('bip', e.target.value); });
document.getElementById('bolt-num').addEventListener('keydown', e => { if (e.key === 'Enter') lookupSpec('bolt', e.target.value); });
document.getElementById('blip-num').addEventListener('keydown', e => { if (e.key === 'Enter') lookupSpec('blip', e.target.value); });
document.getElementById('timeline-concept').addEventListener('keydown', e => { if (e.key === 'Enter') lookupTimeline(); });

async function doSearch(pushState) {
	const q = document.getElementById('q').value.trim();
	if (!q) return;
	const st = document.getElementById('source-type').value;
	let url = '/search?q=' + encodeURIComponent(q) + '&limit=20';
	if (st) url += '&source_type=' + st;
	document.getElementById('detail').innerHTML = '';
	if (pushState !== false) {
		const qs = '?q=' + encodeURIComponent(q) + (st ? '&source_type=' + st : '');
		history.pushState({type:'search', q, source_type: st}, '', qs);
	}
	try {
		const r = await fetch(url);
		const d = await r.json();
		if (d.error) { showError(d.error); return; }
		renderResults(d.results || []);
	} catch(e) { showError(e.message); }
}

function renderResults(results) {
	const el = document.getElementById('results');
	if (!results.length) { el.innerHTML = '<p style="color:var(--muted)">No results found.</p>'; return; }
	el.innerHTML = results.map(r => {
		const url = r.url ? '<a href="' + esc(r.url) + '" target="_blank">' + esc(r.title || r.id) + '</a>' : esc(r.title || r.id);
		const concepts = (r.concepts || []).map(c => '<span class="concept-tag">' + esc(c) + '</span>').join('');
		return '<div class="result">' +
			'<div class="result-title">' + url + '</div>' +
			'<div class="result-meta"><span class="badge">' + esc(r.source_type) + '</span> ' +
			(r.author ? 'by ' + esc(r.author) + ' ' : '') +
			(r.created_at ? r.created_at.slice(0,10) : '') +
			' &middot; score: ' + (r.score||0).toFixed(2) + '</div>' +
			(r.snippet ? '<div class="result-snippet">' + r.snippet + '</div>' : '') +
			(concepts ? '<div class="result-concepts">' + concepts + '</div>' : '') +
			'</div>';
	}).join('');
}

async function lookupSpec(type, num, pushState) {
	if (!num) return;
	document.getElementById('results').innerHTML = '';
	if (pushState !== false) {
		history.pushState({type:'spec', spec: type, num}, '', '?' + type + '=' + num);
	}
	try {
		const r = await fetch('/' + type + '/' + num);
		const d = await r.json();
		if (d.error) { showError(d.error); return; }
		renderDetail(d);
	} catch(e) { showError(e.message); }
}

async function lookupTimeline(pushState) {
	const concept = document.getElementById('timeline-concept').value.trim();
	if (!concept) return;
	document.getElementById('results').innerHTML = '';
	if (pushState !== false) {
		history.pushState({type:'timeline', concept}, '', '?timeline=' + encodeURIComponent(concept));
	}
	try {
		const r = await fetch('/timeline/' + encodeURIComponent(concept));
		const d = await r.json();
		if (d.error) { showError(d.error); return; }
		renderTimeline(d);
	} catch(e) { showError(e.message); }
}

function renderDetail(ctx) {
	const el = document.getElementById('detail');
	const doc = ctx.document;
	const url = ctx.url ? '<a href="' + esc(ctx.url) + '" target="_blank">' + esc(ctx.url) + '</a>' : '';
	const concepts = (ctx.concepts || []).map(c => '<span class="concept-tag">' + esc(c) + '</span>').join(' ');
	const refs_in = (ctx.incoming_refs || []).length;
	const refs_out = (ctx.outgoing_refs || []).length;
	const body = doc.body ? doc.body.slice(0, 3000) + (doc.body.length > 3000 ? '\n\n... (truncated)' : '') : '(no body)';
	el.innerHTML = '<div class="detail-card">' +
		'<h3>' + esc(doc.title || doc.id) + '</h3>' +
		(url ? '<p style="font-size:0.85rem;margin-bottom:0.5rem">' + url + '</p>' : '') +
		'<div class="result-meta" style="margin-bottom:0.5rem"><span class="badge">' + esc(doc.source_type) + '</span> ' +
		(doc.author ? 'by ' + esc(doc.author) : '') +
		' &middot; refs in: ' + refs_in + ' &middot; refs out: ' + refs_out + '</div>' +
		(concepts ? '<div class="result-concepts" style="margin-bottom:0.5rem">' + concepts + '</div>' : '') +
		'<div class="detail-body">' + esc(body) + '</div>' +
		'</div>';
}

function renderTimeline(tl) {
	const el = document.getElementById('detail');
	if (!tl.events || !tl.events.length) {
		el.innerHTML = '<p style="color:var(--muted)">No timeline events for "' + esc(tl.concept) + '".</p>';
		return;
	}
	el.innerHTML = '<div class="detail-card"><h3>Timeline: ' + esc(tl.concept) + '</h3>' +
		tl.events.map(e => {
			const link = e.url ? '<a href="' + esc(e.url) + '" target="_blank">' + esc(e.title || e.id) + '</a>' : esc(e.title || e.id);
			return '<div style="margin:0.4rem 0"><span style="color:var(--muted)">' + esc(e.date) + '</span> ' +
				'<span class="badge">' + esc(e.type) + '</span> ' + link + '</div>';
		}).join('') +
		'</div>';
}

function showError(msg) {
	document.getElementById('results').innerHTML = '<p class="error">' + esc(msg) + '</p>';
}

function esc(s) {
	if (!s) return '';
	return s.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;');
}

function restoreFromURL() {
	const p = new URLSearchParams(location.search);
	if (p.get('q')) {
		document.getElementById('q').value = p.get('q');
		if (p.get('source_type')) document.getElementById('source-type').value = p.get('source_type');
		doSearch(false);
	} else if (p.get('bip')) {
		document.getElementById('bip-num').value = p.get('bip');
		lookupSpec('bip', p.get('bip'), false);
	} else if (p.get('bolt')) {
		document.getElementById('bolt-num').value = p.get('bolt');
		lookupSpec('bolt', p.get('bolt'), false);
	} else if (p.get('blip')) {
		document.getElementById('blip-num').value = p.get('blip');
		lookupSpec('blip', p.get('blip'), false);
	} else if (p.get('timeline')) {
		document.getElementById('timeline-concept').value = p.get('timeline');
		lookupTimeline(false);
	}
}
restoreFromURL();

window.addEventListener('popstate', function(e) {
	document.getElementById('results').innerHTML = '';
	document.getElementById('detail').innerHTML = '';
	if (e.state) {
		if (e.state.type === 'search') {
			document.getElementById('q').value = e.state.q || '';
			document.getElementById('source-type').value = e.state.source_type || '';
			doSearch(false);
		} else if (e.state.type === 'spec') {
			lookupSpec(e.state.spec, e.state.num, false);
		} else if (e.state.type === 'timeline') {
			document.getElementById('timeline-concept').value = e.state.concept || '';
			lookupTimeline(false);
		}
	}
});
</script>
</body>
</html>
"##;
