//! An additive, atomic upgrade of the unversioned legacy database.
use anyhow::{ensure, Context, Result};
use bkb_core::model::SourceType;
use bkb_core::schema::SCHEMA_SQL;
use rusqlite::{types::Value, Connection, OptionalExtension, TransactionBehavior};

const VERSION: i64 = 1;
const LEGACY_TABLES: &[&str] =
	&["documents", "refs", "concepts", "concept_mentions", "change_log", "sync_state"];
const ADDED_TABLES: &[&str] = &["document_links", "document_aliases"];

pub(crate) fn initialize(
	conn: &mut Connection, seed: impl FnOnce(&Connection) -> Result<()>,
) -> Result<()> {
	// REPLACE performs an implicit DELETE; SQLite fires its delete guards only
	// with recursive triggers enabled. Restore the caller's connection setting.
	let recursive: bool = conn.query_row("PRAGMA recursive_triggers", [], |row| row.get(0))?;
	conn.pragma_update(None, "recursive_triggers", true)?;
	let result = migrate(conn, seed);
	conn.pragma_update(None, "recursive_triggers", recursive)?;
	result
}

fn migrate(conn: &mut Connection, seed: impl FnOnce(&Connection) -> Result<()>) -> Result<()> {
	let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
	let version: i64 = tx.query_row("PRAGMA user_version", [], |row| row.get(0))?;
	ensure!(
		(0..=VERSION).contains(&version),
		"unsupported database schema version {version}; refusing upgrade"
	);
	let populated: bool = tx.query_row(
		"SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%')",
		[],
		|row| row.get(0),
	)?;
	if populated || version != 0 {
		validate_schema(&tx, version)?;
	}
	let upgrading = populated && version == 0;
	let mut protected = Vec::new();
	if upgrading {
		tracing::info!("starting stored-data upgrade; validating existing data before commit");
		validate_data(&tx).context("legacy data is not readable; refusing upgrade")?;
		tracing::info!("stored-data validation complete; protecting existing rows");
	}
	if populated {
		for table in LEGACY_TABLES.iter().chain(ADDED_TABLES) {
			if table_exists(&tx, table)? {
				let count = if upgrading {
					tx.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| {
						r.get::<_, i64>(0)
					})?
				} else {
					0
				};
				protected.push((*table, count));
				for action in ["UPDATE", "DELETE"] {
					tx.execute_batch(&format!(
						"CREATE TEMP TRIGGER upgrade_guard_{table}_{action} BEFORE {action} ON main.{table}
						 BEGIN SELECT RAISE(ABORT, 'upgrade must preserve existing rows'); END;"
					))?;
				}
			}
		}
	}
	// These statements add tables/indexes; they never rewrite source records.
	if upgrading {
		tracing::info!("adding database tables and indexes");
	}
	tx.execute_batch(SCHEMA_SQL).context("database schema upgrade failed")?;
	seed(&tx).context("database concept seeding failed")?;
	if upgrading {
		tracing::info!("validating upgraded schema and full-text index");
		validate_schema(&tx, VERSION)?;
		validate_fts(&tx)?;
	}
	for (table, before) in protected {
		if upgrading {
			let after: i64 =
				tx.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))?;
			ensure!(after >= before, "upgrade would delete rows from {table}; refusing upgrade");
		}
		for action in ["UPDATE", "DELETE"] {
			tx.execute_batch(&format!("DROP TRIGGER temp.upgrade_guard_{table}_{action}"))?;
		}
	}
	tx.pragma_update(None, "user_version", VERSION)?;
	tx.commit()?;
	if upgrading {
		tracing::info!(version = VERSION, "stored-data upgrade committed");
	}
	Ok(())
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
	Ok(conn.query_row(
		"SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name=?1 AND type='table')",
		[table],
		|row| row.get(0),
	)?)
}

fn rows(conn: &Connection, sql: &str, name: &str) -> Result<Vec<Vec<Value>>> {
	let mut stmt = conn.prepare(sql)?;
	let columns = stmt.column_count();
	let values = stmt
		.query_map([name], |row| (0..columns).map(|i| row.get(i)).collect())?
		.collect::<rusqlite::Result<_>>()?;
	Ok(values)
}

fn validate_schema(conn: &Connection, version: i64) -> Result<()> {
	let expected = Connection::open_in_memory()?;
	expected.execute_batch(SCHEMA_SQL)?;
	for table in LEGACY_TABLES.iter().chain(ADDED_TABLES) {
		if !table_exists(conn, table)? {
			ensure!(
				version == 0 && ADDED_TABLES.contains(table),
				"incompatible database schema: missing table {table}"
			);
			continue;
		}
		for sql in [
			"SELECT * FROM pragma_table_xinfo(?1)",
			"SELECT * FROM pragma_foreign_key_list(?1)",
			"SELECT wr, strict FROM pragma_table_list WHERE schema='main' AND name=?1",
			// Names of autoindexes are irrelevant; their ordered column sets matter.
			"SELECT il.origin, ii.seqno, ii.name FROM pragma_index_list(?1) il, pragma_index_info(il.name) ii WHERE il.origin IN ('pk','u') ORDER BY il.origin, il.name, ii.seqno",
		] {
			ensure!(rows(conn, sql, table)? == rows(&expected, sql, table)?, "incompatible database schema for {table}; refusing upgrade");
		}
	}
	// FTS configuration and trigger bodies have semantics beyond column layout.
	for name in
		["documents_fts", "documents_fts_insert", "documents_fts_delete", "documents_fts_update"]
	{
		let actual: Option<String> = conn
			.query_row("SELECT sql FROM sqlite_schema WHERE name=?1", [name], |r| r.get(0))
			.optional()?;
		let expected: String =
			expected
				.query_row("SELECT sql FROM sqlite_schema WHERE name=?1", [name], |r| r.get(0))?;
		ensure!(
			actual.as_deref().map(normalize_sql).as_deref()
				== Some(normalize_sql(&expected).as_str()),
			"incompatible database schema for {name}; refusing upgrade"
		);
	}
	Ok(())
}

/// Ignore formatting/case outside SQL string literals, retaining their contents.
fn normalize_sql(sql: &str) -> String {
	let mut literal = false;
	sql.chars()
		.filter_map(|c| {
			if c == '\'' {
				literal = !literal;
			}
			if literal || c == '\'' {
				Some(c)
			} else if c.is_whitespace() {
				None
			} else {
				Some(c.to_ascii_lowercase())
			}
		})
		.collect()
}

fn validate_fts(conn: &Connection) -> Result<()> {
	// rank=1 also checks the external content table, catching missing/stale hits.
	conn.execute_batch(
		"INSERT INTO documents_fts(documents_fts, rank) VALUES ('integrity-check', 1)",
	)
	.context("full-text index is inconsistent with source documents; refusing upgrade")?;
	Ok(())
}

fn validate_data(conn: &Connection) -> Result<()> {
	tracing::info!("checking SQLite integrity");
	let mut check = conn.prepare("PRAGMA quick_check")?;
	for result in check.query_map([], |r| r.get::<_, String>(0))? {
		let result = result?;
		ensure!(result == "ok", "database integrity check failed: {result}");
	}
	tracing::info!("checking foreign keys");
	let violation: Option<String> =
		conn.query_row("PRAGMA foreign_key_check", [], |r| r.get(0)).optional()?;
	ensure!(violation.is_none(), "database has broken foreign keys in {violation:?}");
	tracing::info!("validating stored documents");
	let mut stmt = conn.prepare("SELECT id,source_type,source_repo,source_id,title,body,author,author_id,created_at,updated_at,parent_id,metadata,seq FROM documents")?;
	let mut records = stmt.query([])?;
	let mut checked = 0_u64;
	while let Some(row) = records.next()? {
		let id: String = row.get(0)?;
		let kind: String = row.get(1)?;
		ensure!(
			SourceType::from_str(&kind).is_some(),
			"unknown source type {kind:?} in document {id}"
		);
		let _: String = row.get(3)?;
		for col in [2, 4, 5, 6, 7, 10] {
			let _: Option<String> = row.get(col)?;
		}
		let created: String = row.get(8)?;
		chrono::DateTime::parse_from_rfc3339(&created)
			.with_context(|| format!("invalid creation timestamp in document {id}"))?;
		if let Some(updated) = row.get::<_, Option<String>>(9)? {
			chrono::DateTime::parse_from_rfc3339(&updated)
				.with_context(|| format!("invalid update timestamp in document {id}"))?;
		}
		if let Some(metadata) = row.get::<_, Option<String>>(11)? {
			serde_json::from_str::<serde_json::Value>(&metadata)
				.with_context(|| format!("invalid metadata JSON in document {id}"))?;
		}
		let _: Option<i64> = row.get(12)?;
		checked += 1;
		if checked % 100_000 == 0 {
			tracing::info!(documents = checked, "stored-document validation progress");
		}
	}
	tracing::info!(documents = checked, "stored-document validation complete");
	// Read the remaining tables using their public reader's SQLite types. Opaque
	// checkpoints and optional legacy values are preserved without reinterpretation.
	for (table, text, required, integers, floats) in [
		("refs", &[1, 2, 3, 4, 5][..], &[1, 3][..], &[0][..], &[][..]),
		("concepts", &[0, 1, 2, 3][..], &[0, 1][..], &[][..], &[][..]),
		("concept_mentions", &[0, 1][..], &[0, 1][..], &[][..], &[2][..]),
		("change_log", &[1, 2, 3][..], &[1, 2][..], &[0][..], &[][..]),
		("sync_state", &[0, 1, 2, 3, 4, 5, 6, 7][..], &[0, 1, 6][..], &[8, 9][..], &[][..]),
		("document_links", &[0, 1, 2, 3][..], &[0, 1, 2, 3][..], &[][..], &[][..]),
		("document_aliases", &[0, 1, 2, 3][..], &[0, 1, 2, 3][..], &[][..], &[][..]),
	] {
		if !table_exists(conn, table)? {
			continue;
		}
		tracing::info!(table, "validating stored table");
		let mut stmt = conn.prepare(&format!("SELECT * FROM {table}"))?;
		let mut records = stmt.query([])?;
		let mut checked = 0_u64;
		while let Some(row) = records.next()? {
			for col in text {
				let value: Option<String> = row.get(*col)?;
				ensure!(!required.contains(col) || value.is_some(), "unreadable NULL in {table}");
			}
			for col in integers {
				let value: i64 = row.get(*col)?;
				if table == "sync_state" {
					i32::try_from(value).context("unreadable sync counter")?;
				}
			}
			for col in floats {
				let _: Option<f64> = row.get(*col)?;
			}
			checked += 1;
			if checked % 100_000 == 0 {
				tracing::info!(table, rows = checked, "stored-table validation progress");
			}
		}
		tracing::info!(table, rows = checked, "stored-table validation complete");
	}
	tracing::info!("checking full-text index against stored documents");
	validate_fts(conn)?;
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn replacement_cannot_overwrite_existing_data() {
		let mut conn = Connection::open_in_memory().unwrap();
		conn.pragma_update(None, "recursive_triggers", false).unwrap();
		conn.execute_batch(SCHEMA_SQL).unwrap();
		conn.execute("INSERT INTO concepts VALUES ('taproot', 'Original label', NULL, NULL)", [])
			.unwrap();
		let result = initialize(&mut conn, |conn| {
			conn.execute(
				"INSERT OR REPLACE INTO concepts VALUES ('taproot', 'lost', NULL, NULL)",
				[],
			)?;
			Ok(())
		});
		assert!(
			result.is_err(),
			"replacement must be refused before an existing row is overwritten"
		);
		assert!(format!("{:#}", result.unwrap_err()).contains("preserve existing rows"));
		assert_eq!(
			conn.query_row("SELECT name FROM concepts WHERE slug='taproot'", [], |r| r
				.get::<_, String>(0))
				.unwrap(),
			"Original label"
		);
		assert!(!conn.query_row("PRAGMA recursive_triggers", [], |r| r.get::<_, bool>(0)).unwrap());
	}

	#[test]
	fn a_late_seed_failure_rolls_back_schema_and_version() {
		let mut conn = Connection::open_in_memory().unwrap();
		conn.pragma_update(None, "recursive_triggers", false).unwrap();
		conn.execute_batch(SCHEMA_SQL).unwrap();
		conn.execute_batch("DROP TABLE document_aliases; DROP TABLE document_links; DROP INDEX idx_documents_import").unwrap();
		let result = initialize(&mut conn, |conn| {
			conn.execute("INSERT INTO concepts VALUES ('temporary', 'temporary', NULL, NULL)", [])?;
			anyhow::bail!("simulated seeding failure")
		});
		assert!(format!("{:#}", result.unwrap_err()).contains("simulated seeding failure"));
		assert!(!table_exists(&conn, "document_links").unwrap());
		assert!(!table_exists(&conn, "document_aliases").unwrap());
		assert_eq!(
			conn.query_row("SELECT COUNT(*) FROM concepts", [], |r| r.get::<_, i64>(0)).unwrap(),
			0
		);
		assert_eq!(conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
		assert!(!conn.query_row("PRAGMA recursive_triggers", [], |r| r.get::<_, bool>(0)).unwrap());
		initialize(&mut conn, |_| Ok(())).unwrap();
		assert!(!conn.query_row("PRAGMA recursive_triggers", [], |r| r.get::<_, bool>(0)).unwrap());
	}
}
