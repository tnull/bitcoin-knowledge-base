//! Integration tests exercising the full data pipeline:
//! ingestion -> enrichment -> storage -> query.

use chrono::{TimeZone, Utc};

use bkb_core::model::{Document, RefType, SearchParams, SourceType};
use bkb_core::store::KnowledgeStore;
use bkb_ingest::enrichment::enrich;
use bkb_store::sqlite::SqliteStore;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a test document with the given fields.
fn make_doc(
	source_type: SourceType, source_repo: Option<&str>, source_id: &str, title: &str, body: &str,
	author: &str, created_at: chrono::DateTime<Utc>,
) -> Document {
	let id = Document::make_id(&source_type, source_repo, source_id);
	Document {
		id,
		source_type,
		source_repo: source_repo.map(String::from),
		source_id: source_id.to_string(),
		title: Some(title.to_string()),
		body: Some(body.to_string()),
		author: Some(author.to_string()),
		author_id: None,
		created_at,
		updated_at: None,
		parent_id: None,
		metadata: None,
		seq: None,
	}
}

/// Insert a document, run enrichment, and persist the resulting refs and
/// concept tags.
async fn ingest_and_enrich(store: &SqliteStore, doc: &Document) {
	store.upsert_document(doc).await.unwrap();

	if let Some(body) = doc.body.as_deref() {
		let output = enrich(&doc.id, body, doc.source_repo.as_deref());

		for r in &output.references {
			store.insert_reference(r).await.unwrap();
		}
		for (slug, confidence) in &output.concept_tags {
			store.upsert_concept_mention(&doc.id, slug, *confidence).await.unwrap();
		}
	}
}

/// Create an in-memory store and seed it with four realistic documents that
/// cross-reference each other.
async fn seeded_store() -> SqliteStore {
	let store = SqliteStore::open_in_memory().unwrap();

	// 1. GitHub issue mentioning BIP-340 schnorr and referencing #5678
	let issue = make_doc(
		SourceType::GithubIssue,
		Some("bitcoin/bitcoin"),
		"9999",
		"Implement BIP-340 schnorr signature validation",
		"This PR implements BIP-340 schnorr signatures for taproot key-path \
		 spending. It also addresses the concern raised in #5678 about \
		 signature aggregation with MuSig2. See BIP-341 for the full \
		 taproot specification.",
		"sipa",
		Utc.with_ymd_and_hms(2023, 6, 15, 12, 0, 0).unwrap(),
	);
	ingest_and_enrich(&store, &issue).await;

	// 2. BIP document: BIP-340 Schnorr Signatures
	let bip340 = make_doc(
		SourceType::Bip,
		None,
		"340",
		"BIP-340: Schnorr Signatures for secp256k1",
		"This document proposes a standard for 64-byte schnorr signatures \
		 over the elliptic curve secp256k1. Schnorr signatures offer \
		 advantages over ECDSA including provable security, non-malleability, \
		 and linearity which enables multisig schemes like MuSig2.",
		"Pieter Wuille",
		Utc.with_ymd_and_hms(2020, 1, 19, 0, 0, 0).unwrap(),
	);
	ingest_and_enrich(&store, &bip340).await;

	// 3. Mailing list message discussing taproot activation and BIP-341
	let ml_msg = make_doc(
		SourceType::MailingListMsg,
		None,
		"taproot-activation-2021-04",
		"Taproot activation discussion",
		"The taproot soft fork (BIP-341) is ready for activation. The \
		 changes include schnorr signature validation (BIP-340), tapscript \
		 (BIP-342), and the overall taproot consensus rules. Speedy trial \
		 has been proposed as the activation mechanism.",
		"aj",
		Utc.with_ymd_and_hms(2021, 4, 10, 18, 30, 0).unwrap(),
	);
	ingest_and_enrich(&store, &ml_msg).await;

	// 4. Delving Bitcoin topic about OP_CTV covenants
	let delving = make_doc(
		SourceType::DelvingTopic,
		None,
		"op-ctv-covenants-review",
		"OP_CTV covenants: use cases and review",
		"This topic covers OP_CTV (BIP-119) covenant proposals. Covenants \
		 restrict how coins can be spent in future transactions. OP_CTV \
		 enables congestion control, vaults, and payment pools without \
		 requiring interactive signing.",
		"jamesob",
		Utc.with_ymd_and_hms(2024, 2, 1, 9, 0, 0).unwrap(),
	);
	ingest_and_enrich(&store, &delving).await;

	store
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Search for "schnorr" and verify results come back with correct concept
/// tags.
#[tokio::test]
async fn test_search_returns_results_with_concept_tags() {
	let store = seeded_store().await;

	let results = store
		.search(SearchParams { query: "schnorr".to_string(), ..Default::default() })
		.await
		.unwrap();

	// At minimum the github_issue and bip340 documents mention "schnorr"
	assert!(
		results.results.len() >= 2,
		"expected at least 2 results for 'schnorr', got {}",
		results.results.len()
	);

	// Every result that mentions schnorr should have the "taproot" concept
	// tag (because "schnorr" is an alias for the taproot concept).
	for r in &results.results {
		assert!(
			r.concepts.contains(&"taproot".to_string()),
			"result '{}' should have 'taproot' concept tag, got {:?}",
			r.id,
			r.concepts
		);
	}
}

/// Get a document by ID and verify outgoing refs are populated.
#[tokio::test]
async fn test_get_document_has_outgoing_refs() {
	let store = seeded_store().await;

	let issue_id = Document::make_id(&SourceType::GithubIssue, Some("bitcoin/bitcoin"), "9999");

	let ctx = store.get_document(&issue_id).await.unwrap().expect("document should exist");

	assert_eq!(ctx.document.source_type, SourceType::GithubIssue);
	assert_eq!(ctx.document.author.as_deref(), Some("sipa"));

	// The issue body mentions BIP-340, BIP-341, and #5678, so we expect
	// at least one `ReferencesBip` and at least one `MentionsIssue`.
	let bip_refs: Vec<_> =
		ctx.outgoing_refs.iter().filter(|r| r.ref_type == RefType::ReferencesBip).collect();
	assert!(
		bip_refs.len() >= 2,
		"expected >=2 BIP refs (BIP-340, BIP-341), got {}",
		bip_refs.len()
	);

	let issue_refs: Vec<_> =
		ctx.outgoing_refs.iter().filter(|r| r.ref_type == RefType::MentionsIssue).collect();
	assert!(!issue_refs.is_empty(), "expected at least one issue mention (#5678)");

	// Concepts should include taproot (due to schnorr/BIP-340/BIP-341)
	// and musig2 (mentioned explicitly).
	assert!(
		ctx.concepts.contains(&"taproot".to_string()),
		"concepts should include 'taproot', got {:?}",
		ctx.concepts
	);
	assert!(
		ctx.concepts.contains(&"musig2".to_string()),
		"concepts should include 'musig2', got {:?}",
		ctx.concepts
	);
}

/// Look up BIP-340 by number and verify incoming refs from other documents.
#[tokio::test]
async fn test_lookup_bip_with_incoming_refs() {
	let store = seeded_store().await;

	let ctx = store.lookup_bip(340).await.unwrap().expect("BIP-340 should exist");

	assert_eq!(ctx.document.source_type, SourceType::Bip);
	assert_eq!(ctx.document.source_id, "340");
	assert_eq!(ctx.document.title.as_deref(), Some("BIP-340: Schnorr Signatures for secp256k1"));

	// The github_issue and the mailing_list_msg both reference BIP-340,
	// so we should have at least 2 incoming refs.
	assert!(
		ctx.incoming_refs.len() >= 2,
		"expected >=2 incoming refs to BIP-340, got {} ({:?})",
		ctx.incoming_refs.len(),
		ctx.incoming_refs.iter().map(|r| &r.from_doc_id).collect::<Vec<_>>()
	);

	// Verify the incoming refs are of type `ReferencesBip`.
	for r in &ctx.incoming_refs {
		assert_eq!(r.ref_type, RefType::ReferencesBip, "incoming ref should be ReferencesBip");
	}
}

/// Get references for an entity (e.g., "BIP-340") and verify they are found.
#[tokio::test]
async fn test_get_references_for_entity() {
	let store = seeded_store().await;

	let refs = store.get_references("BIP-340", None, 50).await.unwrap();

	// The github_issue and mailing_list_msg both produce a `ReferencesBip`
	// ref with `to_external = "BIP-340"`.
	assert!(refs.len() >= 2, "expected >=2 refs pointing to BIP-340, got {}", refs.len());

	for r in &refs {
		assert_eq!(r.to_external.as_deref(), Some("BIP-340"));
		assert_eq!(r.ref_type, RefType::ReferencesBip);
	}

	// Also verify we can filter by ref_type.
	let bip_only = store.get_references("BIP-340", Some("references_bip"), 50).await.unwrap();
	assert_eq!(bip_only.len(), refs.len());
}

/// Query the timeline for the "taproot" concept and verify chronological
/// events spanning multiple source types.
#[tokio::test]
async fn test_timeline_for_concept() {
	let store = seeded_store().await;

	let timeline = store.timeline("taproot", None, None).await.unwrap();

	assert_eq!(timeline.concept, "taproot");

	// BIP-340 (2020), mailing_list_msg (2021), and github_issue (2023)
	// all mention taproot-related aliases.
	assert!(
		timeline.events.len() >= 3,
		"expected >=3 taproot timeline events, got {}",
		timeline.events.len()
	);

	// Events must be in chronological order.
	for window in timeline.events.windows(2) {
		assert!(
			window[0].date <= window[1].date,
			"timeline events out of order: {} > {}",
			window[0].date,
			window[1].date
		);
	}

	// Verify that different source types appear in the timeline.
	let types: Vec<&SourceType> = timeline.events.iter().map(|e| &e.source_type).collect();
	assert!(types.contains(&&SourceType::Bip), "timeline should include a BIP event");
	assert!(
		types.contains(&&SourceType::GithubIssue),
		"timeline should include a GithubIssue event"
	);
}

/// Verify that the OP_CTV / covenants document is correctly tagged with
/// both the `op-checktemplateverify` and `covenants` concepts.
#[tokio::test]
async fn test_covenant_concepts_tagged() {
	let store = seeded_store().await;

	let delving_id = Document::make_id(&SourceType::DelvingTopic, None, "op-ctv-covenants-review");

	let ctx = store.get_document(&delving_id).await.unwrap().expect("delving topic should exist");

	assert!(
		ctx.concepts.contains(&"op-checktemplateverify".to_string()),
		"expected 'op-checktemplateverify' concept, got {:?}",
		ctx.concepts
	);
	assert!(
		ctx.concepts.contains(&"covenants".to_string()),
		"expected 'covenants' concept, got {:?}",
		ctx.concepts
	);

	// The body references BIP-119, so there should be an outgoing BIP ref.
	let bip_refs: Vec<_> =
		ctx.outgoing_refs.iter().filter(|r| r.ref_type == RefType::ReferencesBip).collect();
	assert!(!bip_refs.is_empty(), "expected at least one BIP ref (BIP-119) from delving topic");
	let bip119_ref = bip_refs.iter().find(|r| r.to_external.as_deref() == Some("BIP-119"));
	assert!(
		bip119_ref.is_some(),
		"expected a ref to BIP-119, got {:?}",
		bip_refs.iter().map(|r| &r.to_external).collect::<Vec<_>>()
	);
}

/// Verify that re-enrichment (delete + re-insert) works correctly.
#[tokio::test]
async fn test_re_enrichment_replaces_old_data() {
	let store = SqliteStore::open_in_memory().unwrap();

	let doc = make_doc(
		SourceType::GithubIssue,
		Some("bitcoin/bitcoin"),
		"42",
		"Initial taproot issue",
		"This implements BIP-340 schnorr signatures.",
		"satoshi",
		Utc::now(),
	);

	// First enrichment pass.
	ingest_and_enrich(&store, &doc).await;
	let ctx = store.get_document(&doc.id).await.unwrap().unwrap();
	assert!(ctx.concepts.contains(&"taproot".to_string()));
	let old_ref_count = ctx.outgoing_refs.len();
	assert!(old_ref_count > 0);

	// Simulate re-enrichment with updated body text.
	let mut updated = doc.clone();
	updated.body =
		Some("Updated: now covers silent payments (BIP-352) and payjoin (BIP-78).".to_string());
	store.upsert_document(&updated).await.unwrap();

	// Clear old enrichment data.
	store.delete_refs_from(&doc.id).await.unwrap();
	store.delete_concept_mentions(&doc.id).await.unwrap();

	// Re-enrich with new body.
	let output = enrich(&doc.id, updated.body.as_deref().unwrap(), updated.source_repo.as_deref());
	for r in &output.references {
		store.insert_reference(r).await.unwrap();
	}
	for (slug, confidence) in &output.concept_tags {
		store.upsert_concept_mention(&doc.id, slug, *confidence).await.unwrap();
	}

	let ctx = store.get_document(&doc.id).await.unwrap().unwrap();

	// Old concept (taproot) should be gone; new concepts should be present.
	assert!(
		!ctx.concepts.contains(&"taproot".to_string()),
		"taproot should have been removed after re-enrichment"
	);
	assert!(
		ctx.concepts.contains(&"silent-payments".to_string()),
		"expected 'silent-payments', got {:?}",
		ctx.concepts
	);
	assert!(
		ctx.concepts.contains(&"payjoin".to_string()),
		"expected 'payjoin', got {:?}",
		ctx.concepts
	);
}

/// Verify timeline filtering by date range.
#[tokio::test]
async fn test_timeline_date_filtering() {
	let store = seeded_store().await;

	// Only events after 2022 -- should exclude BIP-340 (2020) and the
	// mailing list msg (2021-04).
	let after = Utc.with_ymd_and_hms(2022, 1, 1, 0, 0, 0).unwrap();
	let timeline = store.timeline("taproot", Some(after), None).await.unwrap();

	for event in &timeline.events {
		assert!(event.date.as_str() >= "2022", "event date {} should be >= 2022", event.date);
	}

	// Only events before 2021 -- should only include BIP-340 (2020).
	let before = Utc.with_ymd_and_hms(2021, 1, 1, 0, 0, 0).unwrap();
	let timeline = store.timeline("taproot", None, Some(before)).await.unwrap();

	assert!(!timeline.events.is_empty(), "should have at least one taproot event before 2021");
	for event in &timeline.events {
		assert!(event.date.as_str() < "2021", "event date {} should be < 2021", event.date);
	}
}

/// Verify search filtering by source type.
#[tokio::test]
async fn test_search_with_source_type_filter() {
	let store = seeded_store().await;

	let results = store
		.search(SearchParams {
			query: "schnorr".to_string(),
			source_type: Some(vec![SourceType::Bip]),
			..Default::default()
		})
		.await
		.unwrap();

	assert!(!results.results.is_empty(), "should find BIP-340 when filtering to Bip source type");
	for r in &results.results {
		assert_eq!(r.source_type, SourceType::Bip, "all results should be of type Bip");
	}
}
