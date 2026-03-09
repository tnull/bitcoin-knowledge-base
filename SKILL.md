# Bitcoin Knowledge Base (BKB) -- MCP Skill

The Bitcoin Knowledge Base indexes and serves structured data from across the
Bitcoin and Lightning development ecosystem. Use it to look up BIPs, BOLTs,
bLIPs, find discussions, trace concept timelines, and search across mailing
lists, IRC logs, GitHub issues/PRs, Delving Bitcoin, and Optech newsletters.

**Repository:** https://github.com/tnull/bitcoin-knowledge-base

## Setup

### 1. Start the BKB API server

```bash
git clone https://github.com/tnull/bitcoin-knowledge-base.git
cd bitcoin-knowledge-base
GITHUB_TOKEN=ghp_... cargo run -p bkb-server -- --dev-subset
```

The API will be available at `http://127.0.0.1:3000`.

### 2. Configure the MCP client

Add `bkb-mcp` to your Claude Code MCP configuration
(`~/.claude/claude_code_config.json`):

```json
{
  "mcpServers": {
    "bkb": {
      "command": "cargo",
      "args": ["run", "-p", "bkb-mcp", "--manifest-path", "/path/to/bitcoin-knowledge-base/Cargo.toml"],
      "env": {
        "BKB_API_URL": "http://127.0.0.1:3000"
      }
    }
  }
}
```

## Available Tools

### `bkb_search`
Search across all indexed sources. Supports filters by source type, repository,
author, and date range.

```
bkb_search(query: "taproot", source_type: "github_pr", limit: 10)
```

### `bkb_get_document`
Get full document content by ID, including cross-references and concept tags.

```
bkb_get_document(id: "github_issue:bitcoin/bitcoin:21907")
```

### `bkb_get_references`
Find all documents referencing a given entity.

```
bkb_get_references(entity: "BIP-340")
```

### `bkb_lookup_bip`
Get comprehensive context for a BIP: spec text, all referencing discussions,
PRs, and related documents.

```
bkb_lookup_bip(number: 340)
```

### `bkb_lookup_bolt`
Get comprehensive context for a BOLT: spec text and all referencing documents.

```
bkb_lookup_bolt(number: 11)
```

### `bkb_lookup_blip`
Get comprehensive context for a bLIP: spec text and all referencing documents.

```
bkb_lookup_blip(number: 1)
```

### `bkb_timeline`
Chronological timeline of a concept across all sources: mailing list proposals,
BIPs, implementation PRs, Optech coverage.

```
bkb_timeline(concept: "taproot")
```

### `bkb_find_commit`
Find commits/PRs matching a description, with associated discussion context.

```
bkb_find_commit(query: "schnorr signature verification")
```

## Source Types

Use these values for `source_type` filters:

| Value | Description |
|---|---|
| `github_issue` | GitHub issues |
| `github_pr` | GitHub pull requests |
| `github_comment` | GitHub issue/PR comments |
| `bip` | Bitcoin Improvement Proposals |
| `bolt` | Lightning Network BOLTs |
| `blip` | Bitcoin Lightning Improvement Proposals |
| `mailing_list_msg` | bitcoin-dev mailing list messages |
| `irc_log` | IRC log entries |
| `delving_topic` | Delving Bitcoin topics |
| `delving_post` | Delving Bitcoin posts |
| `optech_newsletter` | Bitcoin Optech newsletters |
| `optech_topic` | Bitcoin Optech topic pages |
