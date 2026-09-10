//! Strict recognition of the configured issue-mirror bot's complete envelopes.
use crate::model::Document;

#[derive(Debug, Clone)]
pub struct MirrorOrigin {
	pub id: String,
	pub parent_id: Option<String>,
	pub body: String,
	pub author: String,
	pub repo: String,
}

pub fn parse(doc: &Document) -> Option<MirrorOrigin> {
	use crate::model::SourceType;
	use crate::repository::forgejo_identity;
	let repo = "lightningdevkit/rust-lightning";
	let fj_repo = forgejo_identity(repo);
	let from_github = match doc.source_type {
		SourceType::ForgejoIssue | SourceType::ForgejoComment
			if doc.source_repo.as_deref() == Some(&fj_repo)
				&& doc.author_id.as_deref() == Some("72")
				&& doc.author.as_deref() == Some("github-comment-mirror") =>
		{
			true
		},
		SourceType::GithubComment
			if doc.source_repo.as_deref() == Some(repo)
				&& doc.author_id.as_deref() == Some("199857858")
				&& doc.author.as_deref() == Some("ldk-reviews-bot") =>
		{
			false
		},
		_ => return None,
	};
	let text = doc.body.as_deref()?.trim_end();
	let (envelope, marker) = text.rsplit_once("\n\n<!-- issue-mirror:v1 ")?;
	let marker = marker.strip_suffix(" -->")?;
	let mut fields = std::collections::BTreeMap::new();
	for pair in marker.split_whitespace() {
		let (key, value) = pair.split_once('=')?;
		if fields.insert(key, value).is_some() {
			return None;
		}
	}
	if fields.get("origin").copied() != Some(if from_github { "github" } else { "forgejo" })
		|| fields.get("gh_repo").copied() != Some(repo)
	{
		return None;
	}
	if let Some(value) = fields.get("fj_repo") {
		if *value != repo {
			return None;
		}
	}
	let (header, body) = envelope.split_once("\n\n---\n\n")?;
	let numeric = |name| {
		fields.get(name).copied().filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
	};
	let gh_issue = numeric("gh_issue")?;
	let thread = doc.source_type == SourceType::ForgejoIssue;
	let base = if from_github { "https://github.com" } else { "https://git.rust-bitcoin.org" };
	let origin_repo = if from_github { repo.to_owned() } else { fj_repo };
	let (author, origin_number, comment) = if thread {
		let prefix = format!("*Mirrored from GitHub issue [{repo}#{gh_issue}]({base}/{repo}/issues/{gh_issue}), opened by [\u{60}@");
		let tail = header.strip_prefix(&prefix)?;
		let (author, tail) = tail.split_once("\u{60}](")?;
		let date = tail.strip_prefix(&format!("{base}/{author}) on "))?.strip_suffix(".*")?;
		chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
		(author, gh_issue.to_owned(), None)
	} else {
		let (author, tail) = header.strip_prefix("*[\u{60}@")?.split_once("\u{60}](")?;
		let tail = tail.strip_prefix(&format!("{base}/{author}) [commented]({base}/{repo}/"))?;
		let (path, tail) = tail.split_once(") on ")?;
		if tail != if from_github { "GitHub:*" } else { "Forgejo:*" } {
			return None;
		}
		let (number, comment_id) = path.strip_prefix("issues/")?.split_once("#issuecomment-")?;
		if !number.bytes().all(|b| b.is_ascii_digit()) || number.is_empty() {
			return None;
		}
		if numeric(if from_github { "gh_comment" } else { "fj_comment" }) != Some(comment_id) {
			return None;
		}
		if from_github && number != gh_issue {
			return None;
		}
		// The reverse marker names the destination GitHub thread.
		if !from_github && doc.parent_id.as_deref()?.rsplit(':').next()? != gh_issue {
			return None;
		}
		(author, number.to_owned(), Some(comment_id))
	};
	if author.is_empty()
		|| !author.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
	{
		return None;
	}
	let kind = if from_github { SourceType::GithubIssue } else { SourceType::ForgejoIssue };
	let parent = Document::make_id(&kind, Some(&origin_repo), &origin_number);
	let id = if let Some(comment) = comment {
		Document::make_id(
			&if from_github { SourceType::GithubComment } else { SourceType::ForgejoComment },
			Some(&origin_repo),
			comment,
		)
	} else {
		parent.clone()
	};
	Some(MirrorOrigin {
		id,
		parent_id: if thread { None } else { Some(parent) },
		body: body.to_owned(),
		author: author.to_owned(),
		repo: origin_repo,
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::model::SourceType;

	fn comment() -> Document {
		let repo = "git.rust-bitcoin.org/lightningdevkit/rust-lightning";
		Document { id: Document::make_id(&SourceType::ForgejoComment, Some(repo), "99"),
			source_type: SourceType::ForgejoComment, source_repo: Some(repo.into()), source_id: "99".into(),
			title: None, body: Some("*[`@alice`](https://github.com/alice) [commented](https://github.com/lightningdevkit/rust-lightning/issues/4760#issuecomment-42) on GitHub:*\n\n---\n\nSee #12\n\n<!-- issue-mirror:v1 origin=github gh_repo=lightningdevkit/rust-lightning gh_issue=4760 gh_comment=42 -->".into()),
			author: Some("github-comment-mirror".into()), author_id: Some("72".into()),
			created_at: chrono::Utc::now(), updated_at: None,
			parent_id: Some(format!("forgejo_issue:{repo}:4866")), metadata: None, seq: None }
	}
	#[test]
	fn trusted_complete_envelope_preserves_origin() {
		let parsed = parse(&comment());
		assert!(parsed.is_some(), "trusted complete mirror envelope must resolve its origin");
		let parsed = parsed.unwrap();
		assert_eq!(parsed.id, "github_comment:lightningdevkit/rust-lightning:42");
		assert_eq!(parsed.body, "See #12");
		assert_eq!(parsed.author, "alice");
	}
	#[test]
	fn reject_spoofs_quotes_and_mismatched_links() {
		let original = comment();
		let mut human = original.clone();
		human.author_id = Some("1".into());
		assert!(parse(&human).is_none());
		let mut quoted = original.clone();
		quoted.body.as_mut().unwrap().push_str("\nMy reply");
		assert!(parse(&quoted).is_none());
		let mut mismatch = original.clone();
		mismatch.body = mismatch.body.map(|b| b.replace("gh_comment=42", "gh_comment=43"));
		assert!(parse(&mismatch).is_none());
		let mut duplicate = original;
		duplicate.body =
			duplicate.body.map(|b| b.replace("gh_comment=42", "gh_comment=42 gh_comment=43"));
		assert!(parse(&duplicate).is_none());
	}
}
