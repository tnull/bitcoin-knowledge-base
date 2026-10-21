# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

## [0.3.0] - 2026-10-21

### Added
- Forgejo ingestion for issues, PRs, conversation comments, reviews, and
  inline review comments from ten migrated LDK and rust-bitcoin repositories,
  including rust-miniscript and rapid-gossip-sync-server. Historical GitHub
  ingestion remains enabled; see [repository mappings](docs/FORGEJO.md).
- GitHub review and inline review-comment backfills for migrated repositories.
- Index `bitcoin-core/bitcoincore.org`,
  `bitcoin-core/bitcoin-maintainer-tools`,
  `rust-bitcoin/rust-bitcoin-maintainer-tools`, and
  `lightningdevkit/ldk-fuzzing-corpus`.
- Index `BitcoinAndLightningLayerSpecs/lsp` (LSPS) GitHub repository for
  historical issues, PRs, and discussions. The repository is archived, but
  its past technical discussions remain valuable context for Lightning
  Service Provider specifications.
- REST and MCP source provenance, `matched_source_id` for search snippets,
  `deduplicate=false` searches, and `raw=true` document reads.
- Metrics for raw and canonical documents, verified and unresolved links,
  and ingestion lag.

### Changed
- Search collapses verified GitHub/Forgejo copies before applying limits.
  Document lookups, references, and timelines resolve verified aliases while
  preserving raw records and IDs. Logical repository filters cover both
  forges; explicit host names select one source's membership.
- Git history follows each migrated repository's primary forge while retaining
  existing commit IDs and cached history.
- Stored-data upgrades validate schema compatibility, record readability,
  foreign keys, and SQLite/full-text integrity. Unsafe upgrades are refused;
  schema changes and seeding are atomic and protect existing application data.
- Deployment prompts for unset GitHub/Forgejo tokens and the admin password,
  installs credentials in a protected service environment file, and waits for
  local and public API readiness. Startup failures include diagnostics, and
  database upgrades log validation stages and row counts.
- Update locked dependencies within the existing Rust 1.85 requirement.

### Fixed
- Commit backfills no longer skip older pages or advance past failed writes.
  Legacy cursors are replayed while retaining existing records and enrichment.
- Conversation feeds checkpoint pages atomically, resume nested review scans,
  and retain local history when remote records disappear or Git fetches fail.
- Cross-forge references and review permalinks retain their source context;
  ambiguous or unverified matches remain separate.
- Forgejo API requests use a curl User-Agent accepted by the instance's
  human-verification filter, with shared request pacing and quota deferrals.
- Deployment honors `CARGO_TARGET_DIR` and builds with the lockfile before
  uploading the binary.
- MCP initialization and OpenAPI metadata report the package version instead
  of a stale hard-coded version.

### Upgrade notes
- This release adds public Rust struct fields and `SourceType` variants, so
  downstream Rust callers may need changes. Update MCP clients alongside the
  server and restart their connections: older clients cannot deserialize the
  new Forgejo source types.
- Existing databases upgrade additively, preserving stored documents, IDs,
  references, concepts, change-log entries, and ingestion cursors. Back up
  before upgrading. The first upgrade scans the database and full-text index
  before HTTP starts, which can take a long time on large databases.
- Rolling back after ingestion of new source types requires restoring a
  pre-upgrade backup. See [upgrade details](docs/FORGEJO.md#backfill-and-operations).

## [0.2.1] - 2026-04-16

### Fixed
- MCP server no longer sends error responses for JSON-RPC notifications
  (e.g. `notifications/initialized`), which caused Claude Code to mark
  the connection as failed.

## [0.2.0] - 2026-03-19

### Added
- OpenAPI 3.1 spec served at `/openapi.json` for ChatGPT Custom GPT Actions
  integration, enabling ChatGPT to query the knowledge base alongside the
  existing MCP support for Claude.
- Privacy policy page at `/privacy` for ChatGPT store publishing.
- Admin reset now refuses to proceed while a re-enrich job is running for the
  same source type (`409 Conflict`), preventing orphaned data.

### Fixed
- FTS5 search queries containing `:`, `/`, or `#` (e.g. repo-qualified names
  like `lightningdevkit/ldk-node:4463`) no longer crash with "no such column"
  errors. Individual terms containing these characters are now quoted before
  being passed to the FTS5 engine.
- `/document/{id}` route changed to a catch-all (`/document/{*id}`) so that
  document IDs containing `/` survive reverse proxies that decode `%2F`.
- Admin "Reset" for paginated sources (BIPs, BOLTs, etc.) no longer leaves
  ingestion stuck on the last page. The sync job now checks `sync_state`
  before each run and clears stale in-memory cursors when the DB entry has
  been deleted.
- FTS5 operator detection now uses exact whole-word matching, preventing
  false positives where words like "MONITOR" (contains "OR") or "HANDLER"
  (contains "AND") were misidentified as FTS5 boolean keywords.
- FTS5 queries containing double-quotes with colons (e.g.
  `lightning:"channel close"`) no longer bypass per-term quoting. All
  double-quotes are now stripped from user input before sanitization, and
  `find_associated_prs` SHA lookups are routed through the same
  `build_fts_query` sanitizer for defense-in-depth.

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
- Concept tagging enrichment with 50+ Bitcoin/Lightning concept slugs derived
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
