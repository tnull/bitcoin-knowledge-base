//! Host-aware references, including the original context of mirrored text.
use crate::sources::github::extract_issue_refs;
use bkb_core::model::{Document, RefType, Reference, SourceType};

pub fn extract(doc: &Document) -> Vec<Reference> {
	use bkb_core::repository::{is_migrated, logical_repo, FORGEJO_HOST};
	use regex::Regex;
	use std::sync::LazyLock;
	static URL: LazyLock<Regex> = LazyLock::new(|| {
		Regex::new(r"https://(github\.com|git\.rust-bitcoin\.org)/([A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+)/(issues|pull|pulls|commit)/([0-9a-f]+)(?:#(issuecomment-|discussion_r|pullrequestreview-|discussioncomment-|pullreview-)([0-9]+))?").unwrap()
	});
	static QUALIFIED: LazyLock<Regex> = LazyLock::new(|| {
		Regex::new(
			r"(github\.com|git\.rust-bitcoin\.org)/([A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+)#([0-9]+)",
		)
		.unwrap()
	});
	let mirrored = bkb_core::mirror::parse(doc);
	let body =
		mirrored.as_ref().map(|m| m.body.as_str()).unwrap_or(doc.body.as_deref().unwrap_or(""));
	let repo = mirrored
		.as_ref()
		.map(|m| m.repo.as_str())
		.unwrap_or(doc.source_repo.as_deref().unwrap_or(""));
	let mut remaining = body.to_owned();
	let mut refs = vec![];
	for cap in URL.captures_iter(body) {
		let fj = &cap[1] == FORGEJO_HOST;
		let repo = if fj { format!("{FORGEJO_HOST}/{}", &cap[2]) } else { cap[2].to_owned() };
		let commit = &cap[3] == "commit";
		let pr = matches!(&cap[3], "pull" | "pulls");
		let (kind, source_id) = if commit {
			(SourceType::Commit, cap[4].to_owned())
		} else if let Some(fragment) = cap.get(5) {
			let kind = match (fj, fragment.as_str()) {
				(false, "issuecomment-") => SourceType::GithubComment,
				(false, "discussion_r") => SourceType::GithubReviewComment,
				(false, "pullrequestreview-") => SourceType::GithubReview,
				(true, "issuecomment-") => SourceType::ForgejoComment,
				(true, "discussion_r" | "discussioncomment-") => SourceType::ForgejoReviewComment,
				(true, "pullreview-" | "pullrequestreview-") => SourceType::ForgejoReview,
				_ => continue,
			};
			(kind, cap[6].to_owned())
		} else {
			(
				match (fj, pr) {
					(false, false) => SourceType::GithubIssue,
					(false, true) => SourceType::GithubPr,
					(true, false) => SourceType::ForgejoIssue,
					(true, true) => SourceType::ForgejoPr,
				},
				cap[4].to_owned(),
			)
		};
		let target_repo = if commit { logical_repo(&repo) } else { &repo };
		// Forgejo uses issuecomment anchors for ordinary comments, inline
		// comments, and review summaries whose API IDs differ from the anchor.
		let ambiguous_anchor = fj && cap.get(5).is_some_and(|v| v.as_str() == "issuecomment-");
		refs.push(Reference {
			id: None,
			from_doc_id: doc.id.clone(),
			to_doc_id: (!ambiguous_anchor)
				.then(|| Document::make_id(&kind, Some(target_repo), &source_id)),
			ref_type: if commit {
				RefType::ReferencesCommit
			} else if pr {
				RefType::MentionsPr
			} else {
				RefType::MentionsIssue
			},
			to_external: Some(if commit {
				format!("commit:{target_repo}:{}", &cap[4])
			} else if cap.get(5).is_some() {
				cap[0].to_owned()
			} else {
				format!("{repo}#{}", &cap[4])
			}),
			context: Some(cap[0].to_owned()),
		});
		remaining = remaining.replace(&cap[0], " ");
	}
	let qualified_text = remaining.clone();
	for cap in QUALIFIED.captures_iter(&qualified_text) {
		let repo = if &cap[1] == FORGEJO_HOST {
			format!("{FORGEJO_HOST}/{}", &cap[2])
		} else {
			cap[2].to_owned()
		};
		refs.push(Reference {
			id: None,
			from_doc_id: doc.id.clone(),
			to_doc_id: None,
			ref_type: RefType::MentionsIssue,
			to_external: Some(format!("{repo}#{}", &cap[3])),
			context: Some(cap[0].into()),
		});
		remaining = remaining.replace(&cap[0], " ");
	}
	for mut reference in extract_issue_refs(&remaining, &doc.id, repo) {
		if let Some(external) = reference.to_external.as_mut() {
			if let Some((target, _)) = external.rsplit_once('#') {
				if doc.source_type == SourceType::Commit && is_migrated(repo) && target == repo {
					continue;
				}
				if repo.starts_with(&format!("{FORGEJO_HOST}/"))
					&& !target.starts_with(&format!("{FORGEJO_HOST}/"))
				{
					*external = format!("{FORGEJO_HOST}/{external}");
				}
			}
		}
		refs.push(reference);
	}
	refs
}

#[cfg(test)]
mod tests {
	use super::*;
	fn doc() -> Document {
		Document { id:"forgejo_issue:git.rust-bitcoin.org/lightningdevkit/rust-lightning:4760".into(),source_type:SourceType::ForgejoIssue,source_repo:Some("git.rust-bitcoin.org/lightningdevkit/rust-lightning".into()),source_id:"4760".into(),title:None,body:Some("See https://github.com/lightningdevkit/rust-lightning/issues/4760 and https://git.rust-bitcoin.org/lightningdevkit/rust-lightning/pulls/4760#issuecomment-42 and #123".into()),author:None,author_id:None,created_at:chrono::Utc::now(),updated_at:None,parent_id:None,metadata:None,seq:None }
	}
	#[test]
	fn explicit_forge_links_keep_host_and_comment_identity() {
		let refs = extract(&doc());
		let comment = refs
			.iter()
			.find(|r| {
				r.to_external.as_deref() == Some(
			"https://git.rust-bitcoin.org/lightningdevkit/rust-lightning/pulls/4760#issuecomment-42",
		)
			})
			.expect("Forgejo permalink must retain its host and anchor");
		assert!(
			comment.to_doc_id.is_none(),
			"a shared Forgejo comment anchor cannot identify its document kind"
		);
		assert!(refs
			.iter()
			.any(|r| r.to_external.as_deref() == Some("lightningdevkit/rust-lightning#4760")));
		assert!(refs.iter().any(|r| r.to_external.as_deref()
			== Some("git.rust-bitcoin.org/lightningdevkit/rust-lightning#123")));
	}
	#[test]
	fn shared_commit_bare_number_is_ambiguous() {
		let mut doc = doc();
		doc.source_type = SourceType::Commit;
		doc.source_repo = Some("lightningdevkit/rust-lightning".into());
		doc.body = Some("Merge #4760\nBIP-340".into());
		let refs = extract(&doc);
		assert!(
			!refs
				.iter()
				.any(|r| r.to_external.as_deref() == Some("lightningdevkit/rust-lightning#4760")),
			"shared Git cannot choose a forge from a bare number"
		);
		assert!(refs.iter().any(|r| r.to_external.as_deref() == Some("BIP-340")));
	}
}
