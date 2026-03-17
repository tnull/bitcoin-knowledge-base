use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::Result;
use axum::extract::{Path, Query, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::IntoResponse;
use axum::routing::{get, post};
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
use crate::openapi;
use crate::sources;

/// Progress tracker for a running re-enrich job.
pub struct ReenrichProgress {
	pub total: AtomicU64,
	pub done: AtomicU64,
}

/// Shared application state for all handlers.
#[derive(Clone)]
pub struct AppState {
	pub store: Arc<SqliteStore>,
	pub metrics: Option<Arc<Metrics>>,
	pub admin_password: Option<String>,
	/// Active re-enrich jobs, keyed by source_type.
	pub reenrich_jobs: Arc<tokio::sync::Mutex<HashMap<String, Arc<ReenrichProgress>>>>,
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
		.route("/sources", get(sources::sources_page))
		.route("/search", get(search))
		.route("/document/{id}", get(get_document))
		.route("/references/{entity}", get(get_references))
		.route("/bip/{number}", get(get_bip))
		.route("/bolt/{number}", get(get_bolt))
		.route("/blip/{number}", get(get_blip))
		.route("/lud/{number}", get(get_lud))
		.route("/nut/{number}", get(get_nut))
		.route("/timeline/{concept}", get(get_timeline))
		.route("/find_commit", get(find_commit))
		.route("/health", get(health))
		.route("/openapi.json", get(openapi::openapi_spec));

	// Only register admin routes if a password is configured
	if state.admin_password.is_some() {
		app = app
			.route("/metrics", get(dashboard::metrics_endpoint))
			.route("/dashboard", get(dashboard::dashboard_page))
			.route("/admin/reset/{source_type}", post(admin_reset_source_type))
			.route("/admin/reenrich/{source_type}", post(admin_reenrich_source_type))
			.route("/admin/reenrich/{source_type}/status", get(admin_reenrich_status));
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

async fn get_lud(State(state): State<AppState>, Path(number): Path<u32>) -> impl IntoResponse {
	match state.store.lookup_lud(number).await {
		Ok(Some(ctx)) => (StatusCode::OK, Json(serde_json::to_value(ctx).unwrap())),
		Ok(None) => (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "LUD not found" }))),
		Err(e) => {
			(StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e.to_string() })))
		},
	}
}

async fn get_nut(State(state): State<AppState>, Path(number): Path<u32>) -> impl IntoResponse {
	match state.store.lookup_nut(number).await {
		Ok(Some(ctx)) => (StatusCode::OK, Json(serde_json::to_value(ctx).unwrap())),
		Ok(None) => (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "NUT not found" }))),
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

/// Map a document `source_type` to the sync_state `source_id` pattern(s) that
/// produce documents of that type.  Patterns with `%` are LIKE-matched.
fn sync_patterns_for_source_type(source_type: &str) -> Vec<String> {
	match source_type {
		"bip" => vec!["specs:bips".into()],
		"bolt" => vec!["specs:bolts".into()],
		"blip" => vec!["specs:blips".into()],
		"lud" => vec!["specs:luds".into()],
		"nut" => vec!["specs:nuts".into()],
		"github_issue" | "github_pr" => vec!["github:%:issues".into()],
		"github_comment" | "github_review" | "github_review_comment" => {
			vec!["github:%:comments".into()]
		},
		"github_discussion" | "github_discussion_comment" => {
			vec!["github:%:discussions".into()]
		},
		"commit" => vec!["commits:%".into()],
		"irc_log" => vec!["irc:%".into()],
		"delving_topic" | "delving_post" => vec!["delving:%".into()],
		"mailing_list_msg" => vec!["mailing_list:%".into(), "mail_archive:%".into()],
		"optech_newsletter" | "optech_topic" | "optech_blog" => vec!["optech:%".into()],
		"bitcointalk_topic" | "bitcointalk_post" => vec!["bitcointalk".into()],
		_ => vec![],
	}
}

async fn admin_reset_source_type(
	State(state): State<AppState>, headers: axum::http::HeaderMap, Path(source_type): Path<String>,
) -> impl IntoResponse {
	if let Err(status) = dashboard::check_admin_auth(&state, &headers) {
		return (
			status,
			[("www-authenticate", "Basic realm=\"BKB Admin\"")],
			Json(serde_json::json!({ "error": "unauthorized" })),
		);
	}

	// Validate the source type
	if SourceType::from_str(&source_type).is_none() {
		return (
			StatusCode::BAD_REQUEST,
			[("www-authenticate", "")],
			Json(serde_json::json!({ "error": format!("unknown source type: {}", source_type) })),
		);
	}

	let patterns = sync_patterns_for_source_type(&source_type);

	match state.store.reset_source_type(&source_type, &patterns).await {
		Ok(deleted) => {
			tracing::info!(
				source_type = %source_type,
				deleted,
				sync_patterns = ?patterns,
				"reset source type"
			);
			(
				StatusCode::OK,
				[("www-authenticate", "")],
				Json(serde_json::json!({
					"source_type": source_type,
					"documents_deleted": deleted,
					"sync_states_cleared": patterns,
				})),
			)
		},
		Err(e) => (
			StatusCode::INTERNAL_SERVER_ERROR,
			[("www-authenticate", "")],
			Json(serde_json::json!({ "error": e.to_string() })),
		),
	}
}

async fn admin_reenrich_source_type(
	State(state): State<AppState>, headers: axum::http::HeaderMap, Path(source_type): Path<String>,
) -> impl IntoResponse {
	if let Err(status) = dashboard::check_admin_auth(&state, &headers) {
		return (
			status,
			[("www-authenticate", "Basic realm=\"BKB Admin\"")],
			Json(serde_json::json!({ "error": "unauthorized" })),
		);
	}

	if SourceType::from_str(&source_type).is_none() {
		return (
			StatusCode::BAD_REQUEST,
			[("www-authenticate", "")],
			Json(serde_json::json!({ "error": format!("unknown source type: {}", source_type) })),
		);
	}

	// Check if a re-enrich job is already running for this source type.
	{
		let jobs = state.reenrich_jobs.lock().await;
		if let Some(progress) = jobs.get(&source_type) {
			let total = progress.total.load(Ordering::Relaxed);
			let done = progress.done.load(Ordering::Relaxed);
			if done < total {
				return (
					StatusCode::CONFLICT,
					[("www-authenticate", "")],
					Json(serde_json::json!({
						"status": "already_running",
						"source_type": source_type,
						"documents_total": total,
						"documents_done": done,
					})),
				);
			}
		}
	}

	// Count documents to process up front.
	let docs = match state.store.docs_for_reenrich(&source_type).await {
		Ok(d) => d,
		Err(e) => {
			return (
				StatusCode::INTERNAL_SERVER_ERROR,
				[("www-authenticate", "")],
				Json(serde_json::json!({ "error": e.to_string() })),
			)
		},
	};

	let total = docs.len() as u64;
	let progress =
		Arc::new(ReenrichProgress { total: AtomicU64::new(total), done: AtomicU64::new(0) });

	// Register the job.
	{
		let mut jobs = state.reenrich_jobs.lock().await;
		jobs.insert(source_type.clone(), Arc::clone(&progress));
	}

	// Spawn background task.
	let store = Arc::clone(&state.store);
	let jobs_map = Arc::clone(&state.reenrich_jobs);
	let st = source_type.clone();
	tokio::spawn(async move {
		let mut enriched = 0u64;
		for (doc_id, body, source_repo) in &docs {
			if let Some(body) = body {
				let output = bkb_ingest::enrichment::enrich(doc_id, body, source_repo.as_deref());

				if let Err(e) = store.delete_refs_from(doc_id).await {
					tracing::warn!(doc_id, error = %e, "failed to delete refs during re-enrich");
					progress.done.fetch_add(1, Ordering::Relaxed);
					continue;
				}
				for reference in &output.references {
					if let Err(e) = store.insert_reference(reference).await {
						tracing::warn!(doc_id, error = %e, "failed to insert ref during re-enrich");
					}
				}

				if let Err(e) = store.delete_concept_mentions(doc_id).await {
					tracing::warn!(doc_id, error = %e, "failed to delete concepts during re-enrich");
					progress.done.fetch_add(1, Ordering::Relaxed);
					continue;
				}
				for (slug, confidence) in &output.concept_tags {
					if let Err(e) = store.upsert_concept_mention(doc_id, slug, *confidence).await {
						tracing::warn!(doc_id, slug, error = %e, "failed to upsert concept during re-enrich");
					}
				}

				enriched += 1;
			}
			progress.done.fetch_add(1, Ordering::Relaxed);
		}

		tracing::info!(source_type = %st, total, enriched, "re-enrich complete");

		// Clean up the job entry after a short delay so the dashboard can
		// pick up the final state on its next auto-refresh.
		tokio::time::sleep(std::time::Duration::from_secs(60)).await;
		jobs_map.lock().await.remove(&st);
	});

	(
		StatusCode::ACCEPTED,
		[("www-authenticate", "")],
		Json(serde_json::json!({
			"status": "started",
			"source_type": source_type,
			"documents_total": total,
		})),
	)
}

async fn admin_reenrich_status(
	State(state): State<AppState>, headers: axum::http::HeaderMap, Path(source_type): Path<String>,
) -> impl IntoResponse {
	if let Err(status) = dashboard::check_admin_auth(&state, &headers) {
		return (
			status,
			[("www-authenticate", "Basic realm=\"BKB Admin\"")],
			Json(serde_json::json!({ "error": "unauthorized" })),
		);
	}

	let jobs = state.reenrich_jobs.lock().await;
	if let Some(progress) = jobs.get(&source_type) {
		let total = progress.total.load(Ordering::Relaxed);
		let done = progress.done.load(Ordering::Relaxed);
		let status = if done >= total { "complete" } else { "running" };
		(
			StatusCode::OK,
			[("www-authenticate", "")],
			Json(serde_json::json!({
				"status": status,
				"source_type": source_type,
				"documents_total": total,
				"documents_done": done,
			})),
		)
	} else {
		(
			StatusCode::NOT_FOUND,
			[("www-authenticate", "")],
			Json(serde_json::json!({ "status": "idle", "source_type": source_type })),
		)
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
