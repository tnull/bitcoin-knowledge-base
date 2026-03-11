use regex::Regex;

/// Simple HTML tag stripper.
pub fn strip_html_tags(html: &str) -> String {
	thread_local! {
		static RE_TAGS: Regex = Regex::new(r"<[^>]+>").unwrap();
	}
	RE_TAGS.with(|re| re.replace_all(html, "").to_string())
}

/// Unescape common HTML entities.
pub fn html_unescape(s: &str) -> String {
	s.replace("&amp;", "&")
		.replace("&lt;", "<")
		.replace("&gt;", ">")
		.replace("&quot;", "\"")
		.replace("&#39;", "'")
		.replace("&nbsp;", " ")
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_html_unescape() {
		assert_eq!(html_unescape("a &amp; b &lt; c"), "a & b < c");
		assert_eq!(html_unescape("&quot;hello&quot;"), "\"hello\"");
	}

	#[test]
	fn test_strip_html_tags() {
		assert_eq!(strip_html_tags("<b>bold</b> text"), "bold text");
		assert_eq!(strip_html_tags("<a href=\"x\">link</a>"), "link");
	}
}
