use regex::Regex;

use bkb_core::bitcoin::CONCEPTS;
use bkb_core::model::Reference;

use crate::sources::github::extract_issue_refs;

/// Output of the enrichment pipeline for a single document.
pub struct EnrichmentOutput {
	/// Cross-references found in the document body.
	pub references: Vec<Reference>,
	/// Concept tags matched in the document body (slug, confidence).
	pub concept_tags: Vec<(String, f32)>,
}

/// Run the enrichment pipeline on a document body.
///
/// Extracts cross-references (BIP, BOLT, issue mentions) and tags Bitcoin
/// concepts found in the text using word-boundary matching.
pub fn enrich(doc_id: &str, body: &str, source_repo: Option<&str>) -> EnrichmentOutput {
	let repo = source_repo.unwrap_or("");
	let references = extract_issue_refs(body, doc_id, repo);
	let concept_tags = tag_concepts(body);
	EnrichmentOutput { references, concept_tags }
}

/// Tag a document body with matching Bitcoin concepts.
///
/// For each concept in the vocabulary, checks whether any of its aliases
/// appear in the text (case-insensitive, word-boundary match). Returns
/// `(slug, confidence)` pairs with confidence 1.0 for literal matches.
fn tag_concepts(body: &str) -> Vec<(String, f32)> {
	let lower_body = body.to_lowercase();
	let mut tags = Vec::new();

	for concept in CONCEPTS {
		let matched = concept.aliases.iter().any(|alias| {
			// Build a regex that matches the alias on word boundaries.
			// Escape regex metacharacters in the alias, then wrap with \b.
			let escaped = regex::escape(alias);
			let pattern = format!(r"(?i)\b{}\b", escaped);
			if let Ok(re) = Regex::new(&pattern) {
				re.is_match(&lower_body)
			} else {
				false
			}
		});

		if matched {
			tags.push((concept.slug.to_string(), 1.0));
		}
	}

	tags
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_tag_taproot_via_schnorr() {
		let tags = tag_concepts("Implementing BIP-340 schnorr signatures");
		let slugs: Vec<&str> = tags.iter().map(|(s, _)| s.as_str()).collect();
		assert!(slugs.contains(&"taproot"), "expected 'taproot' tag, got {:?}", slugs);
	}

	#[test]
	fn test_tag_htlc() {
		let tags = tag_concepts("HTLC timeout");
		let slugs: Vec<&str> = tags.iter().map(|(s, _)| s.as_str()).collect();
		assert!(slugs.contains(&"htlc"), "expected 'htlc' tag, got {:?}", slugs);
	}

	#[test]
	fn test_tag_covenants_and_ctv() {
		let tags = tag_concepts("This is about covenants and OP_CTV");
		let slugs: Vec<&str> = tags.iter().map(|(s, _)| s.as_str()).collect();
		assert!(slugs.contains(&"covenants"), "expected 'covenants' tag, got {:?}", slugs);
		assert!(
			slugs.contains(&"op-checktemplateverify"),
			"expected 'op-checktemplateverify' tag, got {:?}",
			slugs
		);
	}

	#[test]
	fn test_no_false_positive_op_cat() {
		let tags = tag_concepts("The category is fine");
		let slugs: Vec<&str> = tags.iter().map(|(s, _)| s.as_str()).collect();
		assert!(!slugs.contains(&"op-cat"), "should NOT tag 'op-cat', got {:?}", slugs);
	}
}
