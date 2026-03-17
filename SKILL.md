# Bitcoin Knowledge Base (BKB) -- MCP Skill

Search across the Bitcoin and Lightning development ecosystem: BIPs,
BOLTs, bLIPs, LUDs, NUTs, GitHub issues/PRs/commits, mailing lists,
IRC logs, Delving Bitcoin, BitcoinTalk, and Optech newsletters.

## Setup

Install the `bkb-mcp` client and add it to your Claude Code MCP
configuration (`~/.claude.json`):

```bash
cargo install --git https://github.com/tnull/bitcoin-knowledge-base.git bkb-mcp
```

Add a `bkb` entry to the `mcpServers` object in `~/.claude.json`:

```json
{
  "mcpServers": {
    "bkb": {
      "type": "stdio",
      "command": "bkb-mcp",
      "args": [],
      "env": {
        "BKB_API_URL": "https://bitcoinknowledge.dev"
      }
    }
  }
}
```

## Available Tools

- `bkb_search` -- Full-text search across all sources (filters: `source_type`, `source_repo`, `author`, `after`, `before`, `limit`). Supports wildcard queries: use `query: "*"` with at least one filter to retrieve all documents matching the filters (e.g., all commits in a repo within a date range).
- `bkb_get_document` -- Get full document by ID with content, cross-references, and concept tags
- `bkb_get_references` -- Find all documents referencing an entity (e.g. `BIP-340`, `bitcoin/bitcoin#12345`)
- `bkb_lookup_bip` -- BIP spec with all referencing discussions and PRs
- `bkb_lookup_bolt` -- BOLT spec with all referencing documents
- `bkb_lookup_blip` -- bLIP spec with all referencing documents
- `bkb_lookup_lud` -- LUD spec with all referencing documents
- `bkb_lookup_nut` -- NUT spec with all referencing documents
- `bkb_timeline` -- Chronological timeline of a concept across all sources
- `bkb_find_commit` -- Find commits/PRs matching a description
