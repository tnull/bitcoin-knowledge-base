# Bitcoin Knowledge Base (BKB)

A Bitcoin-ecosystem-specific knowledge base that ingests, indexes, and serves
structured data from multiple sources across the Bitcoin and Lightning
development ecosystem. Designed to be queried by AI agents via an MCP (Model
Context Protocol) server for fast, precise lookups.

## Data Sources

| Source | Adapter | Content |
|---|---|---|
| GitHub | Issues, PRs, comments | Bitcoin Core, LDK, rust-bitcoin, BDK, Payjoin, BOLTs, bLIPs |
| Mailing Lists | bitcoin-dev | Proposals, discussions, reviews |
| IRC Logs | gnusha.org | `#bitcoin-core-dev`, `#lightning-dev`, `#bitcoin-wizards` |
| Delving Bitcoin | Discourse API | Technical discussion forum |
| BIPs | Raw spec files | Bitcoin Improvement Proposals |
| BOLTs | Raw spec files | Lightning Network specifications |
| bLIPs | Raw spec files | Bitcoin Lightning Improvement Proposals |
| Bitcoin Optech | Newsletters | Weekly summaries and topic coverage |

## Architecture

```
bkb-core      Shared types, traits, schema, Bitcoin concept vocabulary
bkb-store     SQLite + FTS5 storage backend (KnowledgeStore implementation)
bkb-ingest    Source adapters, job queue, rate limiter, enrichment pipeline
bkb-server    HTTP API (axum) + ingestion scheduler
bkb-mcp       MCP server (JSON-RPC 2.0 over stdio) for AI agent access
```

All content is normalized into a single `documents` table with FTS5 full-text
search. Cross-references (BIP/BOLT/bLIP mentions, issue links, `Fixes`/`Closes`
patterns) are extracted during ingestion. Documents are tagged with Bitcoin
concepts (taproot, HTLCs, covenants, etc.) from a curated 35-concept vocabulary.

## Quick Start

```bash
# Full server with dev subset (fast: ~2 minutes for initial sync)
GITHUB_TOKEN=ghp_... cargo run -p bkb-server -- --dev-subset

# Test a single source adapter
cargo run -p bkb-server -- --ingest-only github:lightningdevkit/ldk-sample --limit-pages 1

# API-only mode (no ingestion)
cargo run -p bkb-server -- --no-ingest --db bkb.db
```

The HTTP API is available at `http://127.0.0.1:3000` by default.

## API Endpoints

| Endpoint | Description |
|---|---|
| `GET /search?q=...` | Full-text search with source type, repo, author, and date filters |
| `GET /document/{id}` | Single document with content, cross-references, and concept tags |
| `GET /references/{entity}` | All documents referencing an entity (e.g., `BIP-340`) |
| `GET /bip/{number}` | BIP spec with all incoming references across the knowledge base |
| `GET /bolt/{number}` | BOLT spec with all incoming references |
| `GET /blip/{number}` | bLIP spec with all incoming references |
| `GET /timeline/{concept}` | Chronological events for a concept across all sources |
| `GET /find_commit?q=...` | Find commits/PRs matching a description |
| `GET /health` | Server status with document counts by source type |

## MCP Tools

The MCP server (`bkb-mcp`) exposes the following tools for AI agents:

- `bkb_search` -- Search across all sources
- `bkb_get_document` -- Get full document by ID
- `bkb_get_references` -- Find cross-references to an entity
- `bkb_lookup_bip` -- Comprehensive BIP context
- `bkb_lookup_bolt` -- Comprehensive BOLT context
- `bkb_lookup_blip` -- Comprehensive bLIP context
- `bkb_timeline` -- Concept timeline across sources
- `bkb_find_commit` -- Find commits with associated PR context

## Development

```bash
cargo check --workspace    # Type-check all crates
cargo test --workspace     # Run all 76 tests
cargo fmt --all            # Format (uses hard tabs, 100-char width)
```

See [docs/DESIGN.md](docs/DESIGN.md) for the full design document including
data model, ingestion pipeline details, and implementation roadmap.

## License

MIT OR Apache-2.0
