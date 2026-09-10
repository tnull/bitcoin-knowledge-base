//! Reversible cross-forge equivalences; raw documents are never overwritten.
use anyhow::Result;
use bkb_core::model::{Document, DocumentContext, DocumentOrigin, RefType, Reference, SourceType};
use bkb_core::repository::{forgejo_identity, is_migrated, logical_repo};
use rusqlite::{params, Connection, OptionalExtension};

pub(crate) fn raw(conn: &Connection, id: &str) -> Result<Option<Document>> {
	Ok(conn.query_row("SELECT id,source_type,source_repo,source_id,title,body,author,author_id,created_at,updated_at,parent_id,metadata,seq FROM documents WHERE id=?1", [id], |row| {
		let created: String = row.get(8)?;
		let updated: Option<String> = row.get(9)?;
		let metadata: Option<String> = row.get(11)?;
		Ok(Document { id: row.get(0)?, source_type: SourceType::from_str(&row.get::<_,String>(1)?).unwrap(),
			source_repo: row.get(2)?, source_id: row.get(3)?, title: row.get(4)?, body: row.get(5)?, author: row.get(6)?, author_id: row.get(7)?,
			created_at: created.parse().map_err(|e| rusqlite::Error::FromSqlConversionFailure(8, rusqlite::types::Type::Text, Box::new(e)))?,
			updated_at: updated.and_then(|s| s.parse().ok()), parent_id: row.get(10)?,
			metadata: metadata.and_then(|s| serde_json::from_str(&s).ok()), seq: row.get(12)? })
	}).optional()?)
}

fn ids(conn: &Connection, sql: &str, values: impl rusqlite::Params) -> Result<Vec<String>> {
	Ok(conn.prepare(sql)?.query_map(values, |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
}

/// Resolve issue-API parent IDs only within their original host's namespace.
pub(crate) fn actual_id(conn: &Connection, id: &str) -> Result<String> {
	if raw(conn, id)?.is_some() {
		return Ok(id.into());
	}
	let alternate =
		id.replacen("github_issue:", "github_pr:", 1).replacen("forgejo_issue:", "forgejo_pr:", 1);
	Ok(if alternate != id && raw(conn, &alternate)?.is_some() { alternate } else { id.into() })
}

/// A Forgejo issuecomment anchor alone does not identify a document kind or
/// review ID. Only a unique stored permalink within the same forge is evidence.
fn permalink_repo(external: &str) -> Option<&str> {
	let path = external.strip_prefix("https://")?;
	let (path, anchor) = path.split_once("#issuecomment-")?;
	if anchor.is_empty() || !anchor.bytes().all(|b| b.is_ascii_digit()) {
		return None;
	}
	let parts: Vec<_> = path.split('/').collect();
	if parts.len() != 5
		|| parts[0] != bkb_core::repository::FORGEJO_HOST
		|| parts[1].is_empty()
		|| parts[2].is_empty()
		|| !matches!(parts[3], "issues" | "pull" | "pulls")
		|| parts[4].is_empty()
		|| !parts[4].bytes().all(|b| b.is_ascii_digit())
	{
		return None;
	}
	Some(&path[..path.len() - parts[3].len() - parts[4].len() - 2])
}

fn permalink_id(conn: &Connection, external: &str, repo: &str) -> Result<Option<String>> {
	let candidates = ids(
		conn,
		"SELECT id FROM documents WHERE json_extract(metadata, '$.url')=?1 AND source_repo=?2
		 AND source_type IN ('forgejo_comment','forgejo_review','forgejo_review_comment') LIMIT 2",
		params![external, repo],
	)?;
	Ok(if candidates.len() == 1 { candidates.into_iter().next() } else { None })
}

pub(crate) fn reference_target(conn: &Connection, reference: &Reference) -> Result<Option<String>> {
	if let Some(external) = &reference.to_external {
		if let Some(repo) = permalink_repo(external) {
			return permalink_id(conn, external, repo);
		}
	}
	match &reference.to_doc_id {
		Some(id) => Ok(Some(actual_id(conn, id)?)),
		None => reference
			.to_external
			.as_deref()
			.map(|e| external_id(conn, e))
			.transpose()
			.map(Option::flatten),
	}
}

/// Repair legacy guesses and pending links without replacing reference rows.
/// Looking up both ends also invalidates links if a target's URL changes.
pub(crate) fn repair_permalink_refs(conn: &Connection, doc: &Document) -> Result<()> {
	let mut urls: std::collections::HashSet<String> = ids(conn,
		"SELECT DISTINCT to_external FROM refs WHERE (from_doc_id=?1 OR to_doc_id=?1) AND to_external IS NOT NULL",
		[&doc.id])?.into_iter().collect();
	if let Some(url) = doc.url() {
		urls.insert(url);
	}
	for url in urls {
		repair_permalink_url(conn, &url)?;
	}
	Ok(())
}

pub(crate) fn repair_permalink_url(conn: &Connection, url: &str) -> Result<()> {
	if let Some(repo) = permalink_repo(url) {
		conn.execute(
			"UPDATE refs SET to_doc_id=?1 WHERE to_external=?2 AND to_doc_id IS NOT ?1",
			params![permalink_id(conn, url, repo)?, url],
		)?;
	}
	Ok(())
}

pub(crate) fn canonical_id(conn: &Connection, id: &str) -> Result<String> {
	let id = actual_id(conn, id)?;
	Ok(conn
		.query_row("SELECT canonical_id FROM document_aliases WHERE doc_id=?1", [&id], |r| r.get(0))
		.optional()?
		.unwrap_or(id))
}

fn thread(doc: &Document) -> bool {
	matches!(
		doc.source_type,
		SourceType::GithubIssue
			| SourceType::GithubPr
			| SourceType::ForgejoIssue
			| SourceType::ForgejoPr
	)
}

fn record_link(
	conn: &Connection, doc: &str, target: &str, origin: &str, evidence: &str,
) -> Result<()> {
	if doc == target {
		return Ok(());
	}
	conn.execute(
		"INSERT OR IGNORE INTO document_links VALUES (?1,?2,?3,?4)",
		params![doc, target, origin, evidence],
	)?;
	bind(conn, doc, target, origin, evidence)
}

fn bind(conn: &Connection, a: &str, b: &str, origin: &str, evidence: &str) -> Result<()> {
	let (Some(a_doc), Some(b_doc)) = (raw(conn, a)?, raw(conn, b)?) else {
		return Ok(());
	};
	if a_doc.source_type.as_str().split_once('_').map(|v| v.1)
		!= b_doc.source_type.as_str().split_once('_').map(|v| v.1)
	{
		return Ok(());
	}
	let a_group = canonical_id(conn, a)?;
	let b_group = canonical_id(conn, b)?;
	if a_group == b_group {
		return Ok(());
	}
	// A thread can have at most one source record from each host. Conflicting
	// evidence remains pending rather than joining unrelated conversations.
	let members = ids(conn, "SELECT id FROM documents WHERE id IN (?1,?2) OR id IN (SELECT doc_id FROM document_aliases WHERE canonical_id IN (?3,?4))", params![a,b,a_group,b_group])?;
	let mut hosts = std::collections::HashSet::new();
	for member in &members {
		let doc = raw(conn, member)?.unwrap();
		if thread(&doc) && !hosts.insert(doc.source_repo) {
			return Ok(());
		}
	}
	let canonical = if thread(&a_doc) {
		if a_doc.source_type.as_str().starts_with("forgejo_") {
			a
		} else {
			b
		}
	} else {
		origin
	};
	if !members.iter().any(|id| id == canonical) || raw(conn, origin)?.is_none() {
		return Ok(());
	}
	for member in members {
		conn.execute("INSERT INTO document_aliases VALUES (?1,?2,?3,?4) ON CONFLICT(doc_id) DO UPDATE SET canonical_id=excluded.canonical_id, origin_id=excluded.origin_id, evidence=excluded.evidence",
			params![member, canonical, origin, evidence])?;
	}
	Ok(())
}

fn mirror_links(conn: &Connection, doc: &Document) -> Result<()> {
	let Some(origin) = bkb_core::mirror::parse(doc) else {
		return Ok(());
	};
	let target = actual_id(conn, &origin.id)?;
	record_link(conn, &doc.id, &target, &target, "issue-mirror:v1")?;
	if let (Some(parent), Some(target)) = (&doc.parent_id, &origin.parent_id) {
		let parent = actual_id(conn, parent)?;
		let target = actual_id(conn, target)?;
		if raw(conn, &parent)?.is_some() {
			// A verified comment URL also proves its containing thread's mapping.
			let primary = if parent.starts_with("forgejo_") { &parent } else { &target };
			record_link(conn, &parent, &target, primary, "issue-mirror:v1 parent")?;
		}
	}
	Ok(())
}

fn counterpart(doc: &Document) -> Option<(String, String)> {
	let repo = logical_repo(doc.source_repo.as_deref()?);
	if !is_migrated(repo) {
		return None;
	}
	let kind = doc.source_type.as_str();
	if let Some(suffix) = kind.strip_prefix("github_") {
		Some((format!("forgejo_{suffix}"), forgejo_identity(repo)))
	} else {
		Some((format!("github_{}", kind.strip_prefix("forgejo_")?), repo.into()))
	}
}

fn fingerprint(a: &Document, b: &Document) -> bool {
	let clean = |body: &Option<String>| body.as_deref().unwrap_or("").replace("\r\n", "\n");
	let body = clean(&a.body);
	if body.trim().is_empty() || body != clean(&b.body) || a.created_at != b.created_at {
		return false;
	}
	if thread(a) {
		return a.source_id == b.source_id && a.title == b.title && a.title.is_some();
	}
	for key in ["path", "diff_hunk", "commit_id"] {
		let av = a.metadata.as_ref().and_then(|m| m.get(key)).filter(|v| !v.is_null());
		let bv = b.metadata.as_ref().and_then(|m| m.get(key)).filter(|v| !v.is_null());
		if av.is_some() && bv.is_some() && av != bv {
			return false;
		}
	}
	true
}

fn exact_candidates(conn: &Connection, doc: &Document) -> Result<Vec<Document>> {
	let Some((kind, repo)) = counterpart(doc) else {
		return Ok(vec![]);
	};
	if !matches!(
		kind.as_str(),
		"github_issue"
			| "github_pr"
			| "github_comment"
			| "github_review"
			| "github_review_comment"
			| "forgejo_issue"
			| "forgejo_pr"
			| "forgejo_comment"
			| "forgejo_review"
			| "forgejo_review_comment"
	) {
		return Ok(vec![]);
	}
	let parent = doc.parent_id.as_deref().map(|id| canonical_id(conn, id)).transpose()?;
	let mut found = vec![];
	for id in ids(
		conn,
		"SELECT id FROM documents WHERE source_type=?1 AND source_repo=?2 AND created_at=?3",
		params![kind, repo, doc.created_at.to_rfc3339()],
	)? {
		let other = raw(conn, &id)?.unwrap();
		if !fingerprint(doc, &other) {
			continue;
		}
		if !thread(doc) {
			let other_parent =
				other.parent_id.as_deref().map(|id| canonical_id(conn, id)).transpose()?;
			if parent.is_none() || parent != other_parent {
				continue;
			}
		}
		found.push(other);
	}
	Ok(found)
}

fn observe(conn: &Connection, doc: &Document) -> Result<()> {
	mirror_links(conn, doc)?;
	let candidates = exact_candidates(conn, doc)?;
	if let [other] = candidates.as_slice() {
		if exact_candidates(conn, other)?.len() == 1 {
			let origin = if thread(doc) {
				if doc.source_type.as_str().starts_with("forgejo_") {
					&doc.id
				} else {
					&other.id
				}
			} else if doc.source_type.as_str().starts_with("github_") {
				&doc.id
			} else {
				&other.id
			};
			record_link(conn, &doc.id, &other.id, origin, "exact-import:v1")?;
		} else {
			invalidate_exact(conn, &other.id)?;
		}
	} else if candidates.len() > 1 {
		invalidate_exact(conn, &doc.id)?;
	}
	Ok(())
}

fn invalidate_exact(conn: &Connection, id: &str) -> Result<()> {
	let group = canonical_id(conn, id)?;
	conn.execute("DELETE FROM document_links WHERE evidence='exact-import:v1' AND (doc_id IN (SELECT doc_id FROM document_aliases WHERE canonical_id=?1) OR target_id IN (SELECT doc_id FROM document_aliases WHERE canonical_id=?1))",[&group])?;
	conn.execute(
		"DELETE FROM document_aliases WHERE canonical_id=?1 AND evidence='exact-import:v1'",
		[&group],
	)?;
	Ok(())
}

/// Called inside the page transaction. Indexed candidates avoid rescanning raw
/// history on each page; late parents and late origins are retried automatically.
pub(crate) fn reconcile(conn: &Connection, documents: &[Document]) -> Result<()> {
	for doc in documents {
		repair_permalink_refs(conn, doc)?;
		observe(conn, doc)?;
	}
	for doc in documents {
		if thread(doc) {
			conn.execute(
				"UPDATE refs SET to_doc_id=?1 WHERE to_external=?2 AND to_doc_id IS NULL",
				params![
					doc.id,
					format!("{}#{}", doc.source_repo.as_deref().unwrap_or(""), doc.source_id)
				],
			)?;
			let issue_id = doc.id.replacen("github_pr:", "github_issue:", 1).replacen(
				"forgejo_pr:",
				"forgejo_issue:",
				1,
			);
			if issue_id != doc.id {
				conn.execute(
					"UPDATE refs SET to_doc_id=?1 WHERE to_doc_id=?2",
					params![doc.id, issue_id],
				)?;
			}
		}
		let mut stmt = conn.prepare("SELECT doc_id,target_id,origin_id,evidence FROM document_links WHERE target_id=?1 OR doc_id=?1")?;
		let links = stmt
			.query_map([&doc.id], |r| {
				Ok((
					r.get::<_, String>(0)?,
					r.get::<_, String>(1)?,
					r.get::<_, String>(2)?,
					r.get::<_, String>(3)?,
				))
			})?
			.collect::<rusqlite::Result<Vec<_>>>()?;
		for (a, b, origin, evidence) in links {
			bind(conn, &a, &b, &origin, &evidence)?;
		}
		if thread(doc) {
			let group = canonical_id(conn, &doc.id)?;
			let issue_id = doc.id.replacen("github_pr:", "github_issue:", 1).replacen(
				"forgejo_pr:",
				"forgejo_issue:",
				1,
			);
			for child in ids(conn, "SELECT id FROM documents WHERE parent_id IN (?1,?2) OR parent_id IN (SELECT doc_id FROM document_aliases WHERE canonical_id=?3)", params![doc.id,issue_id,group])? {
				observe(conn, &raw(conn,&child)?.unwrap())?;
			}
		}
	}
	Ok(())
}

pub(crate) fn origins(conn: &Connection, id: &str) -> Result<Vec<DocumentOrigin>> {
	let group = canonical_id(conn, id)?;
	let mut result = vec![];
	for member in ids(conn,"SELECT id FROM documents WHERE id=?1 OR id IN (SELECT doc_id FROM document_aliases WHERE canonical_id=?1) ORDER BY id", [&group])? {
		let doc = raw(conn,&member)?.unwrap();
		let evidence = conn.query_row("SELECT evidence FROM document_aliases WHERE doc_id=?1",[&member],|r|r.get(0)).optional()?;
		result.push(DocumentOrigin { url: doc.url(), id:doc.id,source_type:doc.source_type,source_repo:doc.source_repo,author:doc.author,author_id:doc.author_id,created_at:doc.created_at,updated_at:doc.updated_at,evidence });
	}
	Ok(result)
}

pub(crate) fn projected(conn: &Connection, id: &str) -> Result<Option<Document>> {
	let canonical = canonical_id(conn, id)?;
	let Some(mut doc) = raw(conn, &canonical)? else {
		return Ok(None);
	};
	let origin: Option<String> = conn
		.query_row("SELECT origin_id FROM document_aliases WHERE doc_id=?1", [&canonical], |r| {
			r.get(0)
		})
		.optional()?;
	if let Some(origin) = origin.filter(|id| id != &canonical) {
		if let Some(original) = raw(conn, &origin)? {
			doc.body = original.body;
			doc.title = original.title;
			doc.author = original.author;
			doc.author_id = original.author_id;
			doc.created_at = original.created_at;
		}
	} else if let Some(origin) = bkb_core::mirror::parse(&doc) {
		// Still show useful content while awaiting the source record; provenance
		// retains the bot timestamp and identity, without inventing origin dates.
		doc.body = Some(origin.body);
		doc.author = Some(origin.author);
		doc.author_id = None;
	}
	if let Some(parent) = &doc.parent_id {
		doc.parent_id = Some(canonical_id(conn, parent)?);
	}
	Ok(Some(doc))
}

pub(crate) fn concepts(conn: &Connection, id: &str) -> Result<Vec<String>> {
	let id = canonical_id(conn, id)?;
	ids(conn,"SELECT DISTINCT cm.concept_slug FROM concept_mentions cm LEFT JOIN document_aliases a ON a.doc_id=cm.doc_id WHERE COALESCE(a.canonical_id,cm.doc_id)=?1 ORDER BY cm.concept_slug",[id])
}

pub(crate) fn reference_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Reference> {
	Ok(Reference {
		id: row.get(0)?,
		from_doc_id: row.get(1)?,
		to_doc_id: row.get(2)?,
		ref_type: RefType::from_str(&row.get::<_, String>(3)?).unwrap_or(RefType::MentionsIssue),
		to_external: row.get(4)?,
		context: row.get(5)?,
	})
}

pub(crate) fn external_id(conn: &Connection, external: &str) -> Result<Option<String>> {
	if let Some(repo) = permalink_repo(external) {
		return permalink_id(conn, external, repo);
	}
	let Some((repo, number)) = external.rsplit_once('#') else {
		return Ok(None);
	};
	let kind = if repo.starts_with("git.rust-bitcoin.org/") {
		SourceType::ForgejoIssue
	} else {
		SourceType::GithubIssue
	};
	let id = actual_id(conn, &Document::make_id(&kind, Some(repo), number))?;
	Ok(if raw(conn, &id)?.is_some() { Some(canonical_id(conn, &id)?) } else { None })
}

pub(crate) fn canonical_refs(conn: &Connection, refs: Vec<Reference>) -> Result<Vec<Reference>> {
	let mut seen = std::collections::HashSet::new();
	let mut result = vec![];
	for mut r in refs {
		r.from_doc_id = canonical_id(conn, &r.from_doc_id)?;
		r.to_doc_id = reference_target(conn, &r)?.map(|id| canonical_id(conn, &id)).transpose()?;
		let target = r.to_doc_id.as_ref().or(r.to_external.as_ref());
		if seen.insert((r.from_doc_id.clone(), target.cloned(), r.ref_type.as_str())) {
			result.push(r);
		}
	}
	Ok(result)
}

pub(crate) fn context(
	conn: &Connection, id: &str, canonical: bool,
) -> Result<Option<DocumentContext>> {
	let doc = if canonical { projected(conn, id)? } else { raw(conn, id)? };
	let Some(doc) = doc else {
		return Ok(None);
	};
	let provenance = origins(conn, &doc.id)?;
	let members: Vec<_> = if canonical {
		provenance.iter().map(|o| o.id.as_str()).collect()
	} else {
		vec![doc.id.as_str()]
	};
	let mut outgoing = vec![];
	let mut incoming = vec![];
	let mut concepts = std::collections::BTreeSet::new();
	for member in members {
		let mut stmt = conn.prepare("SELECT id,from_doc_id,to_doc_id,ref_type,to_external,context FROM refs WHERE from_doc_id=?1")?;
		outgoing.extend(
			stmt.query_map([member], reference_row)?.collect::<rusqlite::Result<Vec<_>>>()?,
		);
		let raw_doc = raw(conn, member)?.unwrap();
		let external =
			format!("{}#{}", raw_doc.source_repo.as_deref().unwrap_or(""), raw_doc.source_id);
		let issue_id = member.replacen("github_pr:", "github_issue:", 1).replacen(
			"forgejo_pr:",
			"forgejo_issue:",
			1,
		);
		let mut stmt = conn.prepare("SELECT id,from_doc_id,to_doc_id,ref_type,to_external,context FROM refs WHERE to_doc_id IN (?1,?2) OR to_external=?3")?;
		incoming.extend(
			stmt.query_map(params![member, issue_id, external], reference_row)?
				.collect::<rusqlite::Result<Vec<_>>>()?,
		);
		concepts.extend(ids(
			conn,
			"SELECT concept_slug FROM concept_mentions WHERE doc_id=?1",
			[member],
		)?);
	}
	if canonical {
		outgoing = canonical_refs(conn, outgoing)?;
		incoming = canonical_refs(conn, incoming)?;
	}
	Ok(Some(DocumentContext {
		url: doc.url(),
		document: doc,
		provenance,
		outgoing_refs: outgoing,
		incoming_refs: incoming,
		concepts: concepts.into_iter().collect(),
	}))
}

#[cfg(test)]
mod tests {
	use crate::sqlite::SqliteStore;
	use bkb_core::model::*;
	use bkb_core::repository::forgejo_identity;
	use bkb_core::store::KnowledgeStore;

	fn thread(forgejo: bool, number: &str, body: &str) -> Document {
		let kind = if forgejo { SourceType::ForgejoIssue } else { SourceType::GithubIssue };
		let repo = if forgejo {
			forgejo_identity("lightningdevkit/rust-lightning")
		} else {
			"lightningdevkit/rust-lightning".into()
		};
		Document {
			id: Document::make_id(&kind, Some(&repo), number),
			source_type: kind,
			source_repo: Some(repo),
			source_id: number.into(),
			title: Some("Electrum script registration".into()),
			body: Some(body.into()),
			author: Some(if forgejo { "renamed" } else { "alice" }.into()),
			author_id: None,
			created_at: parse_datetime("2026-08-08T01:02:03Z").unwrap(),
			updated_at: None,
			parent_id: None,
			metadata: None,
			seq: None,
		}
	}
	#[tokio::test]
	async fn verified_import_collapses_before_limit_and_preserves_alias() {
		let store = SqliteStore::open_in_memory().unwrap();
		let gh = thread(false, "2707", "Electrum exact historical body");
		let fj = thread(true, "2707", "Electrum exact historical body");
		store.upsert_document(&gh).await.unwrap();
		store.upsert_document(&fj).await.unwrap();
		let results = store
			.search(SearchParams { query: "Electrum".into(), ..Default::default() })
			.await
			.unwrap();
		assert_eq!(results.results.len(), 1, "verified imported threads must appear once");
		assert_eq!(store.get_document(&gh.id).await.unwrap().unwrap().document.id, fj.id);
	}
	#[tokio::test]
	async fn number_collision_is_not_an_equivalence() {
		let store = SqliteStore::open_in_memory().unwrap();
		store.upsert_document(&thread(false, "4760", "Electrum issue")).await.unwrap();
		store.upsert_document(&thread(true, "4760", "Electrum unrelated upgrade")).await.unwrap();
		let results = store
			.search(SearchParams { query: "Electrum".into(), ..Default::default() })
			.await
			.unwrap();
		assert_eq!(results.results.len(), 2);
	}

	#[tokio::test]
	async fn mirrored_renumbered_issue_keeps_raw_body_and_native_state() {
		let store = SqliteStore::open_in_memory().unwrap();
		let gh = thread(false, "4760", "Electrum original report");
		let mut fj = thread(true, "4866", "unused");
		fj.author = Some("github-comment-mirror".into());
		fj.author_id = Some("72".into());
		fj.body=Some("*Mirrored from GitHub issue [lightningdevkit/rust-lightning#4760](https://github.com/lightningdevkit/rust-lightning/issues/4760), opened by [\u{60}@alice\u{60}](https://github.com/alice) on 2026-08-08.*\n\n---\n\nElectrum original report\n\n<!-- issue-mirror:v1 origin=github gh_repo=lightningdevkit/rust-lightning gh_issue=4760 -->".into());
		fj.metadata = Some(
			serde_json::json!({"state":"closed","url":"https://git.rust-bitcoin.org/lightningdevkit/rust-lightning/issues/4866"}),
		);
		store.upsert_document(&fj).await.unwrap(); // origin arrives later
		store.upsert_document(&gh).await.unwrap();
		let canonical = store.get_document(&gh.id).await.unwrap().unwrap();
		assert_eq!(canonical.document.id, fj.id);
		assert_eq!(canonical.document.body, gh.body);
		assert_eq!(canonical.document.metadata.unwrap()["state"], "closed");
		assert_eq!(canonical.provenance.len(), 2);
		assert_eq!(store.get_raw_document(&fj.id).await.unwrap().unwrap().document.body, fj.body);
		let raw = store
			.search(SearchParams {
				query: "Electrum".into(),
				deduplicate: Some(false),
				..Default::default()
			})
			.await
			.unwrap();
		assert_eq!(raw.results.len(), 2);
		let filtered = store
			.search(SearchParams {
				query: "Electrum".into(),
				source_type: Some(vec![SourceType::GithubIssue]),
				..Default::default()
			})
			.await
			.unwrap();
		assert_eq!(filtered.results[0].id, fj.id);
	}

	#[tokio::test]
	async fn timeline_deduplicates_verified_imports() {
		let store = SqliteStore::open_in_memory().unwrap();
		for fj in [false, true] {
			let doc = thread(fj, "2707", "Electrum taproot import");
			store.upsert_document(&doc).await.unwrap();
			store.upsert_concept_mention(&doc.id, "taproot", 1.0).await.unwrap();
		}
		assert_eq!(
			store.timeline("taproot", None, None).await.unwrap().events.len(),
			1,
			"timeline must not count an import twice"
		);
	}

	#[tokio::test]
	async fn exact_merge_sha_links_native_pr_without_number_guessing() {
		let store = SqliteStore::open_in_memory().unwrap();
		let sha = "abcde12345abcde12345abcde12345abcde12345678";
		let mut commit = thread(false, sha, "Merge native development");
		commit.source_type = SourceType::Commit;
		commit.id = Document::make_id(&commit.source_type, commit.source_repo.as_deref(), sha);
		store.upsert_document(&commit).await.unwrap();
		let mut pr = thread(true, "4760", "Upgrade tests");
		pr.source_type = SourceType::ForgejoPr;
		pr.id = Document::make_id(&pr.source_type, pr.source_repo.as_deref(), "4760");
		pr.metadata = Some(serde_json::json!({"merge_commit_sha":sha}));
		store.upsert_document(&pr).await.unwrap();
		let results = store
			.find_commit("native development", Some("lightningdevkit/rust-lightning"))
			.await
			.unwrap();
		assert_eq!(
			results[0].associated_prs.len(),
			1,
			"native PR must be associated by exact merge SHA"
		);
		assert_eq!(results[0].associated_prs[0].id, pr.id);
	}

	#[tokio::test]
	async fn late_duplicate_fingerprint_remains_ambiguous() {
		let store = SqliteStore::open_in_memory().unwrap();
		let gh = thread(false, "1", "original thread");
		let fj = thread(true, "1", "original thread");
		store.upsert_document(&gh).await.unwrap();
		store.upsert_document(&fj).await.unwrap();
		for (native, id, parent) in [(false, "11", &gh), (true, "22", &fj), (false, "33", &gh)] {
			let mut comment = thread(native, id, "Electrum identical comment");
			comment.source_type =
				if native { SourceType::ForgejoComment } else { SourceType::GithubComment };
			comment.id =
				Document::make_id(&comment.source_type, comment.source_repo.as_deref(), id);
			comment.parent_id = Some(parent.id.clone());
			comment.title = None;
			store.upsert_document(&comment).await.unwrap();
		}
		let results = store
			.search(SearchParams { query: "identical".into(), ..Default::default() })
			.await
			.unwrap();
		assert_eq!(
			results.results.len(),
			3,
			"a later identical candidate must invalidate a non-unique fingerprint"
		);
	}

	#[tokio::test]
	async fn reconciliation_reads_raw_records_with_stable_resume() {
		let store = SqliteStore::open_in_memory().unwrap();
		let gh = thread(false, "1", "original");
		let fj = thread(true, "1", "original");
		store.upsert_document(&gh).await.unwrap();
		store.upsert_document(&fj).await.unwrap();
		let page = store.reconciliation_page(0).await.unwrap();
		assert_eq!(
			page.len(),
			2,
			"reconciliation must visit both raw records, not their projection"
		);
		assert_eq!(page[0].1.id, gh.id);
		assert_eq!(page[1].1.id, fj.id);
		assert!(store.reconciliation_page(page[1].0).await.unwrap().is_empty());
	}

	#[tokio::test]
	async fn references_to_existing_aliases_unify_before_limit() {
		let store = SqliteStore::open_in_memory().unwrap();
		let gh = thread(false, "1", "original thread");
		let fj = thread(true, "1", "original thread");
		store.upsert_document(&gh).await.unwrap();
		store.upsert_document(&fj).await.unwrap();
		let mention = thread(false, "9", "mention");
		store.upsert_document(&mention).await.unwrap();
		store
			.insert_reference(&Reference {
				id: None,
				from_doc_id: mention.id.clone(),
				to_doc_id: None,
				ref_type: RefType::MentionsIssue,
				to_external: Some("lightningdevkit/rust-lightning#1".into()),
				context: None,
			})
			.await
			.unwrap();
		let refs = store
			.get_references("git.rust-bitcoin.org/lightningdevkit/rust-lightning#1", None, 1)
			.await
			.unwrap();
		assert_eq!(
			refs.len(),
			1,
			"references through an existing GitHub alias must resolve from Forgejo"
		);
		assert_eq!(refs[0].to_doc_id.as_deref(), Some(fj.id.as_str()));
	}

	#[tokio::test]
	async fn reject_conflicting_thread_kinds_even_with_marker() {
		let store = SqliteStore::open_in_memory().unwrap();
		let mut gh = thread(false, "4760", "Electrum PR");
		gh.source_type = SourceType::GithubPr;
		gh.id = Document::make_id(&gh.source_type, gh.source_repo.as_deref(), "4760");
		let mut fj = thread(true, "4866", "unused");
		fj.author = Some("github-comment-mirror".into());
		fj.author_id = Some("72".into());
		fj.body=Some("*Mirrored from GitHub issue [lightningdevkit/rust-lightning#4760](https://github.com/lightningdevkit/rust-lightning/issues/4760), opened by [\u{60}@alice\u{60}](https://github.com/alice) on 2026-08-08.*\n\n---\n\nElectrum PR\n\n<!-- issue-mirror:v1 origin=github gh_repo=lightningdevkit/rust-lightning gh_issue=4760 -->".into());
		store.upsert_document(&gh).await.unwrap();
		store.upsert_document(&fj).await.unwrap();
		let results = store
			.search(SearchParams { query: "Electrum".into(), ..Default::default() })
			.await
			.unwrap();
		assert_eq!(
			results.results.len(),
			2,
			"conflicting issue/PR identity must remain unresolved"
		);
	}

	#[tokio::test]
	async fn explicit_github_repository_does_not_expand() {
		let store = SqliteStore::open_in_memory().unwrap();
		let gh = thread(false, "1", "Electrum GitHub");
		store.upsert_document(&gh).await.unwrap();
		store.upsert_document(&thread(true, "2", "Electrum native Forgejo")).await.unwrap();
		let results = store
			.search(SearchParams {
				query: "Electrum".into(),
				source_repo: Some(vec!["github.com/lightningdevkit/rust-lightning".into()]),
				..Default::default()
			})
			.await
			.unwrap();
		assert_eq!(results.results.len(), 1, "explicit GitHub host selects only GitHub membership");
		assert_eq!(results.results[0].id, gh.id);
	}
}
