use bkb_core::model::Reference;

use crate::sources::github::extract_issue_refs;

/// Output of the enrichment pipeline for a single document.
pub struct EnrichmentOutput {
	/// Cross-references found in the document body.
	pub references: Vec<Reference>,
}

/// Run the enrichment pipeline on a document body.
///
/// Currently only extracts cross-references (BIP, BOLT, issue mentions).
/// Future phases will add concept tagging and embedding generation.
pub fn enrich(doc_id: &str, body: &str, source_repo: Option<&str>) -> EnrichmentOutput {
	let repo = source_repo.unwrap_or("");
	let references = extract_issue_refs(body, doc_id, repo);
	EnrichmentOutput { references }
}
