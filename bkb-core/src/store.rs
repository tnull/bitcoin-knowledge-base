use anyhow::Result;
use async_trait::async_trait;

use crate::model::{DocumentContext, SearchParams, SearchResults};

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
}
