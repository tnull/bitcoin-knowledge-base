//! Upgrade compatibility with the schema shipped before cross-forge ingestion.
#[path = "fixtures/legacy_schema.rs"]
mod legacy;

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use bkb_core::model::{SearchParams, SourceType};
use bkb_core::store::KnowledgeStore;
use bkb_store::sqlite::SqliteStore;
use rusqlite::{types::Value, Connection};

struct Database(PathBuf);

impl Database {
	fn legacy() -> Self {
		static NEXT: AtomicUsize = AtomicUsize::new(0);
		let path = std::env::temp_dir().join(format!(
			"bkb-upgrade-test-{}-{}",
			std::process::id(),
			NEXT.fetch_add(1, Ordering::Relaxed)
		));
		std::fs::create_dir(&path).unwrap();
		let db = Self(path.join("legacy.db"));
		let conn = db.connect();
		conn.execute_batch(legacy::SCHEMA_SQL).unwrap();
		conn.execute_batch(
			"INSERT INTO documents VALUES (
			 'github_issue:rust-bitcoin/rust-miniscript:42', 'github_issue',
			 'rust-bitcoin/rust-miniscript', '42', 'Legacy Taproot', 'preservedneedle body',
			 'old-author', '0012', '2020-01-02T03:04:05+01:00', NULL, NULL,
			 '{ \"nested\": [1, null], \"url\": \"https://github.com/rust-bitcoin/rust-miniscript/issues/42\" }', 73);
			 INSERT INTO refs VALUES (19, 'github_issue:rust-bitcoin/rust-miniscript:42',
			 NULL, 'references_bip', 'BIP-341', 'old reference');
			 INSERT INTO concepts VALUES ('taproot', 'Existing custom label', 'old category', '[\"old alias\"]');
			 INSERT INTO concept_mentions VALUES ('github_issue:rust-bitcoin/rust-miniscript:42', 'taproot', 0.75);
			 INSERT INTO change_log VALUES (73, 'github_issue:rust-bitcoin/rust-miniscript:42', 'insert', '2020-01-02 03:04:05');
			 INSERT INTO sync_state VALUES ('github:rust-bitcoin/rust-miniscript', 'github',
			 'rust-bitcoin/rust-miniscript', 'opaque old cursor', '2020-01-02T03:04:05Z',
			 NULL, 'idle', NULL, 2, 9);",
		)
		.unwrap();
		db
	}

	fn connect(&self) -> Connection {
		Connection::open(&self.0).unwrap()
	}
}

impl Drop for Database {
	fn drop(&mut self) {
		std::fs::remove_dir_all(self.0.parent().unwrap()).unwrap();
	}
}

fn snapshot(conn: &Connection, table: &str) -> Vec<Vec<Value>> {
	let mut stmt = conn.prepare(&format!("SELECT rowid, * FROM {table} ORDER BY rowid")).unwrap();
	let columns = stmt.column_count();
	stmt.query_map([], |row| (0..columns).map(|i| row.get(i)).collect())
		.unwrap()
		.collect::<rusqlite::Result<_>>()
		.unwrap()
}

fn schema(conn: &Connection) -> Vec<Vec<Value>> {
	snapshot(conn, "sqlite_schema")
}

#[tokio::test]
async fn legacy_upgrade_preserves_raw_rows_and_api_reads() {
	let db = Database::legacy();
	let conn = db.connect();
	let tables = [
		"documents",
		"refs",
		"concepts",
		"concept_mentions",
		"change_log",
		"sync_state",
		"sqlite_sequence",
	];
	let before: Vec<_> = tables.iter().map(|table| snapshot(&conn, table)).collect();
	drop(conn);
	let store = SqliteStore::open(&db.0).expect("legacy database must upgrade");
	let id = "github_issue:rust-bitcoin/rust-miniscript:42";
	for context in
		[store.get_document(id).await.unwrap(), store.get_raw_document(id).await.unwrap()]
	{
		let context = context.unwrap();
		assert_eq!(context.document.id, id);
		assert_eq!(context.document.source_type, SourceType::GithubIssue);
		assert_eq!(context.document.created_at.to_rfc3339(), "2020-01-02T02:04:05+00:00");
		assert_eq!(context.document.body.as_deref(), Some("preservedneedle body"));
		assert_eq!(context.document.author_id.as_deref(), Some("0012"));
		assert_eq!(context.outgoing_refs[0].id, Some(19));
		assert_eq!(context.concepts, ["taproot"]);
	}
	let found = store
		.search(SearchParams { query: "preservedneedle".into(), ..Default::default() })
		.await
		.unwrap();
	assert_eq!(found.results.len(), 1);
	assert_eq!(found.results[0].id, id);
	let state = store.get_all_sync_states().await.unwrap().pop().unwrap();
	assert_eq!(state.last_cursor.as_deref(), Some("opaque old cursor"));
	assert_eq!(state.retry_count, 2);
	drop(store);
	let conn = db.connect();
	for (table, expected) in tables.iter().zip(before) {
		let after = snapshot(&conn, table);
		if *table == "concepts" {
			assert_eq!(&after[..expected.len()], &expected, "existing concept values must survive");
		} else {
			assert_eq!(after, expected, "raw values and row IDs changed in {table}");
		}
	}
	assert_eq!(conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
	drop(conn);
	SqliteStore::open(&db.0).expect("already upgraded database must reopen");
}

fn assert_refused(fault: &str, expected: &str) {
	let db = Database::legacy();
	let conn = db.connect();
	conn.execute_batch(fault).unwrap();
	let before_schema = schema(&conn);
	let before_docs = snapshot(&conn, "documents");
	let before_concepts = snapshot(&conn, "concepts");
	let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0)).unwrap();
	let journal: String = conn.query_row("PRAGMA journal_mode", [], |row| row.get(0)).unwrap();
	drop(conn);
	let err = match SqliteStore::open(&db.0) {
		Ok(_) => panic!("unsafe upgrade must be refused: {fault}"),
		Err(err) => format!("{err:#}"),
	};
	let conn = db.connect();
	assert!(schema(&conn) == before_schema, "failed upgrade changed schema: {fault}");
	assert_eq!(snapshot(&conn, "documents"), before_docs, "failed upgrade changed source records");
	assert_eq!(snapshot(&conn, "concepts"), before_concepts, "failed upgrade seeded concepts");
	assert_eq!(
		conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0)).unwrap(),
		version
	);
	assert_eq!(
		conn.query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0)).unwrap(),
		journal
	);
	assert!(err.contains(expected), "{fault}: expected {expected:?}, got {err}");
}

macro_rules! refused_upgrade {
	($name:ident, $fault:literal, $error:literal) => {
		#[test]
		fn $name() {
			assert_refused($fault, $error);
		}
	};
}

refused_upgrade!(future_version, "PRAGMA user_version=99", "version");
refused_upgrade!(
	late_schema_failure,
	"CREATE VIEW document_links AS SELECT id FROM documents",
	"schema"
);
refused_upgrade!(
	incompatible_columns,
	"ALTER TABLE documents ADD COLUMN future_payload BLOB",
	"schema"
);
refused_upgrade!(missing_legacy_table, "DROP TABLE sync_state", "schema");
refused_upgrade!(
	unknown_document_type,
	"UPDATE documents SET source_type='future_source'",
	"source type"
);
refused_upgrade!(unreadable_timestamp, "UPDATE documents SET created_at='yesterday'", "timestamp");
refused_upgrade!(malformed_metadata, "UPDATE documents SET metadata='broken json'", "JSON");
refused_upgrade!(unreadable_sqlite_type, "UPDATE documents SET author=x'80'", "readable");
refused_upgrade!(
	inconsistent_full_text_index,
	"INSERT INTO documents_fts(documents_fts) VALUES ('delete-all')",
	"full-text"
);
refused_upgrade!(
	destructive_seed_trigger,
	"CREATE TRIGGER destroy_documents AFTER INSERT ON concepts BEGIN DELETE FROM refs; DELETE FROM concept_mentions; DELETE FROM documents; END",
	"preserve existing rows"
);
refused_upgrade!(failing_seed_trigger, "CREATE TRIGGER fail_seed AFTER INSERT ON concepts BEGIN SELECT RAISE(ABORT, 'seed rejected'); END", "seed rejected");
refused_upgrade!(mutating_seed_trigger, "CREATE TRIGGER rewrite_documents AFTER INSERT ON concepts BEGIN UPDATE documents SET body='lost'; END", "preserve existing rows");
refused_upgrade!(
	broken_foreign_key,
	"PRAGMA foreign_keys=OFF; UPDATE refs SET from_doc_id='missing'",
	"foreign keys"
);

#[tokio::test]
async fn legacy_source_types_and_harmless_custom_objects_remain_usable() {
	let db = Database::legacy();
	let conn = db.connect();
	conn.execute_batch("CREATE TABLE audit (slug TEXT); CREATE INDEX custom_author_index ON documents(author_id); CREATE TRIGGER audit_seed AFTER INSERT ON concepts BEGIN INSERT INTO audit VALUES (NEW.slug); END").unwrap();
	// The outer INSERT OR IGNORE used by seeding also suppresses conflicting
	// replacements inside a trigger; this does not actually replace old values.
	conn.execute_batch("CREATE TRIGGER replace_concept AFTER INSERT ON concepts WHEN NEW.slug != 'taproot' BEGIN INSERT OR REPLACE INTO concepts VALUES ('taproot', 'lost', NULL, NULL); END").unwrap();
	let kinds = [
		"github_issue",
		"github_pr",
		"github_comment",
		"github_review",
		"github_review_comment",
		"github_discussion",
		"github_discussion_comment",
		"commit",
		"mailing_list_msg",
		"irc_log",
		"delving_topic",
		"delving_post",
		"bip",
		"bolt",
		"blip",
		"lud",
		"nut",
		"optech_newsletter",
		"optech_topic",
		"optech_blog",
		"bitcointalk_topic",
		"bitcointalk_post",
	];
	for kind in kinds {
		conn.execute("INSERT INTO documents(id,source_type,source_id,created_at,body) VALUES (?1,?1,'old-id','2020-01-01T00:00:00Z','sourcekindneedle')", [kind]).unwrap();
	}
	drop(conn);
	let store = SqliteStore::open(&db.0).unwrap();
	for kind in kinds {
		let raw = store.get_raw_document(kind).await.unwrap().unwrap();
		assert_eq!(raw.document.id, kind);
		assert_eq!(raw.document.source_type.as_str(), kind);
	}
	let found = store
		.search(SearchParams {
			query: "sourcekindneedle".into(),
			limit: Some(100),
			..Default::default()
		})
		.await
		.unwrap();
	assert_eq!(found.results.len(), kinds.len());
	drop(store);
	let conn = db.connect();
	assert_eq!(
		conn.query_row("SELECT name FROM concepts WHERE slug='taproot'", [], |row| row
			.get::<_, String>(0))
			.unwrap(),
		"Existing custom label"
	);
	assert!(
		conn.query_row("SELECT COUNT(*) FROM audit", [], |row| row.get::<_, i64>(0)).unwrap() > 0
	);
}
