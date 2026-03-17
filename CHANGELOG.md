# Changelog

All notable changes to this project will be documented in this file.

## [0.1.0] - 2026-03-17

### Initial Release

First public release of the Bitcoin Knowledge Base (BKB) -- a system that
ingests, indexes, and serves knowledge from across the Bitcoin and Lightning
development ecosystem, designed for querying by AI agents via MCP.

#### Core Architecture
- Modular workspace with five crates: `bkb-core` (types/traits), `bkb-store`
  (SQLite + FTS5 backend), `bkb-ingest` (source adapters and job queue),
  `bkb-server` (HTTP API and scheduler), and `bkb-mcp` (MCP server for AI
  agents).
- Single `documents` table with `source_type` discriminator and full-text
  search via SQLite FTS5.
- Application-level change log for tracking document mutations.
- Rate-limited ingestion with adaptive backoff from GitHub API headers.

#### Source Adapters
- **GitHub**: issues, PRs, comments, reviews, review comments, discussions,
  and git commits (with `RepoCache` for local clones).
- **Specifications**: BIPs, BOLTs, bLIPs, LUDs, and NUTs -- auto-discovered
  via the GitHub Tree API.
- **Mailing lists**: bitcoin-dev and lightning-dev archives via
  mail-archive.com with offset-based pagination.
- **IRC logs**: `#bitcoin-core-dev` and other channels from gnusha.org.
- **Delving Bitcoin**: topics and posts via the Discourse API.
- **Bitcoin Optech**: newsletters, topic pages, and blog posts.
- **BitcoinTalk**: technical board topics and posts.

#### Intelligence Features
- Concept tagging enrichment with 50 Bitcoin/Lightning concept slugs derived
  from Optech topics.
- Timeline queries: chronological event view for any concept across all
  sources.
- `find_commit`: specialized search for commits and associated PRs.
- Cross-reference extraction: BIP, BOLT, bLIP, LUD, NUT, issue, PR, and
  commit references automatically linked.

#### API and MCP
- HTTP API with endpoints: `/search`, `/document/{id}`, `/references/{entity}`,
  `/bip/{n}`, `/bolt/{n}`, `/blip/{n}`, `/lud/{n}`, `/nut/{n}`,
  `/timeline/{concept}`, `/find_commit`, `/health`.
- MCP server (JSON-RPC over stdio) exposing all query capabilities as tools:
  `bkb_search`, `bkb_get_document`, `bkb_get_references`, `bkb_lookup_bip`,
  `bkb_lookup_bolt`, `bkb_lookup_blip`, `bkb_timeline`, `bkb_find_commit`.
- Wildcard/unfiltered search support.
- Shareable URLs for search queries and spec lookups.

#### Operations
- Landing page with interactive search UI and quick spec lookup.
- Admin dashboard with Prometheus metrics, sync state monitoring, per-source
  reset, and re-enrichment controls.
- `--dev-subset` mode for fast iteration with a small slice of data.
- `--ingest-only` CLI for per-source testing.
- Deployment configs for systemd and nginx with TLS.
