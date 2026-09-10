# Cross-forge ingestion

Development moved without moving every conversation. BKB therefore continues
polling GitHub while ingesting Forgejo, and keeps source records separate from
their canonical presentation. This is an additive database upgrade; no GitHub
IDs or documents are renamed or removed.

## Repository configuration

The explicit pairs in `bkb-core/src/repository.rs` are:

| Logical repository / GitHub repository | Primary forge |
| --- | --- |
| lightningdevkit/rust-lightning | git.rust-bitcoin.org |
| lightningdevkit/ldk-sample | git.rust-bitcoin.org |
| rust-bitcoin/rust-secp256k1 | git.rust-bitcoin.org |
| rust-bitcoin/rust-psbt | git.rust-bitcoin.org |
| rust-bitcoin/corepc | git.rust-bitcoin.org |
| rust-bitcoin/bitcoin-payment-instructions | git.rust-bitcoin.org |

The development subset includes only ldk-sample. Repository names alone never
establish a pair. The BDK repositories found on the forge were Git mirrors,
not confirmed development migrations, and are not included.

GitHub retains issue/comment polling and gains review-summary and inline-review
feeds for these repositories. Forgejo supplies issues, PR descriptions/details,
conversation comments, review summaries, and inline review comments/replies.
Empty submitted reviews are retained; unpublished reviews are excluded.

There is one Git history stream per logical repository. It fetches the primary
forge, keeps existing `commit:owner/repo:sha` IDs, and links to that forge.
Git pagination pins a head and traversal offset until the complete snapshot has
been committed. Legacy SHA checkpoints are backfilled once because the former
pagination could revisit newer commits while skipping older history.
Replaying Git history inserts missing commits and retains existing commit
records, historical references, concept scores, and change-log sequences.
Stored metadata URLs are preserved; newly imported commits link to the primary
forge. Issue and PR feeds continue refreshing mutable source records.

## Source and canonical identities

Examples:

| Source record | Raw ID |
| --- | --- |
| GitHub issue | `github_issue:lightningdevkit/rust-lightning:4760` |
| Forgejo issue | `forgejo_issue:git.rust-bitcoin.org/lightningdevkit/rust-lightning:4866` |
| Forgejo PR | `forgejo_pr:git.rust-bitcoin.org/lightningdevkit/rust-lightning:4760` |

These examples reflect a real collision: GitHub issue #4760 is mirrored as
Forgejo issue #4866, while Forgejo PR #4760 is unrelated. Never match by number
alone. PRs were imported historically but are not continuously mirrored.

`documents` and its FTS index retain normalized raw source content.
`document_links` records equivalence evidence, including targets not yet
available. `document_aliases` is a replaceable canonical projection.
Equivalences come from:

1. A complete, trusted issue-mirror envelope: matching header URL, final
   unquoted v1 marker, repository, origin, and numeric identifiers. Trust is
   scoped to rust-lightning and the observed account ID **and** login:
   GitHub `ldk-reviews-bot` / `199857858`, Forgejo
   `github-comment-mirror` / `72`.
2. Conservative historical-import fingerprints within a configured pair.
   Threads need the same kind, number, creation timestamp, title, and nonempty
   body. Comments/reviews need a verified containing-thread mapping, exact
   timestamp and nonempty body, compatible available diff context, and a
   unique one-to-one candidate. CRLF is normalized; prose is not fuzzily matched.

Quoted bot envelopes, ordinary users posting markers, mismatched URLs, duplicate
marker keys, and trailing human replies are not mirror evidence. A later
duplicate fingerprint invalidates the now-ambiguous historical match.
Unrecognized formats, edited imports without prior evidence, and empty reviews
remain separate. There is no global username/account merge.

Threads prefer Forgejo's location and state. Explicit mirrors use the original
record's text, author, and creation time once it is available. Historical
comments prefer the GitHub original. Each canonical result exposes source
IDs, URLs, account IDs, timestamps, and the matching method in `provenance`.
When an origin is missing, the copy remains searchable; its verified envelope
can be stripped for presentation, but no original timestamp is invented.
Regular polling and the weekly repair scan retry missing origins.

## Query behavior

- Search deduplicates verified copies **before** applying the result limit.
  The best matching raw row supplies the score/snippet, and
  `matched_source_id` identifies it. Duplicate scores are not added together.
- `deduplicate=false` returns raw search hits and disables logical-repository
  expansion. MCP `bkb_search` accepts the same boolean.
- `source_repo=lightningdevkit/rust-lightning` includes both configured
  source repositories. Explicit `github.com/owner/repo` or
  `git.rust-bitcoin.org/owner/repo` selects that source's membership.
  Source-type, author, and date filters can match a
  member of a canonical group, even when its representative has another type.
- `GET /document/{old-github-id}` resolves a verified alias. Append
  `?raw=true`, or use `bkb_get_document` with `raw: true`, to inspect the
  exact source record, including the original bot wrapper.
- Document references and concept timelines combine verified copies.
  Legacy `owner/repo#N` reference keys retain their GitHub meaning;
  Forgejo keys are `git.rust-bitcoin.org/owner/repo#N`.
- Explicit issue, PR, comment, review, and commit links retain their host.
  Forgejo `#issuecomment-N` links resolve by a unique stored permalink:
  ordinary comments, inline comments, and review summaries share that anchor
  format, and a review's API ID can differ from the anchor. Missing or
  ambiguous targets remain unresolved until ingestion or backfill finds
  sufficient evidence. Historical guessed targets are repaired in place.
  Shorthand inside a recognized mirror is interpreted at the **original**
  host. Bare `#N` in migrated shared Git history is ambiguous and is not
  assigned to either forge. Commit lookup uses merge SHA metadata and explicit
  SHA mentions from PRs on both hosts.

Raw source types are preserved, including the new `forgejo_issue`,
`forgejo_pr`, `forgejo_comment`, `forgejo_review`, and
`forgejo_review_comment` types.

## Backfill and operations

Back up an existing database before deploying. Start the normal server to
register both forge feeds and the local `reconcile:v1` upgrade job. It reads
200 raw Git/GitHub/Forgejo records per page and checkpoints the stable SQLite
row ID. Backfill reads the current records inside its transaction and adds
missing references, concept assignments, and equivalence evidence. It retains
existing raw values, change-log sequences, historical references, and custom
concept scores, including when ingestion updates a record after page fetch.
Newly fetched pages reconcile in their own transaction. API-only mode does
not run backfills.

For bounded checks, use a disposable database and a task-specific Cargo target
directory:

```sh
export CARGO_TARGET_DIR="$(mktemp -d /tmp/cargo-target-bkb-forgejo.XXXXXX)"
cargo run -p bkb-server -- --db /tmp/bkb-forgejo-check.db \
  --ingest-only forgejo:lightningdevkit/ldk-sample:issues --limit-pages 3
cargo run -p bkb-server -- --db /tmp/bkb-forgejo-check.db \
  --ingest-only forgejo:lightningdevkit/ldk-sample:comments --limit-pages 3
cargo run -p bkb-server -- --db /tmp/bkb-forgejo-check.db \
  --ingest-only forgejo:lightningdevkit/ldk-sample:reviews --limit-pages 10
cargo run -p bkb-server -- --db /tmp/bkb-forgejo-check.db \
  --ingest-only github-reviews:lightningdevkit/ldk-sample --limit-pages 10
cargo run -p bkb-server -- --db /tmp/bkb-forgejo-check.db \
  --ingest-only github-review-comments:lightningdevkit/ldk-sample --limit-pages 10
cargo run -p bkb-server -- --db /tmp/bkb-forgejo-check.db \
  --ingest-only reconcile --limit-pages 10
```

Repeated invocations resume the persisted page or begin the next polling
window after a completed scan. Do not run competing writers for the same feed.
`--limit-pages` counts requests/pages, including PR-detail and review phases,
not just pages containing documents.

Forgejo feeds and Git repository metadata checks share an independent
one-request-per-second budget,
explicit pages, the instance's 50-item limit, timeouts, no redirects, and
Retry-After deferrals. `FORGEJO_TOKEN` is optional and is sent only to the
configured Forgejo API; `GITHUB_TOKEN` is not reused there. The GitHub feeds
honor their GitHub quota. Polling windows overlap by five minutes; a weekly
full scan repairs changed reviews and items displaced by mutable pagination.
Source APIs do not provide immutable snapshots, so this is eventual
convergence, not a guarantee against every concurrent page movement.

Each successful page commits documents, references, concepts, equivalences, and
its checkpoint together. Failed writes do not advance the cursor. Missing
external records never cause automatic local deletion. Failed Git fetches
retain the cached repository.

Existing admin reset/re-enrichment endpoints understand the new source types.
Re-enrichment uses raw source context and replaces a document's derived edges
atomically. Resetting a Forgejo type resets its corresponding feed checkpoint,
not the GitHub counterpart. Normal reads still work for unpaired records.

The authenticated metrics endpoint adds:

- `bkb_raw_documents` and `bkb_canonical_documents`
- `bkb_verified_links` (recorded evidence, including unresolved targets)
- `bkb_unresolved_links` (missing targets or conflicting mappings)
- `bkb_source_window_lag_seconds{source="..."}` where a polling watermark exists

To rebuild the projection, stop ingestion, back up the database, clear only
`document_aliases` and the `reconcile:v1` sync-state row, then run the
reconciliation backfill. Retain `document_links` to preserve evidence for
imports whose bodies subsequently changed. Removing those links deliberately
discards that evidence. Raw documents and their IDs need not be changed.

## Validation and live-access limitation

Tests cover atomic page failure, equal-timestamp pagination, nested review
resume, deleted/empty review data, commit pagination, collision separation,
bot spoof/quote rejection, renumbered mirrors, late origins and parents,
ambiguous fingerprints, original edits, raw queries, timelines, merge SHAs,
references, database reopening, REST, and MCP parameter forwarding.

During the September 2026 bounded live check, this instance returned a
human-verification HTML page (HTTP 403) to the truthful `bkb/0.2` user agent.
A curl diagnostic received public JSON; that does **not** establish that the
BKB client can ingest successfully. No Forgejo token was configured. The
adapter surfaced the error and did not advance its checkpoint. Deployment
requires arranging access for BKB with the instance operator, or testing a
suitable token; token access has not been verified. The challenge is not
bypassed and the client does not impersonate another user agent.

## Primary sources

- [Migrated rust-lightning repository](https://git.rust-bitcoin.org/lightningdevkit/rust-lightning)
- [Forgejo API usage](https://forgejo.org/docs/latest/user/api/usage/)
- [Instance OpenAPI schema](https://git.rust-bitcoin.org/swagger.v1.json)
- [Instance API settings](https://git.rust-bitcoin.org/api/v1/settings/api)
- [GitHub pull-request reviews](https://docs.github.com/en/rest/pulls/reviews)
- [GitHub review comments](https://docs.github.com/en/rest/pulls/comments)
- [GitHub issue 4760](https://github.com/lightningdevkit/rust-lightning/issues/4760)
- [Mirrored Forgejo issue 4866](https://git.rust-bitcoin.org/lightningdevkit/rust-lightning/issues/4866)
- [Unrelated Forgejo PR 4760](https://git.rust-bitcoin.org/lightningdevkit/rust-lightning/pulls/4760)
