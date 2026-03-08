use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use bkb_core::model::{SearchParams, SourceType};
use bkb_core::store::KnowledgeStore;
use bkb_store::sqlite::SqliteStore;

type AppState = Arc<SqliteStore>;

/// Start the HTTP API server.
pub async fn serve(store: AppState, addr: SocketAddr) -> Result<()> {
	let app = Router::new()
		.route("/search", get(search))
		.route("/document/{id}", get(get_document))
		.route("/health", get(health))
		.layer(CorsLayer::permissive())
		.layer(TraceLayer::new_for_http())
		.with_state(store);

	let listener = tokio::net::TcpListener::bind(addr).await?;
	axum::serve(listener, app).await?;
	Ok(())
}

/// Query parameters for the search endpoint.
#[derive(Debug, Deserialize)]
struct SearchQuery {
	q: String,
	source_type: Option<String>,
	source_repo: Option<String>,
	author: Option<String>,
	after: Option<String>,
	before: Option<String>,
	semantic: Option<bool>,
	limit: Option<u32>,
}

async fn search(
	State(store): State<AppState>, Query(query): Query<SearchQuery>,
) -> impl IntoResponse {
	let source_types = query
		.source_type
		.as_ref()
		.map(|s| s.split(',').filter_map(|t| SourceType::from_str(t.trim())).collect::<Vec<_>>());

	let source_repos = query
		.source_repo
		.as_ref()
		.map(|s| s.split(',').map(|r| r.trim().to_string()).collect::<Vec<_>>());

	let after = query
		.after
		.as_ref()
		.and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
		.map(|dt| dt.with_timezone(&chrono::Utc));

	let before = query
		.before
		.as_ref()
		.and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
		.map(|dt| dt.with_timezone(&chrono::Utc));

	let params = SearchParams {
		query: query.q,
		source_type: source_types,
		source_repo: source_repos,
		author: query.author,
		after,
		before,
		semantic: query.semantic.unwrap_or(false),
		limit: query.limit,
	};

	match store.search(params).await {
		Ok(results) => (StatusCode::OK, Json(serde_json::to_value(results).unwrap())),
		Err(e) => {
			(StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e.to_string() })))
		},
	}
}

async fn get_document(State(store): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
	// The document ID may contain colons, which are percent-encoded in URLs.
	// axum already decodes them for us.
	match store.get_document(&id).await {
		Ok(Some(ctx)) => (StatusCode::OK, Json(serde_json::to_value(ctx).unwrap())),
		Ok(None) => {
			(StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "document not found" })))
		},
		Err(e) => {
			(StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e.to_string() })))
		},
	}
}

async fn health() -> impl IntoResponse {
	Json(serde_json::json!({ "status": "ok" }))
}
