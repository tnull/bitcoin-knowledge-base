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

const FORGEJO_REPO: &str = "git.rust-bitcoin.org/lightningdevkit/rust-lightning";

#[tokio::test]
async fn commit_history_replay_preserves_existing_data_and_imports_missing_commits() {
	use bkb_core::model::Reference;
	use bkb_ingest::{queue::store_page, sources::SyncPage};
	let temp = tempfile::tempdir().unwrap();
	let path = temp.path().join("commit-replay.db");
	let store = SqliteStore::open(&path).unwrap();
	let sql = rusqlite::Connection::open(&path).unwrap();
	let mut old = make_doc(
		SourceType::Commit,
		Some("lightningdevkit/rust-lightning"),
		"old-sha",
		"Merge #42",
		"Taproot implementation",
		"alice",
		Utc::now(),
	);
	old.metadata = Some(
		serde_json::json!({"url":"https://github.com/lightningdevkit/rust-lightning/commit/old-sha"}),
	);
	store.upsert_document(&old).await.unwrap();
	store
		.insert_reference(&Reference {
			id: None,
			from_doc_id: old.id.clone(),
			to_doc_id: None,
			ref_type: RefType::MentionsPr,
			to_external: Some("lightningdevkit/rust-lightning#42".into()),
			context: Some("Historical GitHub merge".into()),
		})
		.await
		.unwrap();
	sql.execute("INSERT INTO concepts (slug,name) VALUES ('custom-tag','Custom tag')", []).unwrap();
	store.upsert_concept_mention(&old.id, "custom-tag", 0.25).await.unwrap();
	store.upsert_concept_mention(&old.id, "taproot", 0.5).await.unwrap();
	let before = store.get_raw_document(&old.id).await.unwrap().unwrap();
	let old_reference = serde_json::to_value(&before.outgoing_refs[0]).unwrap();
	let mut replayed = old.clone();
	replayed.metadata =
		Some(serde_json::json!({"url":"https://git.rust-bitcoin.org/new-location"}));
	let missing = make_doc(
		SourceType::Commit,
		Some("lightningdevkit/rust-lightning"),
		"missing-sha",
		"Previously skipped commit",
		"Payjoin implementation",
		"bob",
		Utc::now(),
	);
	let checkpoint =
		r#"{"version":1,"head":"head-sha","baseline":null,"offset":200,"complete":false}"#;
	let page = SyncPage {
		documents: vec![replayed, missing.clone()],
		references: vec![],
		next_cursor: Some(checkpoint.into()),
		checkpoint: None,
	};
	let source = "commits:lightningdevkit/rust-lightning";
	sql.execute_batch("CREATE TRIGGER reject_commit_replay BEFORE INSERT ON sync_state BEGIN SELECT RAISE(ABORT,'checkpoint rejected'); END").unwrap();
	assert!(store_page(&store, source, Some("old-sha"), &page).await.is_err());
	assert!(store.get_document(&missing.id).await.unwrap().is_none());
	assert!(store.get_sync_state(source).await.unwrap().is_none());
	sql.execute_batch("DROP TRIGGER reject_commit_replay").unwrap();
	let mut previous = Some("old-sha".to_owned());
	for _ in 0..2 {
		store_page(&store, source, previous.as_deref(), &page).await.unwrap();
		let after = store.get_raw_document(&old.id).await.unwrap().unwrap();
		assert!(
			after.outgoing_refs.iter().any(|r| serde_json::to_value(r).unwrap() == old_reference),
			"Git history repair must retain historical references and their IDs"
		);
		assert_eq!(
			serde_json::to_value(&after.document).unwrap(),
			serde_json::to_value(&before.document).unwrap(),
			"Git history repair must retain the entire stored commit record"
		);
		for (slug, expected) in [("custom-tag", 0.25), ("taproot", 0.5)] {
			let confidence: f32 = sql
				.query_row(
					"SELECT confidence FROM concept_mentions WHERE doc_id=?1 AND concept_slug=?2",
					rusqlite::params![old.id, slug],
					|r| r.get(0),
				)
				.unwrap();
			assert_eq!(confidence, expected);
		}
		let changes: i64 =
			sql.query_row("SELECT COUNT(*) FROM change_log", [], |r| r.get(0)).unwrap();
		assert_eq!(changes, 2, "replay must add only previously missing commits");
		let imported = store.get_raw_document(&missing.id).await.unwrap().unwrap();
		assert_eq!(imported.document.body, missing.body);
		assert!(imported.concepts.contains(&"payjoin".into()));
		previous = store.get_sync_state(source).await.unwrap().unwrap().last_cursor;
		assert_eq!(previous.as_deref(), Some(checkpoint));
	}
	// Mutable source records still refresh through the same page writer.
	let mut issue = make_doc(
		SourceType::GithubIssue,
		Some("bitcoin/bitcoin"),
		"1",
		"Old title",
		"Old body",
		"alice",
		Utc::now(),
	);
	store.upsert_document(&issue).await.unwrap();
	issue.body = Some("Updated body with payjoin".into());
	let update = SyncPage {
		documents: vec![issue.clone()],
		references: vec![],
		next_cursor: None,
		checkpoint: None,
	};
	store_page(&store, "github:bitcoin/bitcoin", None, &update).await.unwrap();
	let updated = store.get_raw_document(&issue.id).await.unwrap().unwrap();
	assert_eq!(updated.document.body, issue.body);
	assert!(updated.concepts.contains(&"payjoin".into()));
}

fn permalink_document(kind: SourceType, id: &str, anchor: &str) -> Document {
	let mut doc =
		make_doc(kind, Some(FORGEJO_REPO), id, "Review", "Review text", "alice", Utc::now());
	doc.metadata = Some(
		serde_json::json!({"url": format!("https://{FORGEJO_REPO}/pulls/4775#issuecomment-{anchor}")}),
	);
	doc
}

async fn store_permalink_mention(store: &SqliteStore, url: &str) -> Document {
	let doc = make_doc(
		SourceType::GithubIssue,
		Some("bitcoin/bitcoin"),
		"123",
		"Mention",
		url,
		"bob",
		Utc::now(),
	);
	let references = bkb_ingest::forge_refs::extract(&doc);
	store.upsert_document(&doc).await.unwrap();
	for reference in references {
		store.insert_reference(&reference).await.unwrap();
	}
	doc
}

async fn assert_permalink_target(store: &SqliteStore, mention: &Document, target: &Document) {
	let url = target.metadata.as_ref().unwrap()["url"].as_str().unwrap();
	for context in [
		store.get_document(&mention.id).await.unwrap().unwrap(),
		store.get_raw_document(&mention.id).await.unwrap().unwrap(),
	] {
		assert_eq!(context.outgoing_refs.len(), 1);
		assert_eq!(
			context.outgoing_refs[0].to_doc_id.as_deref(),
			Some(target.id.as_str()),
			"Forgejo permalinks must resolve to the raw API document ID"
		);
	}
	assert_eq!(
		store.get_document(&target.id).await.unwrap().unwrap().incoming_refs.len(),
		1,
		"resolved permalink must appear in incoming references"
	);
	for entity in [&target.id, url] {
		let references = store.get_references(entity, None, 10).await.unwrap();
		assert_eq!(references.len(), 1, "permalink and raw ID lookups must agree");
		assert_eq!(references[0].to_doc_id.as_deref(), Some(target.id.as_str()));
	}
}

#[tokio::test]
async fn forgejo_permalinks_resolve_all_comment_kinds_by_stored_url() {
	for (kind, id, anchor) in [
		(SourceType::ForgejoComment, "424542", "424542"),
		(SourceType::ForgejoReviewComment, "424543", "424543"),
		(SourceType::ForgejoReview, "159430", "424544"),
	] {
		let store = SqliteStore::open_in_memory().unwrap();
		let target = permalink_document(kind, id, anchor);
		store.upsert_document(&target).await.unwrap();
		let mention = store_permalink_mention(
			&store,
			target.metadata.as_ref().unwrap()["url"].as_str().unwrap(),
		)
		.await;
		assert_permalink_target(&store, &mention, &target).await;
	}
}

#[tokio::test]
async fn forgejo_permalinks_wait_for_targets_and_reject_ambiguous_urls() {
	let store = SqliteStore::open_in_memory().unwrap();
	let target = permalink_document(SourceType::ForgejoReview, "159430", "424544");
	let url = target.metadata.as_ref().unwrap()["url"].as_str().unwrap();
	let mention = store_permalink_mention(&store, url).await;
	assert!(
		store.get_raw_document(&mention.id).await.unwrap().unwrap().outgoing_refs[0]
			.to_doc_id
			.is_none(),
		"a missing permalink target must remain unresolved"
	);
	// A record from another forge cannot provide evidence for this URL.
	let mut other_host = target.clone();
	other_host.source_type = SourceType::GithubReview;
	other_host.source_repo = Some("lightningdevkit/rust-lightning".into());
	other_host.id = Document::make_id(
		&other_host.source_type,
		other_host.source_repo.as_deref(),
		&other_host.source_id,
	);
	store.upsert_document(&other_host).await.unwrap();
	assert!(store.get_document(&mention.id).await.unwrap().unwrap().outgoing_refs[0]
		.to_doc_id
		.is_none());
	store.upsert_document(&target).await.unwrap();
	assert_permalink_target(&store, &mention, &target).await;
	let ambiguous = permalink_document(SourceType::ForgejoComment, "424544", "424544");
	store.upsert_document(&ambiguous).await.unwrap();
	for context in [
		store.get_document(&mention.id).await.unwrap().unwrap(),
		store.get_raw_document(&mention.id).await.unwrap().unwrap(),
	] {
		assert!(
			context.outgoing_refs[0].to_doc_id.is_none(),
			"a second raw candidate must invalidate an ambiguous permalink"
		);
	}
	assert!(store.get_document(&target.id).await.unwrap().unwrap().incoming_refs.is_empty());
	assert!(store.get_document(&ambiguous.id).await.unwrap().unwrap().incoming_refs.is_empty());
	assert!(store.get_references(&target.id, None, 10).await.unwrap().is_empty());
	assert!(store.get_references(url, None, 10).await.unwrap()[0].to_doc_id.is_none());
}

#[tokio::test]
async fn forgejo_permalink_ambiguity_clears_when_a_candidate_changes_url() {
	for write_page in [false, true] {
		let store = SqliteStore::open_in_memory().unwrap();
		let target = permalink_document(SourceType::ForgejoReview, "159430", "424544");
		let mut ambiguous = permalink_document(SourceType::ForgejoComment, "424544", "424544");
		store.upsert_document(&target).await.unwrap();
		store.upsert_document(&ambiguous).await.unwrap();
		let mention = store_permalink_mention(
			&store,
			target.metadata.as_ref().unwrap()["url"].as_str().unwrap(),
		)
		.await;
		assert!(store.get_raw_document(&mention.id).await.unwrap().unwrap().outgoing_refs[0]
			.to_doc_id
			.is_none());
		ambiguous.metadata = Some(
			serde_json::json!({"url": format!("https://{FORGEJO_REPO}/pulls/4775#issuecomment-424545")}),
		);
		if write_page {
			store
				.commit_page(
					&[ambiguous.clone()],
					&[],
					&[],
					&bkb_core::model::SyncState {
						source_id: "forgejo:test".into(),
						last_cursor: Some("1".into()),
						last_synced_at: Some(Utc::now()),
						source_type: "forgejo_comment".into(),
						source_repo: Some(FORGEJO_REPO.into()),
						status: bkb_core::model::SyncStatus::Ok,
						error_message: None,
						next_run_at: None,
						items_found: 1,
						retry_count: 0,
					},
				)
				.await
				.unwrap();
		} else {
			store.upsert_document(&ambiguous).await.unwrap();
		}
		assert_permalink_target(&store, &mention, &target).await;
		assert!(store.get_document(&ambiguous.id).await.unwrap().unwrap().incoming_refs.is_empty());
	}
}

#[tokio::test]
async fn forgejo_permalinks_repair_historical_guesses_without_replacing_references() {
	use bkb_ingest::{queue::store_page, sources::SyncPage};
	for target_arrives_late in [false, true] {
		let temp = tempfile::tempdir().unwrap();
		let path = temp.path().join("permalinks.db");
		let store = SqliteStore::open(&path).unwrap();
		let sql = rusqlite::Connection::open(&path).unwrap();
		let target = permalink_document(SourceType::ForgejoReview, "159430", "424544");
		let url = target.metadata.as_ref().unwrap()["url"].as_str().unwrap();
		if !target_arrives_late {
			store.upsert_document(&target).await.unwrap();
		}
		let mention =
			store_permalink_mention(&store, "Historical reference absent from current text").await;
		sql.execute("INSERT INTO refs (from_doc_id,to_doc_id,ref_type,to_external,context) VALUES (?1,?2,'mentions_pr',?3,'Preserved context')", rusqlite::params![mention.id, format!("forgejo_comment:{FORGEJO_REPO}:424544"), url]).unwrap();
		let reference_id = sql.last_insert_rowid();
		if target_arrives_late {
			store.upsert_document(&target).await.unwrap();
		} else {
			let page = SyncPage {
				documents: vec![mention.clone()],
				references: vec![],
				next_cursor: None,
				checkpoint: Some("1".into()),
			};
			store_page(&store, "reconcile:v1", None, &page).await.unwrap();
		}
		assert_permalink_target(&store, &mention, &target).await;
		let reference =
			&store.get_raw_document(&mention.id).await.unwrap().unwrap().outgoing_refs[0];
		assert_eq!(reference.id, Some(reference_id));
		assert_eq!(reference.context.as_deref(), Some("Preserved context"));
	}
}

#[tokio::test]
async fn reconciliation_preserves_legacy_data_and_adds_missing_enrichment() {
	use bkb_core::model::Reference;
	use bkb_ingest::{queue::store_page, sources::SyncPage};
	let temp = tempfile::tempdir().unwrap();
	let path = temp.path().join("backfill.db");
	let store = SqliteStore::open(&path).unwrap();
	let sql = rusqlite::Connection::open(&path).unwrap();
	let doc = make_doc(
		SourceType::GithubIssue,
		Some("bitcoin/bitcoin"),
		"1",
		"Discussion",
		"Taproot and https://github.com/bitcoin/bitcoin/pull/42",
		"alice",
		Utc::now(),
	);
	let target = make_doc(
		SourceType::GithubPr,
		Some("bitcoin/bitcoin"),
		"42",
		"Implementation",
		"Details",
		"bob",
		Utc::now(),
	);
	store.upsert_document(&doc).await.unwrap();
	store.upsert_document(&target).await.unwrap();
	store
		.insert_reference(&Reference {
			id: None,
			from_doc_id: doc.id.clone(),
			to_doc_id: None,
			ref_type: RefType::MentionsPr,
			to_external: Some("bitcoin/bitcoin#777".into()),
			context: Some("Historical link absent from the current body".into()),
		})
		.await
		.unwrap();
	sql.execute("INSERT INTO concepts (slug, name) VALUES ('custom-tag', 'Custom tag')", [])
		.unwrap();
	store.upsert_concept_mention(&doc.id, "custom-tag", 0.25).await.unwrap();
	store.upsert_concept_mention(&doc.id, "taproot", 0.5).await.unwrap();
	let before = store.get_raw_document(&doc.id).await.unwrap().unwrap();
	let raw_before = serde_json::to_value(&before.document).unwrap();
	let legacy_ref = serde_json::to_value(&before.outgoing_refs[0]).unwrap();
	let changes_before: String = sql.query_row(
		"SELECT json_group_array(json_array(seq, doc_id, change_type, changed_at)) FROM change_log", [], |r| r.get(0),
	).unwrap();
	let page = SyncPage {
		documents: vec![before.document],
		references: vec![],
		next_cursor: None,
		checkpoint: Some("1".into()),
	};
	// Checkpoint failure must roll back every addition in the same transaction.
	sql.execute_batch(
		"CREATE TRIGGER reject_backfill BEFORE INSERT ON sync_state
		WHEN NEW.source_id='reconcile:v1' BEGIN SELECT RAISE(ABORT, 'checkpoint rejected'); END;",
	)
	.unwrap();
	let error = store_page(&store, "reconcile:v1", None, &page).await.unwrap_err();
	assert!(error.to_string().contains("checkpoint rejected"));
	assert!(store.get_sync_state("reconcile:v1").await.unwrap().is_none());
	let after_failure = store.get_raw_document(&doc.id).await.unwrap().unwrap();
	assert_eq!(serde_json::to_value(&after_failure.document).unwrap(), raw_before);
	assert_eq!(after_failure.outgoing_refs.len(), 1);
	assert_eq!(serde_json::to_value(&after_failure.outgoing_refs[0]).unwrap(), legacy_ref);
	sql.execute_batch("DROP TRIGGER reject_backfill").unwrap();
	for _ in 0..2 {
		store_page(&store, "reconcile:v1", None, &page).await.unwrap();
		let after = store.get_raw_document(&doc.id).await.unwrap().unwrap();
		assert!(
			after.outgoing_refs.iter().any(|r| serde_json::to_value(r).unwrap() == legacy_ref),
			"backfill must preserve historical references that cannot be reconstructed"
		);
		assert_eq!(
			serde_json::to_value(&after.document).unwrap(),
			raw_before,
			"backfill must preserve the complete raw document, including seq"
		);
		for (slug, expected) in [("custom-tag", 0.25), ("taproot", 0.5)] {
			let confidence: f32 = sql
				.query_row(
					"SELECT confidence FROM concept_mentions WHERE doc_id=?1 AND concept_slug=?2",
					rusqlite::params![doc.id, slug],
					|r| r.get(0),
				)
				.unwrap();
			assert_eq!(confidence, expected, "backfill must retain existing concept confidence");
		}
		let changes_after: String = sql.query_row(
			"SELECT json_group_array(json_array(seq, doc_id, change_type, changed_at)) FROM change_log", [], |r| r.get(0),
		).unwrap();
		assert_eq!(changes_after, changes_before, "backfill must not manufacture raw changes");
		assert_eq!(
			after
				.outgoing_refs
				.iter()
				.filter(|r| r.to_doc_id.as_deref() == Some(target.id.as_str()))
				.count(),
			1,
			"missing references must be added exactly once across replay"
		);
		assert_eq!(
			store.get_sync_state("reconcile:v1").await.unwrap().unwrap().last_cursor.as_deref(),
			Some("1")
		);
	}
}

#[tokio::test]
async fn reconciliation_uses_current_documents_instead_of_stale_page_data() {
	use bkb_ingest::{queue::store_page, sources::SyncPage};
	let store = SqliteStore::open_in_memory().unwrap();
	let mut doc = make_doc(
		SourceType::GithubIssue,
		Some("bitcoin/bitcoin"),
		"1",
		"Old title",
		"Old body references #10",
		"alice",
		Utc::now(),
	);
	store.upsert_document(&doc).await.unwrap();
	let page = SyncPage {
		documents: vec![store.get_raw_document(&doc.id).await.unwrap().unwrap().document],
		references: vec![],
		next_cursor: None,
		checkpoint: Some("1".into()),
	};
	doc.title = Some("Fresh title".into());
	doc.body = Some("Fresh body references #20 and payjoin".into());
	doc.updated_at = Some(Utc::now());
	store.upsert_document(&doc).await.unwrap();
	let current = store.get_raw_document(&doc.id).await.unwrap().unwrap().document;
	store_page(&store, "reconcile:v1", None, &page).await.unwrap();
	let after = store.get_raw_document(&doc.id).await.unwrap().unwrap();
	assert_eq!(
		serde_json::to_value(&after.document).unwrap(),
		serde_json::to_value(current).unwrap(),
		"backfill must not overwrite an ingestion update made after page fetch"
	);
	assert!(
		after.outgoing_refs.iter().any(|r| r.to_external.as_deref() == Some("bitcoin/bitcoin#20")),
		"backfill must enrich the current body"
	);
	assert!(
		!after.outgoing_refs.iter().any(|r| r.to_external.as_deref() == Some("bitcoin/bitcoin#10")),
		"backfill must not derive new references from stale content"
	);
	assert!(after.concepts.contains(&"payjoin".into()));
}

#[tokio::test]
async fn cross_forge_mirror_survives_restart_and_origin_edits() {
	use bkb_ingest::{queue::store_page, sources::SyncPage};
	let temp = tempfile::tempdir().unwrap();
	let path = temp.path().join("upgrade.db");
	let repo = "lightningdevkit/rust-lightning";
	let fj_repo = bkb_core::repository::forgejo_identity(repo);
	let date = Utc.with_ymd_and_hms(2026, 8, 8, 1, 2, 3).unwrap();
	let gh_issue = make_doc(
		SourceType::GithubIssue,
		Some(repo),
		"4760",
		"Electrum script",
		"Original report",
		"alice",
		date,
	);
	let fj_issue = make_doc(
		SourceType::ForgejoIssue,
		Some(&fj_repo),
		"4866",
		"Electrum script",
		"Imported report",
		"alice",
		date,
	);
	let mut original = make_doc(
		SourceType::GithubComment,
		Some(repo),
		"42",
		"",
		"Schnorr and #4760",
		"alice",
		date,
	);
	original.title = None;
	original.parent_id = Some(gh_issue.id.clone());
	let envelope="*[\u{60}@alice\u{60}](https://github.com/alice) [commented](https://github.com/lightningdevkit/rust-lightning/issues/4760#issuecomment-42) on GitHub:*\n\n---\n\nSchnorr and #4760\n\n<!-- issue-mirror:v1 origin=github gh_repo=lightningdevkit/rust-lightning gh_issue=4760 gh_comment=42 -->";
	let mut copy = make_doc(
		SourceType::ForgejoComment,
		Some(&fj_repo),
		"99",
		"",
		envelope,
		"github-comment-mirror",
		date + chrono::Duration::minutes(1),
	);
	copy.title = None;
	copy.author_id = Some("72".into());
	copy.parent_id = Some(fj_issue.id.clone());
	{
		let store = SqliteStore::open(&path).unwrap();
		let page = SyncPage {
			documents: vec![copy.clone(), gh_issue.clone(), fj_issue.clone()],
			references: vec![],
			next_cursor: Some("next-page".into()),
			checkpoint: None,
		};
		store_page(&store, "fixture:forgejo", None, &page).await.unwrap();
		assert_eq!(
			store
				.reconciliation_stats()
				.await
				.unwrap()
				.iter()
				.find(|s| s.0 == "unresolved_links")
				.unwrap()
				.1,
			1
		);
	}
	let store = SqliteStore::open(&path).unwrap();
	assert_eq!(
		store.get_sync_state("fixture:forgejo").await.unwrap().unwrap().last_cursor.as_deref(),
		Some("next-page")
	);
	let page = SyncPage {
		documents: vec![original.clone()],
		references: vec![],
		next_cursor: None,
		checkpoint: Some("done".into()),
	};
	store_page(&store, "fixture:github", None, &page).await.unwrap();
	let results = store
		.search(SearchParams { query: "Schnorr".into(), limit: Some(1), ..Default::default() })
		.await
		.unwrap();
	assert_eq!(results.results.len(), 1);
	assert_eq!(results.results[0].id, original.id);
	assert_eq!(results.results[0].author.as_deref(), Some("alice"));
	assert_eq!(results.results[0].provenance.len(), 2);
	let ctx = store.get_document(&copy.id).await.unwrap().unwrap();
	assert_eq!(ctx.document.parent_id.as_deref(), Some(fj_issue.id.as_str()));
	assert!(ctx.outgoing_refs.iter().any(|r| r.to_doc_id.as_deref() == Some(fj_issue.id.as_str())));
	assert_eq!(store.timeline("taproot", None, None).await.unwrap().events.len(), 1);
	assert_eq!(
		store.get_raw_document(&copy.id).await.unwrap().unwrap().document.body.as_deref(),
		Some(envelope)
	);
	original.body = Some("Edited original text".into());
	original.updated_at = Some(date + chrono::Duration::hours(1));
	store.upsert_document(&original).await.unwrap();
	assert_eq!(store.get_document(&copy.id).await.unwrap().unwrap().document.body, original.body);
	assert_eq!(
		store.get_raw_document(&copy.id).await.unwrap().unwrap().document.body.as_deref(),
		Some(envelope)
	);
}

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

/// Verify that commit documents are properly stored, searchable, and returned
/// by `find_commit`.
#[tokio::test]
async fn test_commit_ingest_and_find() {
	let store = SqliteStore::open_in_memory().unwrap();

	// Simulate an ingested commit document (as produced by GitCommitSyncSource)
	let commit = make_doc(
		SourceType::Commit,
		Some("lightningdevkit/rust-lightning"),
		"abc123def456789012345678901234567890abcd",
		"Fix channel monitor persistence race condition",
		"Fix channel monitor persistence race condition\n\n\
		 Ensure monitors are persisted before broadcasting commitment\n\
		 transactions. This prevents a race where a crash between broadcast\n\
		 and persistence could lead to loss of funds.\n\
		 Fixes #4567\n\n\
		 ---\nChangeset:\n\
		 1 file(s) changed, 15 insertion(s)(+), 3 deletion(s)(-)\n\
		 M lightning/src/chain/channelmonitor.rs\n\n\
		 +    // Persist before broadcast to avoid race\n\
		 +    self.persist_monitor()?;\n",
		"TheBlueMatt",
		Utc.with_ymd_and_hms(2024, 3, 15, 10, 30, 0).unwrap(),
	);
	ingest_and_enrich(&store, &commit).await;

	// Search for it by keyword
	let results = store
		.search(SearchParams {
			query: "channel monitor persistence".to_string(),
			source_type: Some(vec![SourceType::Commit]),
			..Default::default()
		})
		.await
		.unwrap();

	assert!(!results.results.is_empty(), "should find the commit via FTS5 search");
	assert_eq!(results.results[0].source_type, SourceType::Commit);

	// Use find_commit
	let commits = store.find_commit("channel monitor persistence", None).await.unwrap();

	assert!(!commits.is_empty(), "find_commit should return results");

	let found = commits
		.iter()
		.find(|c| c.document.source_type == SourceType::Commit)
		.expect("should include a Commit-type result");

	assert_eq!(found.document.author.as_deref(), Some("TheBlueMatt"));
	assert_eq!(
		found.url.as_deref(),
		Some("https://github.com/lightningdevkit/rust-lightning/commit/abc123def456789012345678901234567890abcd")
	);

	// Verify enrichment extracted the #4567 issue reference
	let ctx = store.get_document(&commit.id).await.unwrap().unwrap();
	let issue_refs: Vec<_> = ctx
		.outgoing_refs
		.iter()
		.filter(|r| r.ref_type == RefType::MentionsIssue || r.ref_type == RefType::Fixes)
		.collect();
	assert!(!issue_refs.is_empty(), "commit body should have extracted issue refs (Fixes #4567)");
}

/// Verify that searching commits by repo filter works.
#[tokio::test]
async fn test_find_commit_with_repo_filter() {
	let store = SqliteStore::open_in_memory().unwrap();

	let commit1 = make_doc(
		SourceType::Commit,
		Some("bitcoin/bitcoin"),
		"1111111111111111111111111111111111111111",
		"Add taproot validation",
		"Add taproot validation for BIP-341",
		"sipa",
		Utc.with_ymd_and_hms(2023, 1, 1, 0, 0, 0).unwrap(),
	);
	let commit2 = make_doc(
		SourceType::Commit,
		Some("lightningdevkit/rust-lightning"),
		"2222222222222222222222222222222222222222",
		"Add taproot channel support",
		"Add taproot channel support for Lightning",
		"TheBlueMatt",
		Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
	);

	ingest_and_enrich(&store, &commit1).await;
	ingest_and_enrich(&store, &commit2).await;

	// Search without repo filter -- should find both
	let all = store.find_commit("taproot", None).await.unwrap();
	assert!(all.len() >= 2, "should find commits from both repos");

	// Search with repo filter
	let filtered = store.find_commit("taproot", Some("bitcoin/bitcoin")).await.unwrap();
	for ctx in &filtered {
		if ctx.document.source_type == SourceType::Commit {
			assert_eq!(
				ctx.document.source_repo.as_deref(),
				Some("bitcoin/bitcoin"),
				"filtered results should only be from bitcoin/bitcoin"
			);
		}
	}
}
