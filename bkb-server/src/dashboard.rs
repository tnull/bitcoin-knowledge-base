use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse};

use crate::api::AppState;

/// Check HTTP Basic Auth against the configured admin password.
///
/// Accepts any username (including empty); only the password is checked.
fn check_admin_auth(state: &AppState, headers: &HeaderMap) -> Result<(), StatusCode> {
	let password = match state.admin_password {
		Some(ref pw) => pw,
		None => return Err(StatusCode::NOT_FOUND),
	};

	let auth_header = headers
		.get("authorization")
		.and_then(|v| v.to_str().ok())
		.ok_or(StatusCode::UNAUTHORIZED)?;

	let encoded = auth_header.strip_prefix("Basic ").ok_or(StatusCode::UNAUTHORIZED)?;

	let decoded = base64_decode(encoded).map_err(|_| StatusCode::UNAUTHORIZED)?;

	// Format is "username:password" -- we only check the password part
	let provided_password = match decoded.find(':') {
		Some(idx) => &decoded[idx + 1..],
		None => &decoded,
	};

	if provided_password == password {
		Ok(())
	} else {
		Err(StatusCode::UNAUTHORIZED)
	}
}

/// Minimal base64 decoder (RFC 4648 standard alphabet).
///
/// We avoid pulling in the `base64` crate for this single use case.
fn base64_decode(input: &str) -> Result<String, ()> {
	const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

	fn val(c: u8) -> Result<u32, ()> {
		match c {
			b'A'..=b'Z' => Ok((c - b'A') as u32),
			b'a'..=b'z' => Ok((c - b'a' + 26) as u32),
			b'0'..=b'9' => Ok((c - b'0' + 52) as u32),
			b'+' => Ok(62),
			b'/' => Ok(63),
			_ => Err(()),
		}
	}

	let _ = TABLE; // suppress unused warning in test builds

	let input = input.trim_end_matches('=');
	let mut out = Vec::with_capacity(input.len() * 3 / 4);
	let bytes = input.as_bytes();

	let mut i = 0;
	while i + 3 < bytes.len() {
		let a = val(bytes[i])?;
		let b = val(bytes[i + 1])?;
		let c = val(bytes[i + 2])?;
		let d = val(bytes[i + 3])?;
		let triple = (a << 18) | (b << 12) | (c << 6) | d;
		out.push((triple >> 16) as u8);
		out.push((triple >> 8) as u8);
		out.push(triple as u8);
		i += 4;
	}

	let remaining = bytes.len() - i;
	if remaining == 2 {
		let a = val(bytes[i])?;
		let b = val(bytes[i + 1])?;
		let triple = (a << 18) | (b << 12);
		out.push((triple >> 16) as u8);
	} else if remaining == 3 {
		let a = val(bytes[i])?;
		let b = val(bytes[i + 1])?;
		let c = val(bytes[i + 2])?;
		let triple = (a << 18) | (b << 12) | (c << 6);
		out.push((triple >> 16) as u8);
		out.push((triple >> 8) as u8);
	}

	String::from_utf8(out).map_err(|_| ())
}

/// `GET /metrics` -- Prometheus text exposition format.
pub async fn metrics_endpoint(
	State(state): State<AppState>, headers: HeaderMap,
) -> impl IntoResponse {
	if let Err(status) = check_admin_auth(&state, &headers) {
		return (status, [("www-authenticate", "Basic realm=\"BKB Admin\"")], String::new());
	}

	let metrics = match state.metrics {
		Some(ref m) => m,
		None => {
			return (
				StatusCode::OK,
				[("content-type", "text/plain; version=0.0.4; charset=utf-8")],
				"# No metrics available (metrics not initialized)\n".to_string(),
			);
		},
	};

	let doc_stats = state.store.get_stats().await.unwrap_or_default();
	let body = metrics.render_prometheus(&doc_stats);
	(StatusCode::OK, [("content-type", "text/plain; version=0.0.4; charset=utf-8")], body)
}

/// `GET /dashboard` -- lightweight HTML admin dashboard.
pub async fn dashboard_page(
	State(state): State<AppState>, headers: HeaderMap,
) -> impl IntoResponse {
	if let Err(status) = check_admin_auth(&state, &headers) {
		return (status, [("www-authenticate", "Basic realm=\"BKB Admin\"")], Html(String::new()));
	}

	let metrics = match state.metrics {
		Some(ref m) => m,
		None => {
			return (
				StatusCode::OK,
				[("www-authenticate", "")],
				Html("<html><body><p>Metrics not initialized.</p></body></html>".to_string()),
			);
		},
	};

	let doc_stats = state.store.get_stats().await.unwrap_or_default();
	let git_hash = option_env!("BKB_GIT_HASH").unwrap_or("unknown");
	let html = metrics.render_dashboard_html(&doc_stats, git_hash);
	(StatusCode::OK, [("www-authenticate", "")], Html(html))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_base64_decode_basic() {
		// ":" in base64 is "Og=="
		assert_eq!(base64_decode("Og==").unwrap(), ":");
		// ":test123" in base64
		assert_eq!(base64_decode("OnRlc3QxMjM=").unwrap(), ":test123");
		// "admin:secret" in base64
		assert_eq!(base64_decode("YWRtaW46c2VjcmV0").unwrap(), "admin:secret");
	}

	#[test]
	fn test_base64_decode_no_padding() {
		assert_eq!(base64_decode("Og").unwrap(), ":");
		assert_eq!(base64_decode("YWRtaW46c2VjcmV0").unwrap(), "admin:secret");
	}

	#[test]
	fn test_check_admin_auth_no_password_configured() {
		let state = AppState {
			store: std::sync::Arc::new(
				bkb_store::sqlite::SqliteStore::open(std::path::Path::new(":memory:")).unwrap(),
			),
			metrics: None,
			admin_password: None,
		};
		let headers = HeaderMap::new();
		assert_eq!(check_admin_auth(&state, &headers), Err(StatusCode::NOT_FOUND));
	}

	#[test]
	fn test_check_admin_auth_missing_header() {
		let state = AppState {
			store: std::sync::Arc::new(
				bkb_store::sqlite::SqliteStore::open(std::path::Path::new(":memory:")).unwrap(),
			),
			metrics: None,
			admin_password: Some("test123".to_string()),
		};
		let headers = HeaderMap::new();
		assert_eq!(check_admin_auth(&state, &headers), Err(StatusCode::UNAUTHORIZED));
	}

	#[test]
	fn test_check_admin_auth_correct_password() {
		let state = AppState {
			store: std::sync::Arc::new(
				bkb_store::sqlite::SqliteStore::open(std::path::Path::new(":memory:")).unwrap(),
			),
			metrics: None,
			admin_password: Some("test123".to_string()),
		};
		let mut headers = HeaderMap::new();
		// ":test123" -> base64 "OnRlc3QxMjM="
		headers.insert("authorization", "Basic OnRlc3QxMjM=".parse().unwrap());
		assert_eq!(check_admin_auth(&state, &headers), Ok(()));
	}

	#[test]
	fn test_check_admin_auth_wrong_password() {
		let state = AppState {
			store: std::sync::Arc::new(
				bkb_store::sqlite::SqliteStore::open(std::path::Path::new(":memory:")).unwrap(),
			),
			metrics: None,
			admin_password: Some("test123".to_string()),
		};
		let mut headers = HeaderMap::new();
		// ":wrong" -> base64 "Ondyb25n"
		headers.insert("authorization", "Basic Ondyb25n".parse().unwrap());
		assert_eq!(check_admin_auth(&state, &headers), Err(StatusCode::UNAUTHORIZED));
	}
}
