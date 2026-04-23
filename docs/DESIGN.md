# Bitcoin Knowledge Base (BKB) -- Design Document

## 1. Overview

BKB is a Bitcoin-ecosystem-specific knowledge base that ingests, indexes, and
serves structured data from multiple sources: GitHub repositories (code, issues,
PRs, discussions, commits), mailing lists, IRC chat logs, forum posts,
specification documents (BIPs/BOLTs/bLIPs), and Bitcoin Optech content.

It is designed to be queried by AI agents via MCP (Claude) and OpenAI Actions
(ChatGPT) for fast, precise lookups. A hosted API serves as the single source
of truth, with the schema designed to support future local-database mode via
incremental sync.

## 2. Goals

- **Unified search** across all Bitcoin/Lightning development artifacts: code
  commits, issue discussions, mailing list threads, IRC logs, forum posts,
  specs, and Optech summaries.
- **Bitcoin-domain-aware** tokenization, cross-referencing, and entity
  extraction (BIP/BOLT references, opcodes, Lightning-specific terminology).
- **Always fresh**: hourly incremental sync via a rate-limit-aware job queue.
  No webhook dependencies -- purely pull-based using GitHub API cursors, git
  fetch, and Discourse API pagination.
- **Fast agent queries**: sub-200ms response times for structured queries;
  embedding-based semantic search for fuzzy concept matching.
- **Hybrid deployment**: remote API as primary (always up-to-date, embedding
  support). Local DB mode behind a common `KnowledgeStore` trait for Phase 2
  and for testing.
- **Internal change tracking**: `change_log` table from day one for internal
  ordering, debugging, and replay during ingestion. Client-facing incremental
  sync (e.g., `GET /changes?since=<seq>`) is deferred to a future phase.

## 3. Non-Goals

- Not a general-purpose code knowledge base framework -- Bitcoin-specific by
  design.
- Not a replacement for GitHub search -- focus is on cross-source correlation
  and agent-friendly retrieval.
- Not a real-time system -- hourly freshness is sufficient.
- No write/mutation API for end users -- read-only query interface.
- No authentication/multi-tenancy in Phase 1.
- No client-side incremental sync in Phases 1--4 -- the hosted API is the
  sole query interface. Local database mode with incremental sync is a
  future-phase goal.

## 4. Data Sources

### 4.1 GitHub Repositories

| Source | Repositories | Est. Issues/PRs |
|---|---|---|
| Bitcoin Core | `bitcoin/bitcoin` | ~32,900 |
| Bitcoin Inquisition | `bitcoin-inquisition/bitcoin`, `binana` | ~500 |
| LDK | `lightningdevkit/rust-lightning`, `ldk-node`, `ldk-sample`, `ldk-server`, `ldk-c-bindings`, `ldk-garbagecollected`, `vss-server`, `vss-client`, `rapid-gossip-sync-server`, `ldk-swift`, `ldk-review-club`, `orange-sdk` | ~4,400 |
| LND | `lightningnetwork/lnd` | ~7,000 |
| Core Lightning | `ElementsProject/lightning` | ~6,500 |
| Eclair | `ACINQ/eclair` | ~2,500 |
| rust-bitcoin | `rust-bitcoin/rust-bitcoin`, `rust-secp256k1`, `rust-bech32`, `rust-bech32-bitcoin`, `rust-miniscript`, `rust-bitcoinconsensus`, `hex-conservative`, `rust-psbt`, `rust-psbt-v0`, `corepc`, `bip322`, `bip324`, `bitcoin-payment-instructions`, `rust-bip39`, `bitcoind`, `constants` | ~5,500 |
| BDK | `bitcoindevkit/bdk`, `bdk_wallet`, `bdk-ffi`, `bdk-cli`, `bdk-python`, `bdk-kyoto`, `bdk-jvm`, `bdk-swift`, `bdk-dart`, `bdk-rn`, `bdk-tx`, `bdk-sp`, `bdk-reserves`, `bdk-sqlite`, `bdk-sqlx`, `bdk-bitcoind-client`, `coin-select`, `rust-esplora-client`, `rust-electrum-client`, `bitcoin-ffi`, `rust-cktap`, `electrum_streaming_client`, `devkit-wallet` | ~2,500 |
| Payjoin | `payjoin/rust-payjoin`, `nolooking`, `btsim`, `cja`, `cja-2`, `multiparty-protocol-docs`, `bitcoin-hpke`, `tx-indexer`, `receive-payjoin-v2`, `batch-plot` | ~500 |
| BIPs | `bitcoin/bips` | ~400 specs |
| BOLTs | `lightning/bolts` | ~1,200 issues/PRs |
| bLIPs | `lightning/blips` | ~50 specs |
| LSPS | `BitcoinAndLightningLayerSpecs/lsp` | ~350 issues/PRs (archived) |
| Bitcoin Optech | `bitcoinops/bitcoinops.github.io` | ~400 newsletters, ~150 topics |

### 4.2 Mailing Lists

| List | Archive Location | Est. Messages |
|---|---|---|
| bitcoin-dev | `gnusha.org/pi/bitcoindev/` (public-inbox Atom feed, current) | ~15,000 |
| lightning-dev | `mail-archive.com/lightning-dev@lists.linuxfoundation.org/` (HTML scraping, archived 2017-2024) | ~3,525 |

### 4.3 IRC Chat Logs

| Source | Archive Location | Notes |
|---|---|---|
| All channels on gnusha.org | `gnusha.org/{channel}/` | Includes `#bitcoin-core-dev`, `#lightning-dev`, `#bitcoin-wizards`, and others. Each daily log is indexed as one document. |

### 4.4 Forums

| Source | API | Est. Items |
|---|---|---|
| Delving Bitcoin | Discourse REST API (`delvingbitcoin.org`) | ~484 topics, ~6,832 posts |
| BitcoinTalk | HTML scraping (SMF 2.0, `bitcointalk.org`) | Technical boards: Bitcoin Discussion (1), Dev & Technical (6), Technical Support (4), Project Development (12), Mining (14 + children), Economics (7) |

### 4.5 Total Data Estimates

| Category | Est. Raw Text |
|---|---|
| Source code (current tree, all repos) | ~550 MB |
| Git commit metadata + messages | ~250 MB |
| Issues + PRs + comments | ~750 MB |
| Mailing list archives | ~200 MB |
| IRC chat logs | ~200-500 MB |
| BitcoinTalk (technical boards) | ~200-500 MB |
| Delving Bitcoin | ~20 MB |
| Specs (BIPs + BOLTs) | ~25 MB |
| Bitcoin Optech content | ~80 MB |
| **Total** | **~2.2-3 GB** |

## 5. Architecture

```
+--------------------------------------------------------------+
|                       BKB Service                            |
|                                                              |
|  +--------------------------------------------------------+  |
|  |                 Job Queue / Scheduler                   |  |
|  |                                                        |  |
|  |  Priority queue of SyncJobs, sorted by next_run.       |  |
|  |  Each job holds a cursor for resumable pagination.     |  |
|  |  Same codepath for initial sync and incremental sync.  |  |
|  |                                                        |  |
|  |  +----------+ +----------+ +----------+ +-----------+  |  |
|  |  | GitHub   | | Git      | | Mailing  | | IRC Log   |  |  |
|  |  | API      | | Repos    | | List     | | Scraper   |  |  |
|  |  | Adapter  | | Adapter  | | Adapter  | | Adapter   |  |  |
|  |  +----+-----+ +----+-----+ +-----+----+ +-----+-----+  |  |
|  |       |             |            |             |         |  |
|  |  +----------+ +-----------+                              |  |
|  |  | Delving  | | Optech /  |                              |  |
|  |  | Bitcoin  | | Specs     |                              |  |
|  |  | Adapter  | | Adapter   |                              |  |
|  |  +----+-----+ +-----+----+                              |  |
|  |       |              |                                   |  |
|  |       +------+-------+------+--------+--------+         |  |
|  |              v                                           |  |
|  |    +------------------+                                  |  |
|  |    |   Rate Limiter   |  Reads X-RateLimit-* headers.   |  |
|  |    |   (token bucket) |  Blocks acquire() when low.     |  |
|  |    +--------+---------+                                  |  |
|  +-------------+--------------------------------------------+  |
|                v                                              |
|  +--------------------------------------------------------+  |
|  |              Enrichment Pipeline                        |  |
|  |                                                        |  |
|  |  1. Cross-reference extractor                          |  |
|  |     (#1234, BIP-XXX, BOLT-YY, commit SHAs)            |  |
|  |  2. Concept tagger                                     |  |
|  |     (keyword matching against Optech topic vocabulary) |  |
|  |  3. Embedding generator                                |  |
|  |     (bge-base-en-v1.5 via ONNX Runtime, CPU)          |  |
|  +-------------+------------------------------------------+  |
|                v                                              |
|  +--------------------------------------------------------+  |
|  |              Storage Layer (SQLite)                      |  |
|  |                                                        |  |
|  |  +------------+  +----------+  +-------------------+   |  |
|  |  | documents  |  | FTS5     |  | vec_documents     |   |  |
|  |  | references |  | index    |  | (embedding vecs)  |   |  |
|  |  | concepts   |  |          |  |                   |   |  |
|  |  | change_log |  |          |  |                   |   |  |
|  |  | sync_state |  |          |  |                   |   |  |
|  |  +------------+  +----------+  +-------------------+   |  |
|  +-------------+------------------------------------------+  |
|                v                                              |
|  +--------------------------------------------------------+  |
|  |              Query API (HTTP/JSON via axum)              |  |
|  |                                                        |  |
|  |  GET /search        full-text + semantic search        |  |
|  |  GET /document/:id  single document with references    |  |
|  |  GET /references    cross-reference lookup              |  |
|  |  GET /bip/:number   BIP-specific context               |  |
|  |  GET /bolt/:number  BOLT-specific context              |  |
|  |  GET /timeline      concept timeline across sources    |  |
|  |  GET /changes       incremental change feed            |  |
|  +-------------+------------------------------------------+  |
+----------------+---------------------------------------------+
                 v
+--------------------------------------------------------------+
|                     MCP Server                                |
|                                                              |
|  Translates agent tool calls into API queries.               |
|  Implements the KnowledgeStore trait.                         |
|                                                              |
|  Backends:                                                   |
|    RemoteApiStore  -- HTTP client to BKB service (default)   |
|    LocalSqliteStore -- direct DB queries (integration tests) |
+--------------------------------------------------------------+
```

## 6. Data Model

### 6.1 Core Entity: `documents`

All content is normalized into a single `documents` table. The `source_type`
field distinguishes different kinds of content.

```sql
CREATE TABLE documents (
    -- Identity
    id              TEXT PRIMARY KEY,   -- "{source_type}:{source_repo}:{source_id}"
    source_type     TEXT NOT NULL,      -- see table below
    source_repo     TEXT,               -- "bitcoin/bitcoin", etc.
    source_id       TEXT NOT NULL,      -- issue number, commit SHA, Message-ID, etc.

    -- Content
    title           TEXT,
    body            TEXT,
    author          TEXT,
    author_id       TEXT,               -- GitHub user ID, email, etc.

    -- Timestamps
    created_at      TIMESTAMP NOT NULL,
    updated_at      TIMESTAMP,

    -- Hierarchy
    parent_id       TEXT,               -- FK to documents.id (comment -> issue, etc.)

    -- Source-specific metadata (JSON)
    metadata        TEXT,               -- labels, state, merge status, etc.

    -- Change tracking (internal ordering/debug; client change feed
    -- is a future phase)
    seq             INTEGER,            -- monotonic sequence from change_log

    UNIQUE(source_type, source_repo, source_id)
);

CREATE INDEX idx_documents_source ON documents(source_type, source_repo);
CREATE INDEX idx_documents_parent ON documents(parent_id);
CREATE INDEX idx_documents_author ON documents(author);
CREATE INDEX idx_documents_created ON documents(created_at);
CREATE INDEX idx_documents_seq ON documents(seq);
```

**Note on `url`:** The `documents` table does not store a URL. The canonical
URL for each document is derived at query time from `source_type`,
`source_repo`, and `source_id` (e.g., a `github_issue` with `source_repo`
`bitcoin/bitcoin` and `source_id` `12345` maps to
`https://github.com/bitcoin/bitcoin/issues/12345`).

**`source_type` values:**

| Value | Description | `source_id` format |
|---|---|---|
| `github_issue` | GitHub issue (not PR) | Issue number |
| `github_pr` | GitHub pull request | PR number |
| `github_comment` | Comment on issue/PR | Comment ID |
| `github_review` | PR review | Review ID |
| `github_review_comment` | Inline review comment | Comment ID |
| `github_discussion` | GitHub Discussion | Discussion number |
| `github_discussion_comment` | Discussion comment | Comment ID |
| `commit` | Git commit | Commit SHA |
| `mailing_list_msg` | Mailing list message | Message-ID |
| `irc_log` | IRC daily log | `{channel}:{YYYY-MM-DD}` |
| `delving_topic` | Delving Bitcoin topic (OP) | Topic ID |
| `delving_post` | Delving Bitcoin reply | Post ID |
| `bip` | Bitcoin Improvement Proposal | BIP number |
| `bolt` | BOLT specification | BOLT number |
| `blip` | Bitcoin Lightning Improvement Proposal | bLIP number |
| `optech_newsletter` | Optech newsletter | Newsletter number |
| `optech_topic` | Optech topic page | Topic slug |
| `optech_blog` | Optech blog post | Post slug |
| `bitcointalk_topic` | BitcoinTalk topic (OP) | Topic ID |
| `bitcointalk_post` | BitcoinTalk reply post | Message ID |

### 6.2 Full-Text Search Index

```sql
CREATE VIRTUAL TABLE documents_fts USING fts5(
    title,
    body,
    content=documents,
    content_rowid=rowid,
    tokenize='bkb'                  -- custom Bitcoin-aware tokenizer
);

-- Triggers to keep FTS in sync with documents table
CREATE TRIGGER documents_fts_insert AFTER INSERT ON documents BEGIN
    INSERT INTO documents_fts(rowid, title, body)
    VALUES (NEW.rowid, NEW.title, NEW.body);
END;

CREATE TRIGGER documents_fts_delete AFTER DELETE ON documents BEGIN
    INSERT INTO documents_fts(documents_fts, rowid, title, body)
    VALUES ('delete', OLD.rowid, OLD.title, OLD.body);
END;

CREATE TRIGGER documents_fts_update AFTER UPDATE ON documents BEGIN
    INSERT INTO documents_fts(documents_fts, rowid, title, body)
    VALUES ('delete', OLD.rowid, OLD.title, OLD.body);
    INSERT INTO documents_fts(rowid, title, body)
    VALUES (NEW.rowid, NEW.title, NEW.body);
END;
```

**Custom `bkb` tokenizer behavior:**

The tokenizer wraps `unicode61` and adds Bitcoin-specific rules:

- Keeps hyphenated identifiers as single tokens: `BIP-340`, `BIP-341`
- Keeps underscore-joined identifiers: `OP_CHECKSIG`, `OP_CAT`
- Splits CamelCase into additional tokens: `ChannelManager` also indexes
  `channel` and `manager`
- Indexes known synonyms: `HTLC` also indexes `hash time locked contract`,
  `tx` also indexes `transaction`

### 6.3 Cross-References

```sql
CREATE TABLE refs (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    from_doc_id TEXT NOT NULL,
    to_doc_id   TEXT,                -- NULL if target is unresolved
    ref_type    TEXT NOT NULL,       -- 'mentions_issue', 'mentions_pr', 'fixes',
                                    -- 'references_commit', 'references_bip',
                                    -- 'references_bolt', 'replies_to'
    to_external TEXT,                -- for unresolved: "bitcoin/bitcoin#1234", "BIP-340"
    context     TEXT,                -- surrounding text snippet

    FOREIGN KEY (from_doc_id) REFERENCES documents(id)
);

CREATE INDEX idx_refs_from ON refs(from_doc_id);
CREATE INDEX idx_refs_to ON refs(to_doc_id);
CREATE INDEX idx_refs_to_ext ON refs(to_external);
CREATE INDEX idx_refs_type ON refs(ref_type);
```

Cross-references are extracted during ingestion by regex-based pattern matching:

| Pattern | Extracted As |
|---|---|
| `#1234` | `mentions_issue` (in same repo) |
| `bitcoin/bitcoin#6789` | `mentions_issue` (cross-repo) |
| `BIP-340`, `BIP 340`, `bip340` | `references_bip` |
| `BOLT-11`, `BOLT 11`, `bolt11` | `references_bolt` |
| `bLIP-NN`, `blip-NN`, `bLIP NN` | `references_blip` |
| 7+ hex char sequences in commit context | `references_commit` |
| `Fixes #1234`, `Closes #1234` | `fixes` |

### 6.4 Bitcoin Concept Tags

```sql
-- Controlled vocabulary, seeded from Optech topics
CREATE TABLE concepts (
    slug        TEXT PRIMARY KEY,    -- 'taproot', 'channel-splicing', etc.
    name        TEXT NOT NULL,       -- 'Taproot', 'Channel Splicing'
    category    TEXT,                -- 'soft-fork', 'lightning', 'privacy', etc.
    aliases     TEXT                 -- JSON array: ["BIP-340", "BIP-341", "schnorr"]
);

CREATE TABLE concept_mentions (
    doc_id       TEXT NOT NULL,
    concept_slug TEXT NOT NULL,
    confidence   REAL DEFAULT 1.0,   -- 1.0 for explicit, <1.0 for inferred

    PRIMARY KEY (doc_id, concept_slug),
    FOREIGN KEY (doc_id) REFERENCES documents(id),
    FOREIGN KEY (concept_slug) REFERENCES concepts(slug)
);

CREATE INDEX idx_concept_mentions_concept ON concept_mentions(concept_slug);
```

The concept vocabulary is seeded from the Optech topics index, which provides
a curated taxonomy of ~150 Bitcoin/Lightning concepts with aliases. Additional
concepts can be added manually.

### 6.5 Embeddings

```sql
-- Vector similarity search via sqlite-vec extension
CREATE VIRTUAL TABLE vec_documents USING vec0(
    doc_id TEXT PRIMARY KEY,
    embedding FLOAT[768]             -- bge-base-en-v1.5 output dimension
);
```

For long documents, we embed at the chunk level:

```sql
CREATE TABLE embedding_chunks (
    chunk_id    TEXT PRIMARY KEY,     -- "{doc_id}:{chunk_index}"
    doc_id      TEXT NOT NULL,
    chunk_index INTEGER NOT NULL,
    chunk_text  TEXT NOT NULL,
    start_char  INTEGER,
    end_char    INTEGER,

    FOREIGN KEY (doc_id) REFERENCES documents(id)
);

CREATE INDEX idx_chunks_doc ON embedding_chunks(doc_id);
```

Embedding model: `bge-base-en-v1.5` (768 dimensions), run on CPU via ONNX
Runtime (`ort` crate). Initial embedding of ~2M chunks takes ~15-30 minutes
on CPU with batched inference (batch size ~256). Serial inference would take
significantly longer (~5-6 hours). Incremental updates (a few hundred chunks
per hour) are sub-second.

### 6.6 Change Tracking

```sql
CREATE TABLE change_log (
    seq         INTEGER PRIMARY KEY AUTOINCREMENT,
    doc_id      TEXT NOT NULL,
    change_type TEXT NOT NULL,        -- 'insert', 'update', 'delete'
    changed_at  TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);
```

Change tracking is handled at the application level (not SQL triggers) for
performance. Every upsert to `documents` also appends to `change_log` and
updates the document's `seq` field within the same transaction.

The `change_log` is periodically compacted: entries older than 30 days are
deleted. The change_log exists for internal use (ingestion ordering,
debugging, replay). Client-facing incremental sync via this table is
deferred to a future phase.

### 6.7 Sync State

```sql
CREATE TABLE sync_state (
    source_id      TEXT PRIMARY KEY,  -- "github:bitcoin/bitcoin:issues"
    source_type    TEXT NOT NULL,
    source_repo    TEXT,
    last_cursor    TEXT,              -- source-specific (timestamp, SHA, page URL)
    last_synced_at TIMESTAMP,
    next_run_at    TIMESTAMP,
    status         TEXT DEFAULT 'pending',  -- 'pending', 'running', 'ok', 'error'
    error_message  TEXT,
    retry_count    INTEGER DEFAULT 0,
    items_found    INTEGER DEFAULT 0  -- last sync cycle count (for adaptive scheduling)
);
```

## 7. Ingestion Pipeline

### 7.1 Job Queue

The job queue is a priority queue of `SyncJob` structs, sorted by `next_run`.
A single async task pops the highest-priority due job, executes one page of
work, and re-enqueues it. Initial sync and incremental sync use the same
codepath -- an initial sync is simply a large backlog of pages to fetch. If
interrupted, the cursor is saved and the job resumes from where it left off.

```rust
struct SyncJob {
    source_id: String,
    source: Box<dyn SyncSource>,
    priority: Priority,
    cursor: Option<String>,
    next_run: Instant,
    retry_count: u32,
}

enum Priority { High, Medium, Low }

#[async_trait]
trait SyncSource: Send + Sync {
    /// Fetch one page of updates. Returns documents + optional next cursor.
    async fn fetch_page(
        &self,
        cursor: Option<&str>,
        rate_limiter: &RateLimiter,
    ) -> Result<SyncPage>;

    /// Base polling interval for this source.
    fn poll_interval(&self) -> Duration;
}

struct SyncPage {
    documents: Vec<RawDocument>,
    references: Vec<RawReference>,
    next_cursor: Option<String>,     // None = page sequence complete
}
```

**Adaptive scheduling:** After each completed sync cycle, the interval is
adjusted based on activity:

- 0 items found: double the interval (capped at 4x base)
- 1-5 items: base interval
- 6+ items: halve the interval (floored at base/2)

### 7.2 Rate Limiter

```rust
struct RateLimiter {
    remaining: AtomicU32,       // from X-RateLimit-Remaining
    reset_at: AtomicU64,        // from X-RateLimit-Reset (unix timestamp)
    safety_margin: u32,         // stop when remaining < this (default: 200)
}

impl RateLimiter {
    /// Blocks until a request is permitted.
    async fn acquire(&self);

    /// Update state from GitHub response headers.
    fn update_from_response(&self, headers: &HeaderMap);
}
```

The rate limiter reads `X-RateLimit-Remaining` and `X-RateLimit-Reset` from
every GitHub API response. When remaining drops below the safety margin, it
sleeps until the reset time. This ensures the queue naturally spreads requests
across the hour without explicit static scheduling.

With a 5,000 req/hour limit and ~30 repos to track, typical hourly usage is
100-400 requests. The initial full sync for all repos (~3,000-5,000 requests)
completes in 2-4 hours automatically.

### 7.3 Source Adapters

| Adapter | API / Method | Cursor Type |
|---|---|---|
| `GitHubIssueSyncSource` | `GET /repos/{o}/{r}/issues?since=...&state=all&sort=updated` | `since` timestamp |
| `GitHubCommentSyncSource` | `GET /repos/{o}/{r}/issues/comments?since=...&sort=updated` | `since` timestamp |
| `GitHubReviewSyncSource` | `GET /repos/{o}/{r}/pulls/{n}/reviews` | PR number cursor |
| `GitHubDiscussionSyncSource` | GraphQL `repository.discussions` | GraphQL cursor |
| `GitCommitSyncSource` | `git fetch` + `git log` | Commit SHA |
| `MailingListSyncSource` | gnusha.org public-inbox Atom feed | Month + offset cursor |
| `MailArchiveSyncSource` | mail-archive.com HTML scraping | Sequential message number |
| `IrcLogSyncSource` | gnusha.org daily log file scraping | Date (YYYY-MM-DD) |
| `DelvingSyncSource` | Discourse REST API (`/latest.json`, `/t/{id}.json`) | Last activity timestamp |
| `OptechSyncSource` | `git fetch` + markdown file parsing | Commit SHA |
| `SpecSyncSource` | `git fetch` + spec file parsing (BIPs/BOLTs) | Commit SHA |
| `BitcointalkSyncSource` | HTML scraping (SMF 2.0) | Topic ID (sequential) / recent posts (tail mode) |

### 7.4 Enrichment Pipeline

After fetching raw documents, before storage:

```rust
trait Enricher: Send + Sync {
    fn enrich(&self, doc: &mut Document, raw_body: &str) -> Result<EnrichmentOutput>;
}

struct EnrichmentOutput {
    references: Vec<RawReference>,
    concept_tags: Vec<(String, f32)>,    // (concept_slug, confidence)
}
```

Concrete enrichers:

1. **`CrossReferenceExtractor`** -- regex-based extraction of issue refs,
   BIP/BOLT numbers, commit SHAs, cross-repo references.
2. **`ConceptTagger`** -- keyword/alias matching against the Optech concept
   vocabulary.
3. **`EmbeddingGenerator`** -- chunks text and runs through `bge-base-en-v1.5`
   via ONNX Runtime on CPU.

## 8. Query API

### 8.1 `GET /search`

Full-text and optional semantic search across all documents.

**Parameters:**

| Name | Type | Required | Description |
|---|---|---|---|
| `q` | string | yes | Search query |
| `source_type` | string | no | Comma-separated filter |
| `source_repo` | string | no | Comma-separated filter |
| `author` | string | no | Filter by author |
| `after` | string | no | Created after (ISO 8601) |
| `before` | string | no | Created before (ISO 8601) |
| `semantic` | bool | no | Enable embedding similarity (default: false) |
| `limit` | int | no | Max results (default: 20, max: 100) |

**Response:**

```json
{
  "results": [
    {
      "id": "github_issue:bitcoin/bitcoin:12345",
      "source_type": "github_issue",
      "source_repo": "bitcoin/bitcoin",
      "title": "Add package relay support",
      "snippet": "...matched text with highlights...",
      "author": "sdaftuar",
      "created_at": "2024-01-15T10:00:00Z",
      "score": 0.95,
      "url": "https://github.com/bitcoin/bitcoin/issues/12345",
      "concepts": ["package-relay", "mempool"]
    }
  ],
  "total_count": 42
}
```

### 8.2 `GET /document/{id}`

Single document with full content, outgoing references, incoming references,
and related concepts.

### 8.3 `GET /references/{entity}`

All documents referencing a given entity. The `entity` parameter can be:

- A BIP number: `BIP-340`
- A BOLT number: `BOLT-11`
- An issue reference: `bitcoin/bitcoin#12345`
- A commit SHA prefix
- A concept slug: `taproot`

### 8.4 `GET /bip/{number}`

BIP-specific context: spec text, all referencing issues/PRs/commits/mailing
list posts across all tracked repos, and the Optech summary if available.

### 8.5 `GET /bolt/{number}`

Same as above for BOLTs.

### 8.6 `GET /blip/{number}`

Same as above for bLIPs.

### 8.7 `GET /timeline/{concept}`

Chronological timeline of a concept across all sources:

```json
{
  "concept": "package-relay",
  "events": [
    {
      "date": "2019-05-22",
      "type": "mailing_list_msg",
      "title": "[bitcoin-dev] Package relay proposal"
    },
    {
      "date": "2020-02-14",
      "type": "github_issue",
      "title": "#18044: p2p: Package relay"
    },
    {
      "date": "2021-09-15",
      "type": "optech_newsletter",
      "title": "Newsletter #167: Package relay update"
    },
    {
      "date": "2023-11-02",
      "type": "github_pr",
      "title": "#28970: Package relay implementation"
    }
  ]
}
```

**Note on `bkb_find_commit`:** The MCP tool `bkb_find_commit` (Section 10)
does not have a dedicated HTTP endpoint. It is implemented as a specialized
`/search` query with `source_type=commit` and additional post-processing to
include associated PR and discussion context.

### 8.8 `GET /changes` *(Deferred -- Future Phase)*

> **Not implemented in Phases 1--4.** Documented here for future reference.
> The `change_log` table exists from day one for internal use, but this
> client-facing endpoint will be added when the client sync protocol is
> designed.

Incremental change feed for client sync.

**Parameters:**

| Name | Type | Required | Description |
|---|---|---|---|
| `since` | int | yes | Sequence number |
| `limit` | int | no | Max records (default: 1000) |

**Response:**

```json
{
  "changes": [
    {"seq": 50001, "doc_id": "...", "change_type": "insert", "document": {}},
    {"seq": 50002, "doc_id": "...", "change_type": "update", "document": {}}
  ],
  "next_seq": 50003,
  "has_more": false
}
```

## 9. MCP Server Tool Definitions

```json
[
  {
    "name": "bkb_search",
    "description": "Search the Bitcoin knowledge base across all sources (code, issues, PRs, commits, mailing lists, IRC logs, Delving Bitcoin, BIPs, BOLTs, bLIPs, Optech). Returns matching documents with snippets.",
    "parameters": {
      "query": "string, required -- search query",
      "source_type": "string, optional -- filter by type",
      "source_repo": "string, optional -- filter by repo",
      "author": "string, optional -- filter by author",
      "after": "string, optional -- created after (ISO 8601)",
      "before": "string, optional -- created before (ISO 8601)",
      "semantic": "boolean, optional -- use embedding similarity search",
      "limit": "integer, optional -- max results (default 20)"
    }
  },
  {
    "name": "bkb_get_document",
    "description": "Get full document by ID including content, cross-references, and related concepts.",
    "parameters": {
      "id": "string, required -- document ID"
    }
  },
  {
    "name": "bkb_get_references",
    "description": "Find all documents referencing a given entity (BIP, BOLT, issue, commit, or concept).",
    "parameters": {
      "entity": "string, required -- e.g. 'BIP-340', 'bitcoin/bitcoin#12345', commit SHA, or concept slug",
      "ref_type": "string, optional -- filter by reference type",
      "limit": "integer, optional -- max results (default 50)"
    }
  },
  {
    "name": "bkb_lookup_bip",
    "description": "Get comprehensive context for a BIP: spec text, all referencing discussions, PRs, and Optech summary.",
    "parameters": {
      "number": "integer, required -- BIP number"
    }
  },
  {
    "name": "bkb_lookup_bolt",
    "description": "Get comprehensive context for a BOLT: spec text, all referencing discussions, PRs, and Optech summary.",
    "parameters": {
      "number": "integer, required -- BOLT number"
    }
  },
  {
    "name": "bkb_lookup_blip",
    "description": "Get comprehensive context for a bLIP: spec text and all referencing documents.",
    "parameters": {
      "number": "integer, required -- bLIP number"
    }
  },
  {
    "name": "bkb_timeline",
    "description": "Chronological timeline of a concept across all sources: mailing list proposals, BIPs, implementation PRs, Optech coverage.",
    "parameters": {
      "concept": "string, required -- concept slug or search term",
      "after": "string, optional -- ISO 8601 date",
      "before": "string, optional -- ISO 8601 date"
    }
  },
  {
    "name": "bkb_find_commit",
    "description": "Find which commit(s) introduced a change, with the associated PR and discussion context.",
    "parameters": {
      "query": "string, required -- description of change, function name, or code pattern",
      "repo": "string, optional -- limit to a specific repo"
    }
  }
]
```

## 9.1 OpenAI Actions (ChatGPT)

In addition to the MCP server, the HTTP API serves an OpenAPI 3.0 specification
at `GET /openapi.json`. This enables integration with ChatGPT via Custom GPT
Actions -- ChatGPT imports the spec URL and can call all query endpoints
directly. No separate client binary is needed; the hosted API at
`https://bitcoinknowledge.dev/openapi.json` serves as the action schema.

## 10. `KnowledgeStore` Trait

```rust
#[async_trait]
pub trait KnowledgeStore: Send + Sync {
    async fn search(&self, params: SearchParams) -> Result<SearchResults>;
    async fn get_document(&self, id: &str) -> Result<Option<Document>>;
    async fn get_references(
        &self, entity: &str, ref_type: Option<&str>, limit: u32,
    ) -> Result<Vec<Reference>>;
    async fn lookup_bip(&self, number: u32) -> Result<BipContext>;
    async fn lookup_bolt(&self, number: u32) -> Result<BoltContext>;
    async fn lookup_blip(&self, number: u32) -> Result<BlipContext>;
    async fn timeline(
        &self, concept: &str, after: Option<DateTime>, before: Option<DateTime>,
    ) -> Result<Vec<TimelineEvent>>;
    async fn find_commit(
        &self, query: &str, repo: Option<&str>,
    ) -> Result<Vec<CommitContext>>;
    // Future: async fn get_changes(&self, since_seq: u64, limit: u32) -> Result<ChangeSet>;
    // Will be added when the client sync protocol is implemented.
}
```

**Implementations:**

- `RemoteApiStore` -- HTTP client that proxies to the BKB service. Default
  backend for the MCP server.
- `LocalSqliteStore` -- direct SQLite queries. Used for integration tests
  from Phase 1. User-facing local mode (where end users run a local DB
  with sync) is a future-phase goal.

## 11. Rust Crate Structure

```
bitcoin-knowledge-base/
|-- Cargo.toml                   (workspace root)
|-- docs/
|   `-- DESIGN.md
|
|-- bkb-core/                    (shared types, traits, schema)
|   |-- Cargo.toml
|   `-- src/
|       |-- lib.rs
|       |-- model.rs             -- Document, Reference, Concept, SearchParams, etc.
|       |-- store.rs             -- KnowledgeStore trait
|       |-- schema.rs            -- SQL schema definitions
|       `-- bitcoin.rs           -- Bitcoin-specific constants, concept vocabulary
|
|-- bkb-ingest/                  (ingestion pipeline + job queue)
|   |-- Cargo.toml
|   `-- src/
|       |-- lib.rs
|       |-- queue.rs             -- Job queue + scheduler
|       |-- rate_limiter.rs      -- GitHub API rate limiter
|       |-- enrichment.rs        -- Enrichment pipeline
|       |-- tokenizer.rs         -- Custom FTS5 tokenizer
|       `-- sources/
|           |-- mod.rs           -- SyncSource trait
|           |-- github.rs        -- Issues, comments, PRs, discussions
|           |-- commits.rs       -- Git commit walker (bare clone + revwalk)
|           |-- mailing_list.rs  -- gnusha.org public-inbox adapter
|           |-- mail_archive.rs  -- mail-archive.com HTML scraper
|           |-- irc.rs           -- IRC log scraper (gnusha.org)
|           |-- delving.rs       -- Delving Bitcoin (Discourse API)
|           |-- optech.rs        -- Optech markdown parser
|           `-- specs.rs         -- BIP/BOLT/bLIP parser
|
|-- bkb-store/                   (storage backends)
|   |-- Cargo.toml
|   `-- src/
|       |-- lib.rs
|       |-- sqlite.rs            -- SQLite + FTS5 + vec implementation
|       `-- migrations/          -- SQL migration files
|
|-- bkb-server/                  (HTTP API + ingestion runner)
|   |-- Cargo.toml
|   `-- src/
|       |-- main.rs              -- axum server + scheduler startup
|       |-- api.rs               -- Route handlers
|       `-- config.rs            -- Repo list, API tokens, intervals
|
`-- bkb-mcp/                     (MCP server for agents)
    |-- Cargo.toml
    `-- src/
        |-- main.rs              -- MCP stdio server
        |-- tools.rs             -- Tool definitions + handlers
        `-- remote_store.rs      -- RemoteApiStore implementation
```

## 12. Key Dependencies

| Crate | Purpose |
|---|---|
| `rusqlite` | SQLite with bundled FTS5 |
| `sqlite-vec` | Vector similarity search in SQLite |
| `ort` | ONNX Runtime for embedding inference (bge-base-en-v1.5, CPU) |
| `axum` | HTTP server |
| `reqwest` | HTTP client (GitHub API, Discourse API, mailing list archives) |
| `git2` | libgit2 bindings for git operations |
| `mailparse` | MIME/mbox email parsing |
| `pulldown-cmark` | Markdown parsing |
| `serde` / `serde_json` | Serialization |
| `tokio` | Async runtime |
| `rmcp` | Rust MCP SDK |
| `clap` | CLI argument parsing |
| `tracing` | Structured logging |

## 13. Implementation Roadmap

### Phase 1: Foundation (MVP) -- DONE

- Set up workspace and `bkb-core` types/traits
- SQLite schema + migrations in `bkb-store`
- FTS5 index (standard tokenizer initially)
- GitHub issue/PR/comment ingestion adapter
- Rate limiter + job queue
- HTTP API: `/search`, `/document/{id}`
- MCP server: `bkb_search`, `bkb_get_document`
- **Target:** single repo (`lightningdevkit/ldk-sample`) fully indexed and
  queryable end-to-end

### Phase 2: Full Source Coverage -- DONE

- Mailing list adapter (public-inbox Atom feed + raw email parsing)
- IRC log adapter (gnusha.org daily log scraping)
- Delving Bitcoin adapter (Discourse REST API)
- Optech newsletter adapter (GitHub contents API)
- BIP/BOLT/bLIP spec adapter (GitHub raw content API)
- Cross-reference extraction enricher (BIP, BOLT, bLIP, issue, Fixes/Closes)
- All target repos configured and ingesting
- API: `/references`, `/bip/{n}`, `/bolt/{n}`, `/blip/{n}`
- MCP: `bkb_get_references`, `bkb_lookup_bip`, `bkb_lookup_bolt`, `bkb_lookup_blip`

- Git commit adapter (`GitCommitSyncSource`) via `git2` (libgit2) bare clones
  with LRU cache management (`RepoCache`), incremental sync via `revwalk.hide()`,
  and truncated diffs (8 KB cap) for FTS5 searchability

### Phase 3: Bitcoin Intelligence -- DONE

- Concept vocabulary: 35 curated concepts seeded from Optech topics
  (`bkb-core/src/bitcoin.rs`), covering soft forks, scripting,
  transactions, Lightning, privacy, cryptography, and P2P
- Concept tagger enricher: word-boundary regex matching against concept
  aliases, storing matches in `concept_mentions` table
- API: `/timeline/{concept}`, `/find_commit`
- MCP: `bkb_timeline`, `bkb_find_commit`

**Deferred from Phase 3:** Custom Bitcoin-aware FTS5 tokenizer, embedding
generation via `ort`, and semantic search endpoint (see Section 13.5).

### Phase 4: Polish & Local Testing -- DONE

- Integration tests (9 tests exercising the full pipeline: ingest →
  enrich → store → query, covering cross-source scenarios)
- Enhanced `/health` endpoint with document counts by source type
- Per-source CLI testing via `--ingest-only` flag
- Change log compaction (`compact_change_log`)
- Adaptive poll intervals (implemented in job queue from Phase 1)

### 13.5 Deferred Items

The following items were deferred from their original phases. They are
documented here with rationale for deferral and guidance for future
implementation.

#### Git Commit Adapter -- DONE

Implemented in Phase 2. `GitCommitSyncSource` in
`bkb-ingest/src/sources/commits.rs` with `RepoCache` in
`bkb-ingest/src/repo_cache.rs`.

**Architecture:**
- **Bare clones** cached under `--cache-dir` (default `~/.cache/bkb/repos`)
- **LRU eviction** when cache exceeds `--max-cache-gb` (default 40 GB)
- **Repo size gate** via GitHub API `/repos/{owner}/{repo}` `size` field;
  repos exceeding `--max-repo-size-mb` (default 4 GB) are skipped
- **Incremental sync** via `git2::Revwalk::hide(cursor_oid)` -- only new
  commits since last sync are processed
- **Cursor persistence** in `{repo_path}/.bkb_cursor` (survives the job
  queue's cursor reset between cycles)
- **Truncated diff** (8 KB cap) appended to commit body for FTS5
  searchability and concept tagging
- **Associated PR lookup** in `find_commit`: searches PR bodies for the
  commit SHA prefix

#### Custom Bitcoin-Aware FTS5 Tokenizer

**Original phase:** Phase 3. **Reason for deferral:** Implementing a
custom FTS5 tokenizer requires the FTS5 tokenizer C API via `rusqlite`'s
unsafe FFI bindings. This is not a Rust trait -- it's a C callback
interface with raw pointers (`fts5_tokenizer` struct, `xTokenize`
callback). Doable but fiddly and error-prone. The standard `unicode61`
tokenizer works well enough for most queries, and the concept tagger
compensates for matching gaps (e.g., finding documents about "taproot"
even when they only mention "BIP-341").

**Implementation sketch:** Register a custom tokenizer via
`rusqlite::vtab` that wraps `unicode61` and adds:
- Keep hyphenated identifiers as single tokens: `BIP-340`, `BIP-341`
- Keep underscore-joined identifiers: `OP_CHECKSIG`, `OP_CAT`
- Split CamelCase into additional tokens: `ChannelManager` → `channel`,
  `manager`
- Index known synonyms: `HTLC` → `hash time locked contract`

#### Embedding Generation and Semantic Search

**Original phase:** Phase 3. **Reason for deferral:** Heaviest
dependency footprint of any feature. The `ort` crate requires ONNX
Runtime native libraries (~100 MB download). The `bge-base-en-v1.5`
model is ~400 MB. The `sqlite-vec` extension for vector similarity
search in SQLite needs additional build configuration. Initial embedding
of ~2M chunks would take 15--30 minutes even with batched inference.
This is the single most impactful remaining feature (fuzzy concept
matching vs. exact keyword matching), but it was deferred to keep the
initial build lean and fast.

**Implementation sketch:**
1. Add `ort` and `sqlite-vec` dependencies.
2. Download `bge-base-en-v1.5` ONNX model on first run (or bundle).
3. Implement text chunking (split long documents into ~512-token chunks,
   store in `embedding_chunks` table).
4. Implement `EmbeddingGenerator` enricher: for each chunk, run
   inference → store 768-dim vector in `vec_documents`.
5. Modify `/search` to accept `semantic=true`: run the query through the
   same embedding model, then combine FTS5 BM25 scores with cosine
   similarity scores from `sqlite-vec`.

#### Client Sync Protocol

**Original phase:** Future. **Reason for deferral:** Protocol design
question requiring careful consideration of compaction policy, cursor
management, conflict resolution for local DB mode, and bandwidth
optimization. The `change_log` table and `seq` field exist from day one
for internal use, but exposing them as a client-facing API requires
defining a stable sync contract.

- `/changes` API endpoint (Section 8.7)
- `get_changes()` method on `KnowledgeStore` trait
- Incremental sync client (`bkb sync`)
- Client cursor tracking in `change_log` compaction policy
- `LocalSqliteStore` as a full user-facing backend with sync

## 14. Testing Strategy

### 14.1 Test Tiers

The project uses three tiers of tests, each with increasing scope and
external dependencies:

**Tier 1: Unit tests** (`cargo test`)

No network, no database. These test individual components in isolation:

- Cross-reference extraction (regex patterns against known input strings)
- Concept tagging (keyword matching logic)
- Rate limiter state machine (token bucket behavior, sleep calculations)
- Job queue scheduling (priority ordering, adaptive interval adjustment)
- GitHub/Discourse API response parsing (deserialization of fixture JSON)

All external inputs are provided as hardcoded fixtures: real API responses
saved as JSON files in `tests/fixtures/` directories within each crate.

**Tier 2: Integration tests** (`cargo test`)

Spin up an in-memory SQLite database, run migrations, and test the full
data path:

- Insert synthetic documents, verify FTS5 search returns correct results
- Test the complete ingestion-to-storage pipeline by feeding pre-recorded
  API responses through: adapter parse → enrichment → store upsert
- Test the HTTP API handlers against a real (in-memory) store
- Test cross-reference resolution (insert referenced documents, verify links)
- Test upsert/overwrite behavior (simulating edited documents)

These tests are fast, deterministic, and require no network access. They
run in CI on every commit.

**Tier 3: Live smoke tests** (opt-in: `BKB_LIVE_TESTS=1 cargo test`)

Hit real external APIs with a minimal scope to validate that parsing and
pagination work against actual endpoints. Gated behind the `BKB_LIVE_TESTS`
environment variable so they never run in CI accidentally. Requires a
`GITHUB_TOKEN` environment variable for GitHub API access.

Scope:

| Source | Smoke test scope | Est. API calls |
|---|---|---|
| GitHub | `lightningdevkit/ldk-sample` (~50 issues) | ~10 requests |
| Delving Bitcoin | 5 most recent topics | ~6 requests |
| IRC | 1 channel, 1 day of logs | 1 HTTP fetch |
| Mailing list | Last page of archive | 1 HTTP fetch |
| BIPs | First 3 BIPs only | 3 file reads |
| Optech | Last 3 newsletters | 3 file reads |

Total runtime: under 2 minutes.

### 14.2 Recorded Fixtures for CI

To keep integration tests realistic without hitting the network:

1. A `record-fixtures` binary (in `bkb-ingest/examples/`) fetches real API
   responses and saves them as JSON files in `tests/fixtures/`.
2. These fixture files are committed to the repository.
3. Integration tests deserialize fixtures through the same parsing codepath
   that the live adapters use, ensuring we test against real response shapes.
4. Fixtures are refreshed periodically (or when an API response format
   changes) by re-running the recorder.

### 14.3 Development Subset Mode (`--dev-subset`)

Full initial sync of all sources takes 4-6 hours. For development iteration,
the `bkb-server` binary accepts a `--dev-subset` flag (or `BKB_DEV_SUBSET=1`
env var) that restricts ingestion to a small curated set of sources:

| Source | Dev subset scope | Est. items | Est. sync time |
|---|---|---|---|
| GitHub | `lightningdevkit/ldk-sample` | ~50 issues/PRs | ~30 seconds |
| Delving Bitcoin | 5 most recent topics | ~50 posts | ~10 seconds |
| IRC | 1 channel, last 7 days | 7 daily logs | ~5 seconds |
| Mailing list | Last 30 messages | 30 messages | ~10 seconds |
| BIPs | BIP-340 through BIP-344 | 5 specs | ~5 seconds |
| Optech | Last 5 newsletters | 5 newsletters | ~10 seconds |

This gives a fully functional end-to-end system with real data in under
2 minutes. Every query path (search, document lookup, references, timeline)
can be exercised against this subset.

### 14.4 Per-Source Testing During Development

Each source adapter is developed and tested independently:

1. **Implement adapter** with unit tests against fixtures.
2. **Run adapter in isolation** via a CLI subcommand:
   `bkb-server ingest --source github --repo lightningdevkit/ldk-sample --limit 10`
   This runs only the specified adapter and stores results in the database.
3. **Verify via API** by querying the HTTP server against the ingested data.
4. **Add to dev-subset** once the adapter is stable.

This means you never need to wait for all sources to sync just to test one
adapter.

### 14.5 Initial Sync Time Estimates (Full Production)

| Source | Est. API calls | Est. time |
|---|---|---|
| GitHub (all repos) | ~3,000-5,000 | 2-4 hours |
| Delving Bitcoin | ~500 | ~10 minutes |
| IRC logs (full history) | ~5,000 daily logs | ~30 minutes |
| Mailing lists | ~200 archive pages | ~20 minutes |
| BIPs + BOLTs (git clone) | N/A (local parse) | ~2 minutes |
| Optech (git clone) | N/A (local parse) | ~5 minutes |
| **Total** | | **~4-6 hours** |

After initial sync, hourly incremental updates typically complete in
under 60 seconds across all sources.

## 15. Storage Size Estimates

| Component | Size |
|---|---|
| Raw document text | ~2-2.5 GB |
| SQLite DB with indexes | ~3 GB |
| FTS5 index | ~4-6 GB |
| Embedding vectors (768-dim, ~2M chunks) | ~3-4 GB |
| Change log (30-day rolling window) | ~50 MB |
| **Total** | **~11-14 GB** |

## 16. Development Workflow

### 16.1 Commit Discipline

Each logical step should be an individual git commit. Every commit must
build (`cargo check`) and pass tests (`cargo test`) independently -- no
"fix it in the next commit" allowed. This ensures bisectability and clean
review history.

### 16.2 Code Formatting

All Rust code must be formatted with `cargo fmt` before committing. All
crates in the workspace share a single `rustfmt.toml` at the workspace
root, adopted from
[ldk-node's `rustfmt.toml`](https://github.com/lightningdevkit/ldk-node/blob/main/rustfmt.toml):

```toml
use_small_heuristics = "Max"
fn_params_layout = "Compressed"
hard_tabs = true
use_field_init_shorthand = true
max_width = 100
match_block_trailing_comma = true
format_code_in_doc_comments = true
comment_width = 100
format_macro_matchers = true
group_imports = "StdExternalCrate"
reorder_imports = true
imports_granularity = "Module"
normalize_comments = true
normalize_doc_attributes = true
style_edition = "2021"
```

Key settings: hard tabs for indentation, 100-character line width, grouped
and reordered imports (std -> external -> crate), compressed function
parameters, and formatted doc comments.
