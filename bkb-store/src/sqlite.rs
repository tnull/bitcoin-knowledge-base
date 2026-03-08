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
		Ok(Self { conn: Arc::new(Mutex::new(conn)) })
	}

	/// Insert or update a document, appending to the change log.
	pub async fn upsert_document(&self, doc: &Document) -> Result<()> {
		let conn = self.conn.lock().await;
		let metadata_json = doc.metadata.as_ref().map(|m| serde_json::to_string(m)).transpose()?;

		conn.execute_batch("BEGIN IMMEDIATE")?;

		// Check if document exists for change_log type
		let exists: bool = conn
			.query_row("SELECT COUNT(*) FROM documents WHERE id = ?1", [&doc.id], |row| {
				row.get::<_, i64>(0)
			})
			.map(|c| c > 0)?;

		let change_type = if exists { "update" } else { "insert" };

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

		conn.execute_batch("COMMIT")?;

		debug!(doc_id = %doc.id, change_type, seq, "upserted document");
		Ok(())
	}

	/// Insert a cross-reference.
	pub async fn insert_reference(&self, reference: &Reference) -> Result<()> {
		let conn = self.conn.lock().await;
		conn.execute(
			"INSERT INTO refs (from_doc_id, to_doc_id, ref_type, to_external, context)
			 VALUES (?1, ?2, ?3, ?4, ?5)",
			rusqlite::params![
				&reference.from_doc_id,
				&reference.to_doc_id,
				reference.ref_type.as_str(),
				&reference.to_external,
				&reference.context,
			],
		)?;
		Ok(())
	}

	/// Delete all references originating from a document (for re-enrichment).
	pub async fn delete_refs_from(&self, doc_id: &str) -> Result<()> {
		let conn = self.conn.lock().await;
		conn.execute("DELETE FROM refs WHERE from_doc_id = ?1", [doc_id])?;
		Ok(())
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

		// Find the spec document.
		let doc = conn
			.query_row(
				"SELECT id, source_type, source_repo, source_id, title, body,
				 author, author_id, created_at, updated_at, parent_id, metadata, seq
				 FROM documents WHERE source_type = ?1 AND source_id = ?2",
				rusqlite::params![source_type.as_str(), &source_id],
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

		Ok(Some(DocumentContext { document: doc, url, outgoing_refs, incoming_refs, concepts }))
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
		let limit = params.limit.unwrap_or(20).min(100);

		// Build the FTS5 query
		let fts_query = build_fts_query(&params.query);

		let mut sql = String::from(
			"SELECT d.id, d.source_type, d.source_repo, d.title,
				snippet(documents_fts, 1, '<mark>', '</mark>', '...', 64) as snippet,
				d.author, d.created_at,
				bm25(documents_fts, 5.0, 1.0) as score,
				d.source_id
			 FROM documents_fts
			 JOIN documents d ON d.rowid = documents_fts.rowid
			 WHERE documents_fts MATCH ?1",
		);

		let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> =
			vec![Box::new(fts_query.clone())];
		let mut param_idx = 2;

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
			if !repos.is_empty() {
				let placeholders: Vec<String> =
					repos.iter().enumerate().map(|(i, _)| format!("?{}", param_idx + i)).collect();
				sql.push_str(&format!(" AND d.source_repo IN ({})", placeholders.join(",")));
				for repo in repos {
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

		sql.push_str(" ORDER BY score LIMIT ?");
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
					parent_id: None,
					metadata: None,
					seq: None,
				};
				Ok(SearchResult {
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

		let total_count = results.len() as u32;
		Ok(SearchResults { results, total_count })
	}

	async fn get_document(&self, id: &str) -> Result<Option<DocumentContext>> {
		let conn = self.conn.lock().await;

		// Fetch document
		let doc = conn
			.query_row(
				"SELECT id, source_type, source_repo, source_id, title, body,
				 author, author_id, created_at, updated_at, parent_id, metadata, seq
				 FROM documents WHERE id = ?1",
				[id],
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

		// Fetch outgoing refs
		let mut stmt = conn.prepare(
			"SELECT id, from_doc_id, to_doc_id, ref_type, to_external, context
			 FROM refs WHERE from_doc_id = ?1",
		)?;
		let outgoing_refs = stmt
			.query_map([id], |row| {
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

		// Fetch incoming refs
		let mut stmt = conn.prepare(
			"SELECT id, from_doc_id, to_doc_id, ref_type, to_external, context
			 FROM refs WHERE to_doc_id = ?1",
		)?;
		let incoming_refs = stmt
			.query_map([id], |row| {
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

		// Fetch concept tags
		let mut stmt =
			conn.prepare("SELECT concept_slug FROM concept_mentions WHERE doc_id = ?1")?;
		let concepts: Vec<String> =
			stmt.query_map([id], |row| row.get(0))?.collect::<rusqlite::Result<Vec<_>>>()?;

		Ok(Some(DocumentContext { document: doc, url, outgoing_refs, incoming_refs, concepts }))
	}

	async fn get_references(
		&self, entity: &str, ref_type: Option<&str>, limit: u32,
	) -> Result<Vec<Reference>> {
		let conn = self.conn.lock().await;

		let (sql, params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match ref_type {
			Some(rt) => (
				"SELECT r.id, r.from_doc_id, r.to_doc_id, r.ref_type, r.to_external, r.context \
				 FROM refs r \
				 WHERE r.to_external = ?1 AND r.ref_type = ?2 \
				 ORDER BY r.id \
				 LIMIT ?3"
					.to_string(),
				vec![
					Box::new(entity.to_string()),
					Box::new(rt.to_string()),
					Box::new(limit as i64),
				],
			),
			None => (
				"SELECT r.id, r.from_doc_id, r.to_doc_id, r.ref_type, r.to_external, r.context \
				 FROM refs r \
				 WHERE r.to_external = ?1 \
				 ORDER BY r.id \
				 LIMIT ?2"
					.to_string(),
				vec![Box::new(entity.to_string()), Box::new(limit as i64)],
			),
		};

		let param_refs: Vec<&dyn rusqlite::types::ToSql> =
			params.iter().map(|b| b.as_ref()).collect();

		let mut stmt = conn.prepare(&sql)?;
		let refs = stmt
			.query_map(param_refs.as_slice(), |row| {
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

		Ok(refs)
	}

	async fn lookup_bip(&self, number: u32) -> Result<Option<DocumentContext>> {
		self.lookup_spec(SourceType::Bip, number).await
	}

	async fn lookup_bolt(&self, number: u32) -> Result<Option<DocumentContext>> {
		self.lookup_spec(SourceType::Bolt, number).await
	}
}

/// Build an FTS5 query string from user input.
///
/// Escapes special characters and wraps terms for prefix matching.
fn build_fts_query(input: &str) -> String {
	// For now, pass through as-is with basic quoting.
	// FTS5 handles most inputs gracefully.
	let trimmed = input.trim();
	if trimmed.is_empty() {
		return "\"\"".to_string();
	}

	// If the query contains FTS5 operators, pass through as-is
	if trimmed.contains('"')
		|| trimmed.contains("AND")
		|| trimmed.contains("OR")
		|| trimmed.contains("NOT")
		|| trimmed.contains('*')
	{
		return trimmed.to_string();
	}

	// Otherwise, quote the entire query for phrase matching or simple term matching
	// Split into words and join with spaces (implicit AND in FTS5)
	let terms: Vec<&str> = trimmed.split_whitespace().collect();
	if terms.len() == 1 {
		// Single word: use prefix matching
		format!("{}*", terms[0])
	} else {
		// Multiple words: use implicit AND (just space-separated terms)
		terms.join(" ")
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
}
