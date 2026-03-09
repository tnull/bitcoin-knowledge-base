use anyhow::Result;
use async_trait::async_trait;

use chrono::{DateTime, Utc};

use crate::model::{
	CommitContext, DocumentContext, Reference, SearchParams, SearchResults, Timeline,
};

/// Trait for querying the Bitcoin Knowledge Base.
///
/// Implemented by `LocalSqliteStore` (direct DB queries) and
/// `RemoteApiStore` (HTTP client to the BKB service).
#[async_trait]
pub trait KnowledgeStore: Send + Sync {
	/// Full-text (and optionally semantic) search across all documents.
	async fn search(&self, params: SearchParams) -> Result<SearchResults>;

	/// Get a single document by ID with full content, references, and concepts.
	async fn get_document(&self, id: &str) -> Result<Option<DocumentContext>>;

	/// Find all documents referencing a given entity (BIP, BOLT, issue, commit, or concept).
	async fn get_references(
		&self, entity: &str, ref_type: Option<&str>, limit: u32,
	) -> Result<Vec<Reference>>;

	/// Get comprehensive context for a BIP: spec text, all referencing documents, and incoming
	/// refs.
	async fn lookup_bip(&self, number: u32) -> Result<Option<DocumentContext>>;

	/// Get comprehensive context for a BOLT: spec text, all referencing documents, and incoming
	/// refs.
	async fn lookup_bolt(&self, number: u32) -> Result<Option<DocumentContext>>;

	/// Get comprehensive context for a bLIP: spec text, all referencing documents, and incoming
	/// refs.
	async fn lookup_blip(&self, number: u32) -> Result<Option<DocumentContext>>;

	/// Chronological timeline of a concept across all sources.
	async fn timeline(
		&self, concept: &str, after: Option<DateTime<Utc>>, before: Option<DateTime<Utc>>,
	) -> Result<Timeline>;

	/// Find commits matching a query, with associated PR and discussion context.
	async fn find_commit(&self, query: &str, repo: Option<&str>) -> Result<Vec<CommitContext>>;
}
