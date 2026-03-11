use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use axum::extract::{Path, Query, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use bkb_core::model::{parse_datetime, SearchParams, SourceType};
use bkb_core::store::KnowledgeStore;
use bkb_ingest::metrics::Metrics;
use bkb_store::sqlite::SqliteStore;

use crate::dashboard;
use crate::examples;
use crate::landing;

/// Shared application state for all handlers.
#[derive(Clone)]
pub struct AppState {
	pub store: Arc<SqliteStore>,
	pub metrics: Option<Arc<Metrics>>,
	pub admin_password: Option<String>,
}

/// Middleware that increments the request counter.
async fn count_requests(
	State(state): State<AppState>, req: Request, next: Next,
) -> impl IntoResponse {
	if let Some(ref metrics) = state.metrics {
		metrics.record_request();
	}
	next.run(req).await
}

/// Start the HTTP API server.
pub async fn serve(state: AppState, addr: SocketAddr) -> Result<()> {
	let mut app = Router::new()
		.route("/", get(landing::landing_page))
		.route("/logo.png", get(landing::logo))
		.route("/examples", get(examples::examples_page))
		.route("/search", get(search))
		.route("/document/{id}", get(get_document))
		.route("/references/{entity}", get(get_references))
		.route("/bip/{number}", get(get_bip))
		.route("/bolt/{number}", get(get_bolt))
		.route("/blip/{number}", get(get_blip))
		.route("/timeline/{concept}", get(get_timeline))
		.route("/find_commit", get(find_commit))
		.route("/health", get(health));

	// Only register admin routes if a password is configured
	if state.admin_password.is_some() {
		app = app
			.route("/metrics", get(dashboard::metrics_endpoint))
			.route("/dashboard", get(dashboard::dashboard_page));
	}

	let app = app
		.layer(middleware::from_fn_with_state(state.clone(), count_requests))
		.layer(CorsLayer::permissive())
		.layer(TraceLayer::new_for_http())
		.with_state(state);

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
	State(state): State<AppState>, Query(query): Query<SearchQuery>,
) -> impl IntoResponse {
	let source_types = query
		.source_type
		.as_ref()
		.map(|s| s.split(',').filter_map(|t| SourceType::from_str(t.trim())).collect::<Vec<_>>());

	let source_repos = query
		.source_repo
		.as_ref()
		.map(|s| s.split(',').map(|r| r.trim().to_string()).collect::<Vec<_>>());

	let after = query.after.as_ref().and_then(|s| parse_datetime(s));

	let before = query.before.as_ref().and_then(|s| parse_datetime(s));

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

	match state.store.search(params).await {
		Ok(results) => (StatusCode::OK, Json(serde_json::to_value(results).unwrap())),
		Err(e) => {
			(StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e.to_string() })))
		},
	}
}

async fn get_document(State(state): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
	// The document ID may contain colons, which are percent-encoded in URLs.
	// axum already decodes them for us.
	match state.store.get_document(&id).await {
		Ok(Some(ctx)) => (StatusCode::OK, Json(serde_json::to_value(ctx).unwrap())),
		Ok(None) => {
			(StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "document not found" })))
		},
		Err(e) => {
			(StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e.to_string() })))
		},
	}
}

#[derive(Debug, Deserialize)]
struct ReferencesQuery {
	ref_type: Option<String>,
	limit: Option<u32>,
}

async fn get_references(
	State(state): State<AppState>, Path(entity): Path<String>, Query(query): Query<ReferencesQuery>,
) -> impl IntoResponse {
	let limit = query.limit.unwrap_or(50).min(100);
	match state.store.get_references(&entity, query.ref_type.as_deref(), limit).await {
		Ok(refs) => (StatusCode::OK, Json(serde_json::to_value(refs).unwrap())),
		Err(e) => {
			(StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e.to_string() })))
		},
	}
}

async fn get_bip(State(state): State<AppState>, Path(number): Path<u32>) -> impl IntoResponse {
	match state.store.lookup_bip(number).await {
		Ok(Some(ctx)) => (StatusCode::OK, Json(serde_json::to_value(ctx).unwrap())),
		Ok(None) => (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "BIP not found" }))),
		Err(e) => {
			(StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e.to_string() })))
		},
	}
}

async fn get_bolt(State(state): State<AppState>, Path(number): Path<u32>) -> impl IntoResponse {
	match state.store.lookup_bolt(number).await {
		Ok(Some(ctx)) => (StatusCode::OK, Json(serde_json::to_value(ctx).unwrap())),
		Ok(None) => (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "BOLT not found" }))),
		Err(e) => {
			(StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e.to_string() })))
		},
	}
}

async fn get_blip(State(state): State<AppState>, Path(number): Path<u32>) -> impl IntoResponse {
	match state.store.lookup_blip(number).await {
		Ok(Some(ctx)) => (StatusCode::OK, Json(serde_json::to_value(ctx).unwrap())),
		Ok(None) => (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "bLIP not found" }))),
		Err(e) => {
			(StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e.to_string() })))
		},
	}
}

#[derive(Debug, Deserialize)]
struct TimelineQuery {
	after: Option<String>,
	before: Option<String>,
}

async fn get_timeline(
	State(state): State<AppState>, Path(concept): Path<String>, Query(query): Query<TimelineQuery>,
) -> impl IntoResponse {
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

	match state.store.timeline(&concept, after, before).await {
		Ok(timeline) => (StatusCode::OK, Json(serde_json::to_value(timeline).unwrap())),
		Err(e) => {
			(StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e.to_string() })))
		},
	}
}

#[derive(Debug, Deserialize)]
struct FindCommitQuery {
	q: String,
	repo: Option<String>,
}

async fn find_commit(
	State(state): State<AppState>, Query(query): Query<FindCommitQuery>,
) -> impl IntoResponse {
	match state.store.find_commit(&query.q, query.repo.as_deref()).await {
		Ok(results) => (StatusCode::OK, Json(serde_json::to_value(results).unwrap())),
		Err(e) => {
			(StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e.to_string() })))
		},
	}
}

async fn health(State(state): State<AppState>) -> impl IntoResponse {
	let stats = state.store.get_stats().await.unwrap_or_default();
	let total: i64 = stats.iter().map(|(_, c)| c).sum();
	let by_type: serde_json::Value = stats
		.into_iter()
		.map(|(t, c)| (t, serde_json::Value::from(c)))
		.collect::<serde_json::Map<String, serde_json::Value>>()
		.into();

	Json(serde_json::json!({
		"status": "ok",
		"version": env!("CARGO_PKG_VERSION"),
		"git_hash": option_env!("BKB_GIT_HASH").unwrap_or("unknown"),
		"documents": {
			"total": total,
			"by_type": by_type,
		}
	}))
}
