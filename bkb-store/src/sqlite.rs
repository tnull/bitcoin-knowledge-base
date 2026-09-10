use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use rusqlite::Connection;
use tokio::sync::Mutex;
use tracing::debug;

use bkb_core::model::{
	Document, DocumentContext, RefType, Reference, SearchParams, SearchResult, SearchResults,
	SourceType, SyncState, SyncStatus,
};
use bkb_core::schema::SCHEMA_SQL;
use bkb_core::store::KnowledgeStore;

/// SQLite-backed storage for the Bitcoin Knowledge Base.
///
/// Uses `rusqlite` with bundled FTS5 for full-text search.
/// All database access is serialized through a `Mutex<Connection>`.
pub struct SqliteStore {
	conn: Arc<Mutex<Connection>>,
}

impl SqliteStore {
	/// Open (or create) a database at the given path and run migrations.
	pub fn open(path: &Path) -> Result<Self> {
		let conn = Connection::open(path)?;
		Self::init(conn)
	}

	/// Create an in-memory database (for tests).
	pub fn open_in_memory() -> Result<Self> {
		let conn = Connection::open_in_memory()?;
		Self::init(conn)
	}

	fn init(conn: Connection) -> Result<Self> {
		conn.execute_batch("PRAGMA journal_mode=WAL;")?;
		conn.execute_batch("PRAGMA foreign_keys=ON;")?;
		conn.execute_batch(SCHEMA_SQL)?;
		Self::seed_concepts(&conn)?;
		Ok(Self { conn: Arc::new(Mutex::new(conn)) })
	}

	/// Seed the `concepts` table with all entries from the curated vocabulary.
	///
	/// Uses `INSERT OR IGNORE` so existing rows are not overwritten.
	fn seed_concepts(conn: &Connection) -> Result<()> {
		use bkb_core::bitcoin::CONCEPTS;
		let mut stmt = conn.prepare(
			"INSERT OR IGNORE INTO concepts (slug, name, category, aliases)
			 VALUES (?1, ?2, ?3, ?4)",
		)?;
		for concept in CONCEPTS {
			let aliases_json = serde_json::to_string(concept.aliases)?;
			stmt.execute(rusqlite::params![
				concept.slug,
				concept.name,
				concept.category,
				&aliases_json,
			])?;
		}
		Ok(())
	}

	/// Insert or update a document, rolling back on any error.
	pub async fn upsert_document(&self, doc: &Document) -> Result<()> {
		let mut conn = self.conn.lock().await;
		let tx = conn.transaction()?;
		Self::write_document(&tx, doc)?;
		crate::reconcile::reconcile(&tx, std::slice::from_ref(doc))?;
		tx.commit()?;
		Ok(())
	}

	fn write_document(conn: &Connection, doc: &Document) -> Result<()> {
		let metadata_json = doc.metadata.as_ref().map(|m| serde_json::to_string(m)).transpose()?;

		// Retain the old URL so pending references can be reconsidered when
		// this document stops being a candidate for an ambiguous permalink.
		let previous_metadata: Option<Option<String>> = conn
			.query_row("SELECT metadata FROM documents WHERE id = ?1", [&doc.id], |row| row.get(0))
			.optional()?;
		let change_type = if previous_metadata.is_some() { "update" } else { "insert" };
		let previous_url = previous_metadata
			.flatten()
			.and_then(|value| serde_json::from_str::<serde_json::Value>(&value).ok())
			.and_then(|value| value.get("url")?.as_str().map(str::to_owned));

		// Insert into change_log first to get seq
		conn.execute(
			"INSERT INTO change_log (doc_id, change_type) VALUES (?1, ?2)",
			rusqlite::params![&doc.id, change_type],
		)?;

		let seq: i64 = conn.last_insert_rowid();

		// Upsert document
		conn.execute(
			r#"INSERT INTO documents (id, source_type, source_repo, source_id,
				title, body, author, author_id, created_at, updated_at,
				parent_id, metadata, seq)
			VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
			ON CONFLICT(id) DO UPDATE SET
				title = excluded.title,
				body = excluded.body,
				author = excluded.author,
				author_id = excluded.author_id,
				updated_at = excluded.updated_at,
				parent_id = excluded.parent_id,
				metadata = excluded.metadata,
				seq = excluded.seq"#,
			rusqlite::params![
				&doc.id,
				doc.source_type.as_str(),
				&doc.source_repo,
				&doc.source_id,
				&doc.title,
				&doc.body,
				&doc.author,
				&doc.author_id,
				doc.created_at.to_rfc3339(),
				doc.updated_at.map(|t| t.to_rfc3339()),
				&doc.parent_id,
				&metadata_json,
				seq,
			],
		)?;

		if let Some(previous_url) = previous_url {
			if doc.url().as_deref() != Some(previous_url.as_str()) {
				crate::reconcile::repair_permalink_url(conn, &previous_url)?;
			}
		}

		debug!(doc_id = %doc.id, change_type, seq, "upserted document");
		Ok(())
	}

	/// Atomically persist a complete ingestion page and its resume checkpoint.
	pub async fn commit_page(
		&self, documents: &[Document], references: &[Reference],
		mentions: &[(String, String, f32)], state: &SyncState,
	) -> Result<()> {
		let mut conn = self.conn.lock().await;
		let tx = conn.transaction()?;
		let mut retained = std::collections::HashMap::new();
		let mut current = Vec::with_capacity(documents.len());
		// Git objects are immutable. Legacy cursor repair can replay them,
		// but must not erase their historical references or custom tags.
		for doc in documents.iter().filter(|doc| doc.source_type == SourceType::Commit) {
			if let Some(stored) = crate::reconcile::raw(&tx, &doc.id)? {
				retained.insert(doc.id.clone(), stored);
			}
		}
		for doc in documents {
			if let Some(stored) = retained.get(&doc.id) {
				current.push(stored.clone());
				continue;
			}
			Self::write_document(&tx, doc)?;
			tx.execute("DELETE FROM refs WHERE from_doc_id = ?1", [&doc.id])?;
			tx.execute("DELETE FROM concept_mentions WHERE doc_id = ?1", [&doc.id])?;
			current.push(doc.clone());
		}
		for reference in references {
			if !retained.contains_key(&reference.from_doc_id) {
				Self::write_reference(&tx, reference)?;
			}
		}
		for (doc_id, slug, confidence) in mentions {
			if retained.contains_key(doc_id) {
				continue;
			}
			tx.execute(
				"INSERT OR REPLACE INTO concept_mentions VALUES (?1, ?2, ?3)",
				rusqlite::params![doc_id, slug, confidence],
			)?;
		}
		crate::reconcile::reconcile(&tx, &current)?;
		Self::write_sync_state(&tx, state)?;
		tx.commit()?;
		Ok(())
	}

	/// Add local reconciliation evidence without replacing previously ingested data.
	/// Read and enrich current documents in the checkpoint transaction so a stale
	/// backfill page cannot overwrite, or derive evidence from, an older snapshot.
	pub async fn reconcile_page<F>(
		&self, document_ids: &[String], state: &SyncState, enrich: F,
	) -> Result<()>
	where
		F: Fn(&Document) -> (Vec<Reference>, Vec<(String, f32)>),
	{
		let mut conn = self.conn.lock().await;
		let tx = conn.transaction()?;
		let mut documents = Vec::new();
		for id in document_ids {
			let Some(doc) = crate::reconcile::raw(&tx, id)? else {
				continue;
			};
			crate::reconcile::repair_permalink_refs(&tx, &doc)?;
			let (mut references, mentions) = enrich(&doc);
			if let Some(parent) = &doc.parent_id {
				references.push(Reference {
					id: None,
					from_doc_id: doc.id.clone(),
					to_doc_id: Some(parent.clone()),
					ref_type: RefType::RepliesTo,
					to_external: None,
					context: None,
				});
			}
			for reference in references {
				let target = crate::reconcile::reference_target(&tx, &reference)?;
				tx.execute(
					"INSERT INTO refs (from_doc_id, to_doc_id, ref_type, to_external, context)
					 SELECT ?1, ?2, ?3, ?4, ?5 WHERE NOT EXISTS (
					 SELECT 1 FROM refs WHERE from_doc_id=?1 AND to_doc_id IS ?2
					 AND ref_type=?3 AND to_external IS ?4)",
					rusqlite::params![
						reference.from_doc_id,
						target,
						reference.ref_type.as_str(),
						reference.to_external,
						reference.context
					],
				)?;
			}
			for (slug, confidence) in mentions {
				tx.execute(
					"INSERT OR IGNORE INTO concept_mentions (doc_id, concept_slug, confidence) VALUES (?1, ?2, ?3)",
					rusqlite::params![doc.id, slug, confidence],
				)?;
			}
			documents.push(doc);
		}
		crate::reconcile::reconcile(&tx, &documents)?;
		Self::write_sync_state(&tx, state)?;
		tx.commit()?;
		Ok(())
	}

	/// Insert a cross-reference.
	pub async fn insert_reference(&self, reference: &Reference) -> Result<()> {
		let conn = self.conn.lock().await;
		Self::write_reference(&conn, reference)
	}

	fn write_reference(conn: &Connection, reference: &Reference) -> Result<()> {
		let target = crate::reconcile::reference_target(conn, reference)?;
		conn.execute(
			"INSERT INTO refs (from_doc_id, to_doc_id, ref_type, to_external, context)
			 VALUES (?1, ?2, ?3, ?4, ?5)",
			rusqlite::params![
				&reference.from_doc_id,
				target,
				reference.ref_type.as_str(),
				&reference.to_external,
				&reference.context,
			],
		)?;
		Ok(())
	}

	/// Delete all references originating from a document (for re-enrichment).
	pub async fn replace_enrichment(
		&self, doc_id: &str, references: &[Reference], concepts: &[(String, f32)],
	) -> Result<()> {
		let mut conn = self.conn.lock().await;
		let tx = conn.transaction()?;
		let Some(doc) = crate::reconcile::raw(&tx, doc_id)? else {
			return Ok(());
		};
		tx.execute("DELETE FROM refs WHERE from_doc_id=?1", [doc_id])?;
		tx.execute("DELETE FROM concept_mentions WHERE doc_id=?1", [doc_id])?;
		for reference in references {
			Self::write_reference(&tx, reference)?;
		}
		if let Some(parent) = &doc.parent_id {
			Self::write_reference(
				&tx,
				&Reference {
					id: None,
					from_doc_id: doc_id.into(),
					to_doc_id: Some(parent.clone()),
					ref_type: RefType::RepliesTo,
					to_external: None,
					context: None,
				},
			)?;
		}
		for (slug, confidence) in concepts {
			tx.execute(
				"INSERT OR REPLACE INTO concept_mentions VALUES (?1,?2,?3)",
				rusqlite::params![doc_id, slug, confidence],
			)?;
		}
		crate::reconcile::reconcile(&tx, &[doc])?;
		tx.commit()?;
		Ok(())
	}

	/// Delete all references originating from a document (for re-enrichment).
	pub async fn delete_refs_from(&self, doc_id: &str) -> Result<()> {
		let conn = self.conn.lock().await;
		conn.execute("DELETE FROM refs WHERE from_doc_id = ?1", [doc_id])?;
		Ok(())
	}

	/// Insert or replace a concept mention for a document.
	pub async fn upsert_concept_mention(
		&self, doc_id: &str, concept_slug: &str, confidence: f32,
	) -> Result<()> {
		let conn = self.conn.lock().await;
		conn.execute(
			"INSERT OR REPLACE INTO concept_mentions (doc_id, concept_slug, confidence)
			 VALUES (?1, ?2, ?3)",
			rusqlite::params![doc_id, concept_slug, confidence],
		)?;
		Ok(())
	}

	/// Delete all concept mentions for a document (for re-enrichment).
	pub async fn delete_concept_mentions(&self, doc_id: &str) -> Result<()> {
		let conn = self.conn.lock().await;
		conn.execute("DELETE FROM concept_mentions WHERE doc_id = ?1", [doc_id])?;
		Ok(())
	}

	/// Delete all documents, refs, concept mentions, and sync state for a given
	/// source type.  Returns the number of documents deleted.
	pub async fn reset_source_type(
		&self, source_type: &str, sync_id_patterns: &[String],
	) -> Result<u64> {
		let conn = self.conn.lock().await;

		conn.execute_batch("BEGIN IMMEDIATE")?;

		// Delete concept mentions for matching documents.
		conn.execute(
			"DELETE FROM concept_mentions WHERE doc_id IN \
			 (SELECT id FROM documents WHERE source_type = ?1)",
			[source_type],
		)?;

		// Delete outgoing refs from matching documents.
		conn.execute(
			"DELETE FROM refs WHERE from_doc_id IN \
			 (SELECT id FROM documents WHERE source_type = ?1)",
			[source_type],
		)?;

		// Delete incoming refs pointing at matching documents.
		conn.execute(
			"DELETE FROM refs WHERE to_doc_id IN \
			 (SELECT id FROM documents WHERE source_type = ?1)",
			[source_type],
		)?;

		// Delete change log entries for matching documents.
		conn.execute(
			"DELETE FROM change_log WHERE doc_id IN \
			 (SELECT id FROM documents WHERE source_type = ?1)",
			[source_type],
		)?;

		// Delete the documents themselves.
		let deleted =
			conn.execute("DELETE FROM documents WHERE source_type = ?1", [source_type])? as u64;

		// Reset matching sync state entries.
		for pattern in sync_id_patterns {
			if pattern.contains('%') {
				conn.execute("DELETE FROM sync_state WHERE source_id LIKE ?1", [pattern])?;
			} else {
				conn.execute("DELETE FROM sync_state WHERE source_id = ?1", [pattern])?;
			}
		}

		conn.execute_batch("COMMIT")?;

		Ok(deleted)
	}

	/// Return all document IDs and bodies for a given source type, for
	/// re-enrichment.  Streams in batches to avoid loading everything into
	/// memory at once.
	pub async fn docs_for_reenrich(
		&self, source_type: &str,
	) -> Result<Vec<(String, Option<String>, Option<String>)>> {
		let conn = self.conn.lock().await;
		let mut stmt =
			conn.prepare("SELECT id, body, source_repo FROM documents WHERE source_type = ?1")?;
		let rows = stmt
			.query_map([source_type], |row| {
				Ok((
					row.get::<_, String>(0)?,
					row.get::<_, Option<String>>(1)?,
					row.get::<_, Option<String>>(2)?,
				))
			})?
			.collect::<rusqlite::Result<Vec<_>>>()?;
		Ok(rows)
	}

	/// Read a bounded raw page for the resumable cross-forge upgrade job.
	pub async fn reconciliation_page(&self, after: i64) -> Result<Vec<(i64, Document)>> {
		let conn = self.conn.lock().await;
		let mut stmt=conn.prepare("SELECT rowid,id FROM documents WHERE rowid>?1 AND (source_type GLOB 'github_*' OR source_type GLOB 'forgejo_*' OR source_type='commit') ORDER BY rowid LIMIT 200")?;
		let rows = stmt
			.query_map([after], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
			.collect::<rusqlite::Result<Vec<_>>>()?;
		rows.into_iter()
			.map(|(rowid, id)| Ok((rowid, crate::reconcile::raw(&conn, &id)?.unwrap())))
			.collect()
	}

	/// Counts for monitoring the raw/derived boundary and missing mirror targets.
	pub async fn reconciliation_stats(&self) -> Result<Vec<(&'static str, i64)>> {
		let conn = self.conn.lock().await;
		let mut stats = vec![];
		for (name,sql) in [
			("raw_documents","SELECT COUNT(*) FROM documents"),
			("canonical_documents","SELECT COUNT(*) FROM documents WHERE id NOT IN (SELECT doc_id FROM document_aliases WHERE doc_id!=canonical_id)"),
			("verified_links","SELECT COUNT(*) FROM document_links"),
			("unresolved_links","SELECT COUNT(*) FROM document_links l LEFT JOIN document_aliases a ON a.doc_id=l.doc_id LEFT JOIN document_aliases b ON b.doc_id=l.target_id WHERE a.canonical_id IS NULL OR b.canonical_id IS NULL OR a.canonical_id!=b.canonical_id")
		] { stats.push((name,conn.query_row(sql,[],|r|r.get(0))?)); }
		Ok(stats)
	}

	/// Get document counts grouped by source type.
	pub async fn get_stats(&self) -> Result<Vec<(String, i64)>> {
		let conn = self.conn.lock().await;
		let mut stmt = conn.prepare(
			"SELECT source_type, COUNT(*) FROM documents GROUP BY source_type ORDER BY COUNT(*) DESC",
		)?;
		let stats = stmt
			.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))?
			.collect::<rusqlite::Result<Vec<_>>>()?;
		Ok(stats)
	}

	/// Get all sync states.
	pub async fn get_all_sync_states(&self) -> Result<Vec<SyncState>> {
		let conn = self.conn.lock().await;
		let mut stmt = conn.prepare(
			"SELECT source_id, source_type, source_repo, last_cursor,
				last_synced_at, next_run_at, status, error_message,
				retry_count, items_found
			 FROM sync_state ORDER BY source_id",
		)?;
		let states = stmt
			.query_map([], |row| {
				Ok(SyncState {
					source_id: row.get(0)?,
					source_type: row.get(1)?,
					source_repo: row.get(2)?,
					last_cursor: row.get(3)?,
					last_synced_at: row
						.get::<_, Option<String>>(4)?
						.and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
						.map(|dt| dt.with_timezone(&chrono::Utc)),
					next_run_at: row
						.get::<_, Option<String>>(5)?
						.and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
						.map(|dt| dt.with_timezone(&chrono::Utc)),
					status: SyncStatus::from_str(row.get::<_, String>(6)?.as_str()),
					error_message: row.get(7)?,
					retry_count: row.get(8)?,
					items_found: row.get(9)?,
				})
			})?
			.collect::<rusqlite::Result<Vec<_>>>()?;
		Ok(states)
	}

	/// Compact the change log by deleting entries older than the given duration.
	pub async fn compact_change_log(&self, max_age: std::time::Duration) -> Result<u64> {
		let conn = self.conn.lock().await;
		let cutoff = chrono::Utc::now() - chrono::Duration::from_std(max_age)?;
		let deleted =
			conn.execute("DELETE FROM change_log WHERE changed_at < ?1", [cutoff.to_rfc3339()])?;
		Ok(deleted as u64)
	}

	/// Get or create sync state for a source.
	pub async fn get_sync_state(&self, source_id: &str) -> Result<Option<SyncState>> {
		let conn = self.conn.lock().await;
		let mut stmt = conn.prepare(
			"SELECT source_id, source_type, source_repo, last_cursor,
				last_synced_at, next_run_at, status, error_message,
				retry_count, items_found
			 FROM sync_state WHERE source_id = ?1",
		)?;

		let result = stmt
			.query_row([source_id], |row| {
				Ok(SyncState {
					source_id: row.get(0)?,
					source_type: row.get(1)?,
					source_repo: row.get(2)?,
					last_cursor: row.get(3)?,
					last_synced_at: row
						.get::<_, Option<String>>(4)?
						.and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
						.map(|dt| dt.with_timezone(&chrono::Utc)),
					next_run_at: row
						.get::<_, Option<String>>(5)?
						.and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
						.map(|dt| dt.with_timezone(&chrono::Utc)),
					status: SyncStatus::from_str(row.get::<_, String>(6)?.as_str()),
					error_message: row.get(7)?,
					retry_count: row.get(8)?,
					items_found: row.get(9)?,
				})
			})
			.optional()?;

		Ok(result)
	}

	/// Update sync state after a sync cycle.
	pub async fn update_sync_state(&self, state: &SyncState) -> Result<()> {
		let conn = self.conn.lock().await;
		Self::write_sync_state(&conn, state)
	}

	fn write_sync_state(conn: &Connection, state: &SyncState) -> Result<()> {
		conn.execute(
			"INSERT INTO sync_state (source_id, source_type, source_repo, last_cursor,
				last_synced_at, next_run_at, status, error_message, retry_count, items_found)
			 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
			 ON CONFLICT(source_id) DO UPDATE SET
				last_cursor = excluded.last_cursor,
				last_synced_at = excluded.last_synced_at,
				next_run_at = excluded.next_run_at,
				status = excluded.status,
				error_message = excluded.error_message,
				retry_count = excluded.retry_count,
				items_found = excluded.items_found",
			rusqlite::params![
				&state.source_id,
				&state.source_type,
				&state.source_repo,
				&state.last_cursor,
				state.last_synced_at.map(|t| t.to_rfc3339()),
				state.next_run_at.map(|t| t.to_rfc3339()),
				state.status.as_str(),
				&state.error_message,
				state.retry_count,
				state.items_found,
			],
		)?;
		Ok(())
	}
}

impl SqliteStore {
	/// Shared implementation for `lookup_bip` and `lookup_bolt`.
	///
	/// Finds the spec document by `source_type` and `source_id`, then collects all
	/// incoming references via `to_external LIKE '{PREFIX}-{number}'`.
	async fn lookup_spec(
		&self, source_type: SourceType, number: u32,
	) -> Result<Option<DocumentContext>> {
		let conn = self.conn.lock().await;
		let source_id = number.to_string();
		// Also try zero-padded form (e.g., "06") for LUDs/NUTs whose filenames
		// use two-digit padding.  This ensures lookups work regardless of
		// whether the data was ingested before or after the padding fix.
		let source_id_padded = format!("{:02}", number);

		// Find the spec document.
		let doc = conn
			.query_row(
				"SELECT id, source_type, source_repo, source_id, title, body,
				 author, author_id, created_at, updated_at, parent_id, metadata, seq
				 FROM documents WHERE source_type = ?1 AND source_id IN (?2, ?3)",
				rusqlite::params![source_type.as_str(), &source_id, &source_id_padded],
				|row| {
					let source_type_str: String = row.get(1)?;
					let created_at_str: String = row.get(8)?;
					let updated_at_str: Option<String> = row.get(9)?;
					let metadata_str: Option<String> = row.get(11)?;
					Ok(Document {
						id: row.get(0)?,
						source_type: SourceType::from_str(&source_type_str)
							.unwrap_or(SourceType::GithubIssue),
						source_repo: row.get(2)?,
						source_id: row.get(3)?,
						title: row.get(4)?,
						body: row.get(5)?,
						author: row.get(6)?,
						author_id: row.get(7)?,
						created_at: chrono::DateTime::parse_from_rfc3339(&created_at_str)
							.map(|dt| dt.with_timezone(&chrono::Utc))
							.unwrap_or_else(|_| chrono::Utc::now()),
						updated_at: updated_at_str.and_then(|s| {
							chrono::DateTime::parse_from_rfc3339(&s)
								.ok()
								.map(|dt| dt.with_timezone(&chrono::Utc))
						}),
						parent_id: row.get(10)?,
						metadata: metadata_str.and_then(|s| serde_json::from_str(&s).ok()),
						seq: row.get(12)?,
					})
				},
			)
			.optional()?;

		let doc = match doc {
			Some(d) => d,
			None => return Ok(None),
		};

		let url = doc.url();
		let doc_id = &doc.id;

		// Fetch outgoing refs from this document.
		let mut stmt = conn.prepare(
			"SELECT id, from_doc_id, to_doc_id, ref_type, to_external, context
			 FROM refs WHERE from_doc_id = ?1",
		)?;
		let outgoing_refs = stmt
			.query_map([doc_id], |row| {
				let ref_type_str: String = row.get(3)?;
				Ok(Reference {
					id: row.get(0)?,
					from_doc_id: row.get(1)?,
					to_doc_id: row.get(2)?,
					ref_type: RefType::from_str(&ref_type_str).unwrap_or(RefType::MentionsIssue),
					to_external: row.get(4)?,
					context: row.get(5)?,
				})
			})?
			.collect::<rusqlite::Result<Vec<_>>>()?;

		// Fetch incoming refs: both via to_doc_id (resolved) and to_external (unresolved).
		let prefix = match source_type {
			SourceType::Bip => "BIP",
			SourceType::Bolt => "BOLT",
			SourceType::Blip => "bLIP",
			SourceType::Lud => "LUD",
			SourceType::Nut => "NUT",
			_ => unreachable!(),
		};
		let external_pattern = format!("{}-{}", prefix, number);

		let mut stmt = conn.prepare(
			"SELECT id, from_doc_id, to_doc_id, ref_type, to_external, context
			 FROM refs WHERE to_doc_id = ?1 OR to_external = ?2",
		)?;
		let incoming_refs = stmt
			.query_map(rusqlite::params![doc_id, &external_pattern], |row| {
				let ref_type_str: String = row.get(3)?;
				Ok(Reference {
					id: row.get(0)?,
					from_doc_id: row.get(1)?,
					to_doc_id: row.get(2)?,
					ref_type: RefType::from_str(&ref_type_str).unwrap_or(RefType::MentionsIssue),
					to_external: row.get(4)?,
					context: row.get(5)?,
				})
			})?
			.collect::<rusqlite::Result<Vec<_>>>()?;

		// Fetch concept tags.
		let mut stmt =
			conn.prepare("SELECT concept_slug FROM concept_mentions WHERE doc_id = ?1")?;
		let concepts: Vec<String> =
			stmt.query_map([doc_id], |row| row.get(0))?.collect::<rusqlite::Result<Vec<_>>>()?;

		Ok(Some(DocumentContext {
			provenance: vec![],
			document: doc,
			url,
			outgoing_refs,
			incoming_refs,
			concepts,
		}))
	}
}

/// Helper trait for `rusqlite::OptionalExtension`-like behavior.
trait OptionalRow<T> {
	fn optional(self) -> rusqlite::Result<Option<T>>;
}

impl<T> OptionalRow<T> for rusqlite::Result<T> {
	fn optional(self) -> rusqlite::Result<Option<T>> {
		match self {
			Ok(v) => Ok(Some(v)),
			Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
			Err(e) => Err(e),
		}
	}
}

#[async_trait]
impl KnowledgeStore for SqliteStore {
	async fn search(&self, params: SearchParams) -> Result<SearchResults> {
		let conn = self.conn.lock().await;
		let deduplicate = params.deduplicate != Some(false);
		let limit = params.limit.unwrap_or(20).min(100);

		let query_trimmed = params.query.trim();
		let is_wildcard = query_trimmed.is_empty() || query_trimmed == "*";

		// Wildcard queries require at least one filter to avoid full table scans.
		if is_wildcard {
			let has_filter = params.source_type.as_ref().is_some_and(|v| !v.is_empty())
				|| params.source_repo.as_ref().is_some_and(|v| !v.is_empty())
				|| params.author.is_some()
				|| params.after.is_some()
				|| params.before.is_some();
			if !has_filter {
				anyhow::bail!(
					"wildcard queries require at least one filter \
					 (source_type, source_repo, author, after, or before)"
				);
			}
		}

		let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
		let mut param_idx = 1;

		let mut sql = if is_wildcard {
			// No FTS -- query directly from documents table
			String::from(
				"SELECT d.id, d.source_type, d.source_repo, d.title,
					NULL as snippet,
					d.author, d.created_at,
					0.0 as score,
					d.source_id, d.parent_id, d.metadata
				 FROM documents d
				 WHERE 1=1",
			)
		} else {
			// Full-text search via FTS5
			let fts_query = build_fts_query(query_trimmed);
			param_values.push(Box::new(fts_query));
			param_idx = 2;
			String::from(
				"SELECT d.id, d.source_type, d.source_repo, d.title,
					snippet(documents_fts, 1, '<mark>', '</mark>', '...', 64) as snippet,
					d.author, d.created_at,
					bm25(documents_fts, 5.0, 1.0) as score,
					d.source_id, d.parent_id, d.metadata
				 FROM documents_fts
				 JOIN documents d ON d.rowid = documents_fts.rowid
				 WHERE documents_fts MATCH ?1",
			)
		};

		let filter_start = sql.len();
		// Source type filter
		if let Some(ref source_types) = params.source_type {
			if !source_types.is_empty() {
				let placeholders: Vec<String> = source_types
					.iter()
					.enumerate()
					.map(|(i, _)| format!("?{}", param_idx + i))
					.collect();
				sql.push_str(&format!(" AND d.source_type IN ({})", placeholders.join(",")));
				for st in source_types {
					param_values.push(Box::new(st.as_str().to_string()));
					param_idx += 1;
				}
			}
		}

		// Source repo filter
		if let Some(ref repos) = params.source_repo {
			let mut repos: Vec<String> = repos
				.iter()
				.map(|repo| repo.strip_prefix("github.com/").unwrap_or(repo).to_owned())
				.collect();
			if deduplicate {
				for repo in params.source_repo.as_ref().unwrap() {
					if bkb_core::repository::is_migrated(repo) {
						repos.push(bkb_core::repository::forgejo_identity(repo));
					}
				}
			}
			if !repos.is_empty() {
				let placeholders: Vec<String> =
					repos.iter().enumerate().map(|(i, _)| format!("?{}", param_idx + i)).collect();
				sql.push_str(&format!(" AND d.source_repo IN ({})", placeholders.join(",")));
				for repo in &repos {
					param_values.push(Box::new(repo.clone()));
					param_idx += 1;
				}
			}
		}

		// Author filter
		if let Some(ref author) = params.author {
			sql.push_str(&format!(" AND d.author = ?{}", param_idx));
			param_values.push(Box::new(author.clone()));
			param_idx += 1;
		}

		// Date filters
		if let Some(ref after) = params.after {
			sql.push_str(&format!(" AND d.created_at >= ?{}", param_idx));
			param_values.push(Box::new(after.to_rfc3339()));
			param_idx += 1;
		}

		if let Some(ref before) = params.before {
			sql.push_str(&format!(" AND d.created_at <= ?{}", param_idx));
			param_values.push(Box::new(before.to_rfc3339()));
			let _ = param_idx;
		}

		if deduplicate {
			let filters = sql.split_off(filter_start).replace("d.", "f.");
			if !filters.is_empty() {
				// Build the matching canonical-ID set once. A correlated EXISTS
				// would rescan matching filter rows for every FTS or wildcard hit.
				sql.push_str(&format!(" AND COALESCE(a.canonical_id,d.id) IN (SELECT COALESCE(fa.canonical_id,f.id) FROM documents f LEFT JOIN document_aliases fa ON fa.doc_id=f.id WHERE 1=1{filters})"));
			}
			sql = sql.replace("d.source_id, d.parent_id, d.metadata", "d.source_id, d.parent_id, d.metadata, COALESCE(a.canonical_id,d.id) AS canonical_id");
			// Both wildcard and FTS queries retain the original matching row for
			// snippet provenance. Materialize bm25 before applying a window.
			let where_pos = sql.find(" WHERE ").unwrap();
			sql.insert_str(where_pos, " LEFT JOIN document_aliases a ON a.doc_id=d.id ");
			sql = format!("WITH hits AS MATERIALIZED ({sql}), ranked AS (SELECT *, ROW_NUMBER() OVER (PARTITION BY canonical_id ORDER BY score,id) AS duplicate_rank FROM hits) SELECT * FROM ranked WHERE duplicate_rank=1");
		}
		sql.push_str(if is_wildcard {
			" ORDER BY created_at DESC, id LIMIT ?"
		} else {
			" ORDER BY score, id LIMIT ?"
		});
		param_values.push(Box::new(limit as i64));

		let param_refs: Vec<&dyn rusqlite::types::ToSql> =
			param_values.iter().map(|b| b.as_ref()).collect();

		let mut stmt = conn.prepare(&sql)?;

		let results: Vec<SearchResult> = stmt
			.query_map(param_refs.as_slice(), |row| {
				let source_type_str: String = row.get(1)?;
				let created_at_str: String = row.get(6)?;
				let source_type =
					SourceType::from_str(&source_type_str).unwrap_or(SourceType::GithubIssue);
				let source_repo: Option<String> = row.get(2)?;
				let source_id: String = row.get(8)?;
				let parent_id: Option<String> = row.get(9)?;
				let metadata_str: Option<String> = row.get(10)?;
				let doc = Document {
					id: String::new(),
					source_type: source_type.clone(),
					source_repo: source_repo.clone(),
					source_id,
					title: None,
					body: None,
					author: None,
					author_id: None,
					created_at: chrono::Utc::now(),
					updated_at: None,
					parent_id,
					metadata: metadata_str.and_then(|s| serde_json::from_str(&s).ok()),
					seq: None,
				};
				Ok(SearchResult {
					provenance: vec![],
					matched_source_id: None,
					id: row.get(0)?,
					source_type,
					source_repo,
					title: row.get(3)?,
					snippet: row.get(4)?,
					author: row.get(5)?,
					created_at: chrono::DateTime::parse_from_rfc3339(&created_at_str)
						.map(|dt| dt.with_timezone(&chrono::Utc))
						.unwrap_or_else(|_| chrono::Utc::now()),
					score: row.get::<_, f64>(7)?.abs(),
					url: doc.url(),
					concepts: Vec::new(),
				})
			})?
			.collect::<rusqlite::Result<Vec<_>>>()?;

		// Populate concept tags for each result
		let mut concept_stmt =
			conn.prepare("SELECT concept_slug FROM concept_mentions WHERE doc_id = ?1")?;
		let results: Vec<SearchResult> = results
			.into_iter()
			.map(|mut r| {
				if let Ok(concepts) = concept_stmt
					.query_map([&r.id], |row| row.get::<_, String>(0))
					.and_then(|rows| rows.collect::<rusqlite::Result<Vec<_>>>())
				{
					r.concepts = concepts;
				}
				r
			})
			.collect();

		let mut results = results;
		if deduplicate {
			for result in &mut results {
				let doc = crate::reconcile::projected(&conn, &result.id)?.unwrap();
				result.url = doc.url();
				result.provenance = crate::reconcile::origins(&conn, &doc.id)?;
				result.concepts = crate::reconcile::concepts(&conn, &doc.id)?;
				result.matched_source_id = Some(result.id.clone());
				result.id = doc.id;
				result.source_type = doc.source_type;
				result.source_repo = doc.source_repo;
				result.title = doc.title;
				result.author = doc.author;
				result.created_at = doc.created_at;
			}
		}
		let total_count = results.len() as u32;
		Ok(SearchResults { results, total_count })
	}

	async fn get_document(&self, id: &str) -> Result<Option<DocumentContext>> {
		let conn = self.conn.lock().await;
		crate::reconcile::context(&conn, id, true)
	}

	async fn get_raw_document(&self, id: &str) -> Result<Option<DocumentContext>> {
		let conn = self.conn.lock().await;
		crate::reconcile::context(&conn, id, false)
	}
	async fn get_references(
		&self, entity: &str, ref_type: Option<&str>, limit: u32,
	) -> Result<Vec<Reference>> {
		let conn = self.conn.lock().await;
		let target = crate::reconcile::external_id(&conn, entity)?
			.unwrap_or(crate::reconcile::canonical_id(&conn, entity)?);
		let mut stmt=conn.prepare("SELECT r.id,r.from_doc_id,r.to_doc_id,r.ref_type,r.to_external,r.context FROM refs r LEFT JOIN document_aliases fa ON fa.doc_id=r.from_doc_id LEFT JOIN document_aliases ta ON ta.doc_id=r.to_doc_id WHERE (r.to_external=?1 OR COALESCE(ta.canonical_id,r.to_doc_id)=?2) AND (?3 IS NULL OR r.ref_type=?3) GROUP BY COALESCE(fa.canonical_id,r.from_doc_id),COALESCE(ta.canonical_id,r.to_doc_id,r.to_external),r.ref_type ORDER BY MIN(r.id) LIMIT ?4")?;
		let refs = stmt
			.query_map(
				rusqlite::params![entity, target, ref_type, limit.min(1000)],
				crate::reconcile::reference_row,
			)?
			.collect::<rusqlite::Result<Vec<_>>>()?;
		crate::reconcile::canonical_refs(&conn, refs)
	}
	async fn lookup_bip(&self, number: u32) -> Result<Option<DocumentContext>> {
		self.lookup_spec(SourceType::Bip, number).await
	}

	async fn lookup_bolt(&self, number: u32) -> Result<Option<DocumentContext>> {
		self.lookup_spec(SourceType::Bolt, number).await
	}

	async fn lookup_blip(&self, number: u32) -> Result<Option<DocumentContext>> {
		self.lookup_spec(SourceType::Blip, number).await
	}

	async fn lookup_lud(&self, number: u32) -> Result<Option<DocumentContext>> {
		self.lookup_spec(SourceType::Lud, number).await
	}

	async fn lookup_nut(&self, number: u32) -> Result<Option<DocumentContext>> {
		self.lookup_spec(SourceType::Nut, number).await
	}

	async fn timeline(
		&self, concept: &str, after: Option<chrono::DateTime<chrono::Utc>>,
		before: Option<chrono::DateTime<chrono::Utc>>,
	) -> Result<bkb_core::model::Timeline> {
		let conn = self.conn.lock().await;
		let mut stmt=conn.prepare("SELECT COALESCE(a.canonical_id,d.id) AS canonical FROM concept_mentions cm JOIN documents d ON d.id=cm.doc_id LEFT JOIN document_aliases a ON a.doc_id=d.id WHERE cm.concept_slug=?1 AND (?2 IS NULL OR d.created_at>=?2) AND (?3 IS NULL OR d.created_at<=?3) GROUP BY canonical ORDER BY MIN(d.created_at),canonical LIMIT 200")?;
		let ids = stmt
			.query_map(
				rusqlite::params![
					concept,
					after.map(|d| d.to_rfc3339()),
					before.map(|d| d.to_rfc3339())
				],
				|r| r.get::<_, String>(0),
			)?
			.collect::<rusqlite::Result<Vec<_>>>()?;
		let mut events = vec![];
		for id in ids {
			let doc = crate::reconcile::projected(&conn, &id)?.unwrap();
			events.push(bkb_core::model::TimelineEvent {
				url: doc.url(),
				date: doc.created_at.format("%Y-%m-%d").to_string(),
				id: doc.id,
				source_type: doc.source_type,
				title: doc.title,
			});
		}
		Ok(bkb_core::model::Timeline { concept: concept.into(), events })
	}
	async fn find_commit(
		&self, query: &str, repo: Option<&str>,
	) -> Result<Vec<bkb_core::model::CommitContext>> {
		// Search commits first, then also search PRs as fallback context.
		let mut source_types = vec![SourceType::Commit];
		source_types.push(SourceType::GithubPr);
		source_types.push(SourceType::ForgejoPr);

		let params = SearchParams {
			query: query.to_string(),
			source_type: Some(source_types),
			source_repo: repo.map(|r| vec![r.to_string()]),
			limit: Some(10),
			..Default::default()
		};

		let results = self.search(params).await?;

		let mut contexts = Vec::new();
		for result in results.results {
			let doc_ctx = self.get_document(&result.id).await?;
			if let Some(ctx) = doc_ctx {
				// For commit documents, look up associated PRs that mention
				// this commit SHA in their body or via references.
				let associated_prs = if ctx.document.source_type == SourceType::Commit {
					self.find_associated_prs(
						&ctx.document.source_id,
						ctx.document.source_repo.as_deref(),
					)
					.await
					.unwrap_or_default()
				} else {
					Vec::new()
				};

				contexts.push(bkb_core::model::CommitContext {
					url: ctx.url.clone(),
					document: ctx.document,
					associated_prs,
				});
			}
		}

		Ok(contexts)
	}
}

impl SqliteStore {
	/// Prefer exact merge metadata; explicit SHA mentions remain useful context.
	async fn find_associated_prs(
		&self, commit_sha: &str, source_repo: Option<&str>,
	) -> Result<Vec<SearchResult>> {
		let conn = self.conn.lock().await;
		let fj_repo = source_repo
			.filter(|r| bkb_core::repository::is_migrated(r))
			.map(bkb_core::repository::forgejo_identity);
		let fts = build_fts_query(&commit_sha[..commit_sha.len().min(12)]);
		let mut stmt=conn.prepare("SELECT COALESCE(a.canonical_id,d.id) AS canonical FROM documents d LEFT JOIN document_aliases a ON a.doc_id=d.id WHERE d.source_type IN ('github_pr','forgejo_pr') AND (?1 IS NULL OR d.source_repo IN (?1,?2)) AND (json_extract(d.metadata,'$.merge_commit_sha')=?3 OR d.rowid IN (SELECT rowid FROM documents_fts WHERE documents_fts MATCH ?4)) GROUP BY canonical ORDER BY MAX(json_extract(d.metadata,'$.merge_commit_sha')=?3) DESC,canonical LIMIT 5")?;
		let ids = stmt
			.query_map(rusqlite::params![source_repo, fj_repo, commit_sha, fts], |r| {
				r.get::<_, String>(0)
			})?
			.collect::<rusqlite::Result<Vec<_>>>()?;
		let mut results = vec![];
		for id in ids {
			let ctx = crate::reconcile::context(&conn, &id, true)?.unwrap();
			let doc = ctx.document;
			results.push(SearchResult {
				id: doc.id,
				source_type: doc.source_type,
				source_repo: doc.source_repo,
				title: doc.title,
				snippet: None,
				author: doc.author,
				created_at: doc.created_at,
				score: 0.0,
				url: ctx.url,
				concepts: ctx.concepts,
				provenance: ctx.provenance,
				matched_source_id: None,
			});
		}
		Ok(results)
	}
}

/// Build an FTS5 query string from user input.
///
/// Escapes special characters and wraps terms for prefix matching.
fn build_fts_query(input: &str) -> String {
	let trimmed = input.trim();
	if trimmed.is_empty() {
		return "\"\"".to_string();
	}

	// Strip all double-quotes so that user input like `lightning:"channel close"`
	// cannot bypass per-term quoting and re-expose the `:` column-filter bug.
	let trimmed = &trimmed.replace('"', "");
	let trimmed = trimmed.trim();
	if trimmed.is_empty() {
		return "\"\"".to_string();
	}

	// Split into whitespace-delimited terms.
	let terms: Vec<&str> = trimmed.split_whitespace().collect();

	// A term is a whole-word FTS5 boolean keyword when it exactly matches
	// one of the reserved words.  Previous substring-based checks caused
	// false positives for words like "MONITOR" (contains "OR"), "HANDLER"
	// (contains "AND"), or "CANNOT" (contains "NOT").
	let is_fts_keyword = |t: &str| matches!(t, "AND" | "OR" | "NOT" | "NEAR");

	// Terms that contain FTS5 syntax characters need quoting so the engine
	// doesn't interpret them as column-filter (`:`) or other operators.
	let needs_quoting =
		|t: &str| t.contains(':') || t.contains('/') || t.contains('#') || t.contains('-');

	let quoted: Vec<String> = terms
		.iter()
		.map(|t| {
			if is_fts_keyword(t) || t.ends_with('*') {
				// Preserve intentional FTS5 operators and prefix globs.
				t.to_string()
			} else if needs_quoting(t) {
				format!("\"{}\"", t)
			} else {
				t.to_string()
			}
		})
		.collect();

	if quoted.len() == 1 {
		let term = &quoted[0];
		// Single term: use prefix matching unless already quoted/globbed.
		if term.starts_with('"') || term.ends_with('*') {
			term.clone()
		} else {
			format!("{}*", term)
		}
	} else {
		// Multiple terms: implicit AND (space-separated).
		quoted.join(" ")
	}
}

#[cfg(test)]
mod tests {
	use chrono::Utc;

	use super::*;
	use bkb_core::model::{Document, SourceType};

	fn test_doc(id: &str, title: &str, body: &str) -> Document {
		Document {
			id: id.to_string(),
			source_type: SourceType::GithubIssue,
			source_repo: Some("bitcoin/bitcoin".to_string()),
			source_id: "1".to_string(),
			title: Some(title.to_string()),
			body: Some(body.to_string()),
			author: Some("satoshi".to_string()),
			author_id: None,
			created_at: Utc::now(),
			updated_at: None,
			parent_id: None,
			metadata: None,
			seq: None,
		}
	}

	#[tokio::test]
	async fn test_upsert_and_get_document() {
		let store = SqliteStore::open_in_memory().unwrap();
		let doc =
			test_doc("github_issue:bitcoin/bitcoin:1", "Test issue", "This is a test issue body");

		store.upsert_document(&doc).await.unwrap();

		let ctx = store.get_document("github_issue:bitcoin/bitcoin:1").await.unwrap();
		assert!(ctx.is_some());
		let ctx = ctx.unwrap();
		assert_eq!(ctx.document.title.as_deref(), Some("Test issue"));
		assert_eq!(ctx.document.body.as_deref(), Some("This is a test issue body"));
	}

	#[tokio::test]
	async fn test_upsert_updates_existing() {
		let store = SqliteStore::open_in_memory().unwrap();
		let doc = test_doc("github_issue:bitcoin/bitcoin:1", "Original title", "Original body");
		store.upsert_document(&doc).await.unwrap();

		let mut updated = doc.clone();
		updated.title = Some("Updated title".to_string());
		store.upsert_document(&updated).await.unwrap();

		let ctx = store.get_document("github_issue:bitcoin/bitcoin:1").await.unwrap().unwrap();
		assert_eq!(ctx.document.title.as_deref(), Some("Updated title"));
	}

	#[tokio::test]
	async fn test_search_fts() {
		let store = SqliteStore::open_in_memory().unwrap();

		let doc1 = test_doc(
			"github_issue:bitcoin/bitcoin:1",
			"Add taproot support",
			"Implementing BIP-340 and BIP-341 for schnorr signatures",
		);
		let mut doc2 = test_doc(
			"github_issue:bitcoin/bitcoin:2",
			"Fix mempool bug",
			"There is a bug in the mempool validation logic",
		);
		doc2.source_id = "2".to_string();

		store.upsert_document(&doc1).await.unwrap();
		store.upsert_document(&doc2).await.unwrap();

		let results = store
			.search(SearchParams { query: "taproot".to_string(), ..Default::default() })
			.await
			.unwrap();

		assert_eq!(results.results.len(), 1);
		assert!(results.results[0].title.as_deref().unwrap().contains("taproot"));
	}

	#[tokio::test]
	async fn test_filtered_fts_search_instruction_budget() {
		assert_filtered_search_instruction_budget("taproot").await;
	}

	#[tokio::test]
	async fn test_filtered_wildcard_search_instruction_budget() {
		assert_filtered_search_instruction_budget("*").await;
	}

	async fn assert_filtered_search_instruction_budget(query: &str) {
		let store = SqliteStore::open_in_memory().unwrap();
		for number in 0..1024 {
			let mut doc = test_doc(
				&format!("github_issue:bitcoin/bitcoin:{number}"),
				"Taproot support",
				"Implementing taproot",
			);
			doc.source_id = number.to_string();
			store.upsert_document(&doc).await.unwrap();
		}

		// Count SQLite VM instructions rather than elapsed time. This budget
		// leaves room for sorting and projection, but rejects rescanning all
		// matching filter rows for each result, even without any duplicates.
		let mut remaining_callbacks = 500;
		store.conn.lock().await.progress_handler(
			1000,
			Some(move || {
				remaining_callbacks -= 1;
				remaining_callbacks == 0
			}),
		);
		let results = store
			.search(SearchParams {
				query: query.to_string(),
				author: Some("satoshi".into()),
				limit: Some(20),
				..Default::default()
			})
			.await;
		store.conn.lock().await.progress_handler(0, None::<fn() -> bool>);
		assert!(
			results.is_ok(),
			"filtered search must finish within 500,000 SQLite VM instructions: {results:?}"
		);
		assert_eq!(results.unwrap().results.len(), 20);
	}

	#[tokio::test]
	async fn test_canonical_filters_match_one_member_independently_of_fts() {
		let store = SqliteStore::open_in_memory().unwrap();
		let mut matching_origin = String::new();
		let mut matching_canonical = String::new();
		for (number, github_author, forgejo_author) in [(1, "bob", "alice"), (2, "alice", "bob")] {
			let mut gh = test_doc("", "Historical title", "Taproot implementation");
			gh.source_repo = Some("lightningdevkit/rust-lightning".into());
			gh.source_id = number.to_string();
			gh.id = Document::make_id(&gh.source_type, gh.source_repo.as_deref(), &gh.source_id);
			gh.author = Some(github_author.into());
			let mut fj = gh.clone();
			fj.source_type = SourceType::ForgejoIssue;
			fj.source_repo =
				Some(bkb_core::repository::forgejo_identity("lightningdevkit/rust-lightning"));
			fj.id = Document::make_id(&fj.source_type, fj.source_repo.as_deref(), &fj.source_id);
			fj.body = Some("Current discussion".into());
			fj.author = Some(forgejo_author.into());
			store.upsert_document(&gh).await.unwrap();
			store.upsert_document(&fj).await.unwrap();
			// Model verified imports whose text and authors differ across hosts.
			for id in [&gh.id, &fj.id] {
				store.conn.lock().await.execute(
					"INSERT INTO document_aliases (doc_id, canonical_id, origin_id, evidence) VALUES (?1, ?2, ?3, 'test')",
					rusqlite::params![id, fj.id, gh.id],
				).unwrap();
			}
			if number == 1 {
				matching_origin = gh.id;
				matching_canonical = fj.id;
			}
		}

		for query in ["taproot", "*"] {
			let params = SearchParams {
				query: query.into(),
				source_type: Some(vec![SourceType::ForgejoIssue]),
				author: Some("alice".into()),
				..Default::default()
			};
			let results = store.search(params.clone()).await.unwrap();
			assert_eq!(results.results.len(), 1, "both filters must match the same member");
			assert_eq!(results.results[0].id, matching_canonical);
			if query == "taproot" {
				assert_eq!(
					results.results[0].matched_source_id.as_deref(),
					Some(matching_origin.as_str())
				);
				assert!(results.results[0].snippet.as_deref().unwrap().contains("Taproot"));
			}

			let raw =
				store.search(SearchParams { deduplicate: Some(false), ..params }).await.unwrap();
			if query == "taproot" {
				assert!(raw.results.is_empty(), "raw search requires FTS and filters on one row");
			} else {
				assert_eq!(raw.results.len(), 1);
				assert_eq!(raw.results[0].id, matching_canonical);
			}
		}
	}

	/// Regression test: queries containing `:` must not trigger FTS5
	/// "no such column" errors (`:` is the column-filter operator).
	#[tokio::test]
	async fn test_search_fts_with_colon_in_query() {
		let store = SqliteStore::open_in_memory().unwrap();

		let mut doc = test_doc(
			"github_pr:lightningdevkit/ldk-node:4463",
			"Fix lightning channel close",
			"Resolves an issue with force-close in ldk-node",
		);
		doc.source_type = SourceType::GithubPr;
		doc.source_repo = Some("lightningdevkit/ldk-node".to_string());
		doc.source_id = "4463".to_string();
		store.upsert_document(&doc).await.unwrap();

		// Query containing `:` — previously caused "no such column: lightning".
		let results = store
			.search(SearchParams {
				query: "lightningdevkit/ldk-node:4463".to_string(),
				..Default::default()
			})
			.await;
		assert!(results.is_ok(), "search with colon must not fail: {:?}", results.err());

		// Also verify the document can be retrieved by its ID (which contains `/`).
		let ctx = store.get_document("github_pr:lightningdevkit/ldk-node:4463").await.unwrap();
		assert!(ctx.is_some());
		assert_eq!(ctx.unwrap().document.source_id, "4463");
	}

	/// Regression test: words containing "OR"/"AND"/"NOT" as substrings
	/// (e.g. "ChannelMonitor", "ChannelManager") must not trigger the
	/// pass-through path that skips quoting.
	#[tokio::test]
	async fn test_search_fts_operator_substring_false_positive() {
		let store = SqliteStore::open_in_memory().unwrap();

		let doc = test_doc(
			"github_issue:lightningdevkit/rust-lightning:1",
			"ChannelMonitor persistence ordering",
			"The ChannelManager and ChannelMonitor must be persisted in the right order",
		);
		store.upsert_document(&doc).await.unwrap();

		// "ChannelMonitor" contains "OR", "ChannelManager" contains "AND" —
		// these must NOT cause the query to be passed through raw.
		let results = store
			.search(SearchParams {
				query: "ChannelMonitor ChannelManager ordering".to_string(),
				..Default::default()
			})
			.await;
		assert!(
			results.is_ok(),
			"search with OR/AND substrings must not fail: {:?}",
			results.err()
		);
		assert!(!results.unwrap().results.is_empty());
	}

	/// Regression test: hyphenated terms like `rust-lightning` must not be
	/// interpreted as `rust NOT lightning` by FTS5.
	#[tokio::test]
	async fn test_search_fts_hyphenated_term() {
		let store = SqliteStore::open_in_memory().unwrap();

		let doc = test_doc(
			"github_issue:lightningdevkit/rust-lightning:42",
			"rust-lightning bug fix",
			"Fix a bug in rust-lightning channel handling",
		);
		store.upsert_document(&doc).await.unwrap();

		let results = store
			.search(SearchParams { query: "rust-lightning".to_string(), ..Default::default() })
			.await;
		assert!(results.is_ok(), "hyphenated search must not fail: {:?}", results.err());
	}

	#[tokio::test]
	async fn test_search_fts_quotes_with_colon() {
		// Regression: `lightning:"channel close"` previously bypassed per-term
		// quoting, exposing the `:` column-filter operator to FTS5.
		let store = SqliteStore::open_in_memory().unwrap();

		let doc = test_doc(
			"github_issue:lightningdevkit/rust-lightning:99",
			"lightning channel close",
			"Details about closing a lightning channel",
		);
		store.upsert_document(&doc).await.unwrap();

		let results = store
			.search(SearchParams {
				query: "lightning:\"channel close\"".to_string(),
				..Default::default()
			})
			.await;
		assert!(results.is_ok(), "quoted+colon search must not crash: {:?}", results.err());
	}

	#[test]
	fn test_build_fts_query_strips_quotes() {
		// Quotes are stripped so `:` still gets properly quoted.
		assert_eq!(build_fts_query("lightning:\"channel close\""), "\"lightning:channel\" close");

		// Pure quotes collapse to empty.
		assert_eq!(build_fts_query("\"\""), "\"\"");

		// Normal input unchanged.
		assert_eq!(build_fts_query("hello world"), "hello world");
	}

	#[tokio::test]
	async fn test_get_nonexistent_document() {
		let store = SqliteStore::open_in_memory().unwrap();
		let result = store.get_document("nonexistent:id").await.unwrap();
		assert!(result.is_none());
	}

	#[tokio::test]
	async fn test_references() {
		let store = SqliteStore::open_in_memory().unwrap();

		let doc = test_doc("github_issue:bitcoin/bitcoin:1", "Test", "Body");
		store.upsert_document(&doc).await.unwrap();

		let reference = Reference {
			id: None,
			from_doc_id: "github_issue:bitcoin/bitcoin:1".to_string(),
			to_doc_id: None,
			ref_type: RefType::ReferencesBip,
			to_external: Some("BIP-340".to_string()),
			context: Some("Implementing BIP-340 schnorr".to_string()),
		};
		store.insert_reference(&reference).await.unwrap();

		let ctx = store.get_document("github_issue:bitcoin/bitcoin:1").await.unwrap().unwrap();
		assert_eq!(ctx.outgoing_refs.len(), 1);
		assert_eq!(ctx.outgoing_refs[0].ref_type, RefType::ReferencesBip);
	}

	#[tokio::test]
	async fn test_sync_state() {
		let store = SqliteStore::open_in_memory().unwrap();

		let state = SyncState {
			source_id: "github:bitcoin/bitcoin:issues".to_string(),
			source_type: "github_issue".to_string(),
			source_repo: Some("bitcoin/bitcoin".to_string()),
			last_cursor: Some("2024-01-01T00:00:00Z".to_string()),
			last_synced_at: Some(Utc::now()),
			next_run_at: None,
			status: SyncStatus::Ok,
			error_message: None,
			retry_count: 0,
			items_found: 42,
		};

		store.update_sync_state(&state).await.unwrap();

		let retrieved = store.get_sync_state("github:bitcoin/bitcoin:issues").await.unwrap();
		assert!(retrieved.is_some());
		let retrieved = retrieved.unwrap();
		assert_eq!(retrieved.items_found, 42);
		assert_eq!(retrieved.status, SyncStatus::Ok);
	}

	#[tokio::test]
	async fn test_lookup_lud() {
		let store = SqliteStore::open_in_memory().unwrap();

		// Store a LUD with unpadded source_id (as the sync adapter now does)
		let doc = Document {
			id: Document::make_id(&SourceType::Lud, None, "6"),
			source_type: SourceType::Lud,
			source_repo: None,
			source_id: "6".to_string(),
			title: Some("LUD-06: lnurl-pay".to_string()),
			body: Some("# lnurl-pay\n\nSpec content here.".to_string()),
			author: None,
			author_id: None,
			created_at: Utc::now(),
			updated_at: None,
			parent_id: None,
			metadata: Some(
				serde_json::json!({ "url": "https://github.com/lnurl/luds/blob/luds/06.md" }),
			),
			seq: None,
		};
		store.upsert_document(&doc).await.unwrap();

		let ctx = store.lookup_lud(6).await.unwrap();
		assert!(ctx.is_some(), "lookup_lud(6) should find the document");
		let ctx = ctx.unwrap();
		assert_eq!(ctx.document.title.as_deref(), Some("LUD-06: lnurl-pay"));
		assert_eq!(ctx.url.as_deref(), Some("https://github.com/lnurl/luds/blob/luds/06.md"));
	}

	#[tokio::test]
	async fn test_lookup_lud_padded_source_id() {
		let store = SqliteStore::open_in_memory().unwrap();

		// Simulate old data with zero-padded source_id
		let doc = Document {
			id: Document::make_id(&SourceType::Lud, None, "06"),
			source_type: SourceType::Lud,
			source_repo: None,
			source_id: "06".to_string(),
			title: Some("LUD-06: lnurl-pay".to_string()),
			body: Some("# lnurl-pay\n\nSpec content here.".to_string()),
			author: None,
			author_id: None,
			created_at: Utc::now(),
			updated_at: None,
			parent_id: None,
			metadata: None,
			seq: None,
		};
		store.upsert_document(&doc).await.unwrap();

		// lookup_lud(6) should still find it even though source_id is "06"
		let ctx = store.lookup_lud(6).await.unwrap();
		assert!(ctx.is_some(), "lookup_lud(6) should find padded source_id '06'");
		assert_eq!(ctx.unwrap().document.title.as_deref(), Some("LUD-06: lnurl-pay"));
	}

	#[tokio::test]
	async fn test_lookup_nut_zero() {
		let store = SqliteStore::open_in_memory().unwrap();

		// NUT-00 is a real spec — source_id is "0"
		let doc = Document {
			id: Document::make_id(&SourceType::Nut, None, "0"),
			source_type: SourceType::Nut,
			source_repo: None,
			source_id: "0".to_string(),
			title: Some("NUT-00: Notation, Usage, and Terminology".to_string()),
			body: Some("# NUT-00\n\nSpec content here.".to_string()),
			author: None,
			author_id: None,
			created_at: Utc::now(),
			updated_at: None,
			parent_id: None,
			metadata: Some(
				serde_json::json!({ "url": "https://github.com/cashubtc/nuts/blob/main/00.md" }),
			),
			seq: None,
		};
		store.upsert_document(&doc).await.unwrap();

		let ctx = store.lookup_nut(0).await.unwrap();
		assert!(ctx.is_some(), "lookup_nut(0) should find NUT-00");
		let ctx = ctx.unwrap();
		assert_eq!(ctx.document.title.as_deref(), Some("NUT-00: Notation, Usage, and Terminology"));
		assert_eq!(ctx.url.as_deref(), Some("https://github.com/cashubtc/nuts/blob/main/00.md"));
	}

	#[tokio::test]
	async fn test_reset_source_type_only_deletes_targeted() {
		let store = SqliteStore::open_in_memory().unwrap();

		// Insert a LUD document
		let lud_doc = Document {
			id: Document::make_id(&SourceType::Lud, None, "6"),
			source_type: SourceType::Lud,
			source_repo: None,
			source_id: "6".to_string(),
			title: Some("LUD-06".to_string()),
			body: Some("lnurl-pay spec".to_string()),
			author: None,
			author_id: None,
			created_at: Utc::now(),
			updated_at: None,
			parent_id: None,
			metadata: None,
			seq: None,
		};
		store.upsert_document(&lud_doc).await.unwrap();

		// Insert a BIP document
		let bip_doc = Document {
			id: Document::make_id(&SourceType::Bip, None, "340"),
			source_type: SourceType::Bip,
			source_repo: None,
			source_id: "340".to_string(),
			title: Some("BIP-340".to_string()),
			body: Some("Schnorr signatures".to_string()),
			author: None,
			author_id: None,
			created_at: Utc::now(),
			updated_at: None,
			parent_id: None,
			metadata: None,
			seq: None,
		};
		store.upsert_document(&bip_doc).await.unwrap();

		// Insert a GitHub issue
		let issue_doc = Document {
			id: "github_issue:bitcoin/bitcoin:1".to_string(),
			source_type: SourceType::GithubIssue,
			source_repo: Some("bitcoin/bitcoin".to_string()),
			source_id: "1".to_string(),
			title: Some("Test issue".to_string()),
			body: Some("Mentions LUD-6 and BIP-340".to_string()),
			author: Some("alice".to_string()),
			author_id: None,
			created_at: Utc::now(),
			updated_at: None,
			parent_id: None,
			metadata: None,
			seq: None,
		};
		store.upsert_document(&issue_doc).await.unwrap();

		// Add a cross-reference from the issue to the LUD
		let lud_ref = Reference {
			id: None,
			from_doc_id: issue_doc.id.clone(),
			to_doc_id: None,
			ref_type: RefType::ReferencesLud,
			to_external: Some("LUD-6".to_string()),
			context: None,
		};
		store.insert_reference(&lud_ref).await.unwrap();

		// Add a cross-reference from the issue to the BIP
		let bip_ref = Reference {
			id: None,
			from_doc_id: issue_doc.id.clone(),
			to_doc_id: None,
			ref_type: RefType::ReferencesBip,
			to_external: Some("BIP-340".to_string()),
			context: None,
		};
		store.insert_reference(&bip_ref).await.unwrap();

		// Add a concept mention on the LUD doc (use a seeded concept)
		store.upsert_concept_mention(&lud_doc.id, "taproot", 1.0).await.unwrap();

		// Add sync state entries
		let lud_state = SyncState {
			source_id: "specs:luds".to_string(),
			source_type: "specs".to_string(),
			source_repo: None,
			last_cursor: None,
			last_synced_at: Some(Utc::now()),
			next_run_at: None,
			status: SyncStatus::Ok,
			error_message: None,
			retry_count: 0,
			items_found: 21,
		};
		store.update_sync_state(&lud_state).await.unwrap();

		let bip_state = SyncState {
			source_id: "specs:bips".to_string(),
			source_type: "specs".to_string(),
			source_repo: None,
			last_cursor: None,
			last_synced_at: Some(Utc::now()),
			next_run_at: None,
			status: SyncStatus::Ok,
			error_message: None,
			retry_count: 0,
			items_found: 300,
		};
		store.update_sync_state(&bip_state).await.unwrap();

		// Verify all 3 docs exist
		assert!(store.get_document(&lud_doc.id).await.unwrap().is_some());
		assert!(store.get_document(&bip_doc.id).await.unwrap().is_some());
		assert!(store.get_document(&issue_doc.id).await.unwrap().is_some());

		// Reset LUD source type
		let deleted = store.reset_source_type("lud", &["specs:luds".into()]).await.unwrap();
		assert_eq!(deleted, 1, "should delete exactly 1 LUD document");

		// LUD doc should be gone
		assert!(store.get_document(&lud_doc.id).await.unwrap().is_none());

		// BIP and issue docs should still exist
		assert!(store.get_document(&bip_doc.id).await.unwrap().is_some());
		let issue_ctx = store.get_document(&issue_doc.id).await.unwrap().unwrap();
		assert_eq!(issue_ctx.document.title.as_deref(), Some("Test issue"));

		// The issue's outgoing refs should still be intact (they originate
		// from the issue, not from the deleted LUD)
		assert_eq!(issue_ctx.outgoing_refs.len(), 2);

		// LUD sync state should be gone, BIP sync state should remain
		assert!(store.get_sync_state("specs:luds").await.unwrap().is_none());
		assert!(store.get_sync_state("specs:bips").await.unwrap().is_some());
	}

	#[tokio::test]
	async fn test_reset_source_type_with_like_pattern() {
		let store = SqliteStore::open_in_memory().unwrap();

		// Insert two GitHub issues from different repos
		let doc1 = Document {
			id: "github_issue:bitcoin/bitcoin:1".to_string(),
			source_type: SourceType::GithubIssue,
			source_repo: Some("bitcoin/bitcoin".to_string()),
			source_id: "1".to_string(),
			title: Some("Bitcoin issue".to_string()),
			body: Some("body".to_string()),
			author: None,
			author_id: None,
			created_at: Utc::now(),
			updated_at: None,
			parent_id: None,
			metadata: None,
			seq: None,
		};
		let doc2 = Document {
			id: "github_issue:lightningdevkit/ldk-node:1".to_string(),
			source_type: SourceType::GithubIssue,
			source_repo: Some("lightningdevkit/ldk-node".to_string()),
			source_id: "1".to_string(),
			title: Some("LDK issue".to_string()),
			body: Some("body".to_string()),
			author: None,
			author_id: None,
			created_at: Utc::now(),
			updated_at: None,
			parent_id: None,
			metadata: None,
			seq: None,
		};
		let bolt_doc = Document {
			id: Document::make_id(&SourceType::Bolt, None, "2"),
			source_type: SourceType::Bolt,
			source_repo: None,
			source_id: "2".to_string(),
			title: Some("BOLT-2".to_string()),
			body: Some("Peer protocol".to_string()),
			author: None,
			author_id: None,
			created_at: Utc::now(),
			updated_at: None,
			parent_id: None,
			metadata: None,
			seq: None,
		};
		store.upsert_document(&doc1).await.unwrap();
		store.upsert_document(&doc2).await.unwrap();
		store.upsert_document(&bolt_doc).await.unwrap();

		// Add sync states
		for sid in &["github:bitcoin/bitcoin:issues", "github:lightningdevkit/ldk-node:issues"] {
			let state = SyncState {
				source_id: sid.to_string(),
				source_type: "github".to_string(),
				source_repo: None,
				last_cursor: None,
				last_synced_at: Some(Utc::now()),
				next_run_at: None,
				status: SyncStatus::Ok,
				error_message: None,
				retry_count: 0,
				items_found: 10,
			};
			store.update_sync_state(&state).await.unwrap();
		}

		// Reset github_issue — should delete both issues but not the BOLT
		let deleted =
			store.reset_source_type("github_issue", &["github:%:issues".into()]).await.unwrap();
		assert_eq!(deleted, 2, "should delete both GitHub issues");

		// Both issues gone
		assert!(store.get_document(&doc1.id).await.unwrap().is_none());
		assert!(store.get_document(&doc2.id).await.unwrap().is_none());

		// BOLT still exists
		assert!(store.get_document(&bolt_doc.id).await.unwrap().is_some());

		// Both issue sync states cleared
		assert!(store.get_sync_state("github:bitcoin/bitcoin:issues").await.unwrap().is_none());
		assert!(store
			.get_sync_state("github:lightningdevkit/ldk-node:issues")
			.await
			.unwrap()
			.is_none());
	}

	#[tokio::test]
	async fn test_lookup_nut_padded_source_id() {
		let store = SqliteStore::open_in_memory().unwrap();

		// Simulate old data with zero-padded source_id "00"
		let doc = Document {
			id: Document::make_id(&SourceType::Nut, None, "00"),
			source_type: SourceType::Nut,
			source_repo: None,
			source_id: "00".to_string(),
			title: Some("NUT-00: Notation, Usage, and Terminology".to_string()),
			body: Some("# NUT-00\n\nSpec content here.".to_string()),
			author: None,
			author_id: None,
			created_at: Utc::now(),
			updated_at: None,
			parent_id: None,
			metadata: None,
			seq: None,
		};
		store.upsert_document(&doc).await.unwrap();

		// lookup_nut(0) should still find it even though source_id is "00"
		let ctx = store.lookup_nut(0).await.unwrap();
		assert!(ctx.is_some(), "lookup_nut(0) should find padded source_id '00'");
		assert_eq!(
			ctx.unwrap().document.title.as_deref(),
			Some("NUT-00: Notation, Usage, and Terminology")
		);
	}
}
