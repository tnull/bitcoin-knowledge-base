use std::io::BufRead;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tracing::{debug, error};

use bkb_core::model::{SearchParams, SourceType};
use bkb_core::store::KnowledgeStore;

/// Run the MCP server over stdio (JSON-RPC 2.0).
///
/// Reads JSON-RPC requests from stdin, dispatches to tool handlers,
/// and writes responses to stdout. Logs go to stderr.
pub async fn run_stdio_server(store: impl KnowledgeStore + 'static) -> Result<()> {
	let stdin = std::io::stdin();
	let mut stdout = std::io::stdout();

	for line in stdin.lock().lines() {
		let line = line?;
		if line.trim().is_empty() {
			continue;
		}

		debug!(request = %line, "received JSON-RPC request");

		let request: JsonRpcRequest = match serde_json::from_str(&line) {
			Ok(r) => r,
			Err(e) => {
				let error_response = JsonRpcResponse {
					jsonrpc: "2.0".to_string(),
					id: serde_json::Value::Null,
					result: None,
					error: Some(JsonRpcError {
						code: -32700,
						message: format!("parse error: {}", e),
						data: None,
					}),
				};
				write_response(&mut stdout, &error_response)?;
				continue;
			},
		};

		let response = handle_request(&store, &request).await;
		write_response(&mut stdout, &response)?;
	}

	Ok(())
}

async fn handle_request(store: &impl KnowledgeStore, request: &JsonRpcRequest) -> JsonRpcResponse {
	match request.method.as_str() {
		"initialize" => JsonRpcResponse {
			jsonrpc: "2.0".to_string(),
			id: request.id.clone(),
			result: Some(serde_json::json!({
				"protocolVersion": "2024-11-05",
				"capabilities": {
					"tools": { "listChanged": false }
				},
				"serverInfo": {
					"name": "bkb-mcp",
					"version": "0.1.0"
				}
			})),
			error: None,
		},
		"notifications/initialized" => {
			// Client acknowledgment, no response needed for notifications
			// but we still return one since our loop expects it
			JsonRpcResponse {
				jsonrpc: "2.0".to_string(),
				id: request.id.clone(),
				result: Some(serde_json::Value::Null),
				error: None,
			}
		},
		"tools/list" => {
			let tools = tool_definitions();
			JsonRpcResponse {
				jsonrpc: "2.0".to_string(),
				id: request.id.clone(),
				result: Some(serde_json::json!({ "tools": tools })),
				error: None,
			}
		},
		"tools/call" => handle_tool_call(store, request).await,
		_ => JsonRpcResponse {
			jsonrpc: "2.0".to_string(),
			id: request.id.clone(),
			result: None,
			error: Some(JsonRpcError {
				code: -32601,
				message: format!("method not found: {}", request.method),
				data: None,
			}),
		},
	}
}

async fn handle_tool_call(
	store: &impl KnowledgeStore, request: &JsonRpcRequest,
) -> JsonRpcResponse {
	let params = request.params.as_ref();
	let tool_name = params.and_then(|p| p.get("name")).and_then(|n| n.as_str()).unwrap_or("");
	let arguments = params
		.and_then(|p| p.get("arguments"))
		.cloned()
		.unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

	let result = match tool_name {
		"bkb_search" => tool_search(store, &arguments).await,
		"bkb_get_document" => tool_get_document(store, &arguments).await,
		_ => Err(anyhow::anyhow!("unknown tool: {}", tool_name)),
	};

	match result {
		Ok(content) => JsonRpcResponse {
			jsonrpc: "2.0".to_string(),
			id: request.id.clone(),
			result: Some(serde_json::json!({
				"content": [{
					"type": "text",
					"text": content
				}]
			})),
			error: None,
		},
		Err(e) => {
			error!(tool = tool_name, error = %e, "tool call failed");
			JsonRpcResponse {
				jsonrpc: "2.0".to_string(),
				id: request.id.clone(),
				result: Some(serde_json::json!({
					"content": [{
						"type": "text",
						"text": format!("Error: {}", e)
					}],
					"isError": true
				})),
				error: None,
			}
		},
	}
}

async fn tool_search(store: &impl KnowledgeStore, args: &serde_json::Value) -> Result<String> {
	let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("").to_string();

	let source_type = args
		.get("source_type")
		.and_then(|v| v.as_str())
		.map(|s| s.split(',').filter_map(|t| SourceType::from_str(t.trim())).collect());

	let source_repo = args
		.get("source_repo")
		.and_then(|v| v.as_str())
		.map(|s| s.split(',').map(|r| r.trim().to_string()).collect());

	let limit = args.get("limit").and_then(|v| v.as_u64()).map(|n| n as u32);

	let params = SearchParams {
		query,
		source_type,
		source_repo,
		author: args.get("author").and_then(|v| v.as_str()).map(|s| s.to_string()),
		after: args
			.get("after")
			.and_then(|v| v.as_str())
			.and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
			.map(|dt| dt.with_timezone(&chrono::Utc)),
		before: args
			.get("before")
			.and_then(|v| v.as_str())
			.and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
			.map(|dt| dt.with_timezone(&chrono::Utc)),
		semantic: args.get("semantic").and_then(|v| v.as_bool()).unwrap_or(false),
		limit,
	};

	let results = store.search(params).await?;
	Ok(serde_json::to_string_pretty(&results)?)
}

async fn tool_get_document(
	store: &impl KnowledgeStore, args: &serde_json::Value,
) -> Result<String> {
	let id = args
		.get("id")
		.and_then(|v| v.as_str())
		.ok_or_else(|| anyhow::anyhow!("missing required parameter: id"))?;

	match store.get_document(id).await? {
		Some(ctx) => Ok(serde_json::to_string_pretty(&ctx)?),
		None => Ok(format!("Document not found: {}", id)),
	}
}

fn tool_definitions() -> serde_json::Value {
	serde_json::json!([
		{
			"name": "bkb_search",
			"description": "Search the Bitcoin knowledge base across all sources (code, issues, PRs, mailing lists, IRC logs, Delving Bitcoin, BIPs, BOLTs, Optech). Returns matching documents with snippets.",
			"inputSchema": {
				"type": "object",
				"properties": {
					"query": {
						"type": "string",
						"description": "Search query"
					},
					"source_type": {
						"type": "string",
						"description": "Comma-separated filter by source type (e.g. github_issue,github_pr)"
					},
					"source_repo": {
						"type": "string",
						"description": "Comma-separated filter by repository (e.g. bitcoin/bitcoin)"
					},
					"author": {
						"type": "string",
						"description": "Filter by author"
					},
					"after": {
						"type": "string",
						"description": "Created after (ISO 8601)"
					},
					"before": {
						"type": "string",
						"description": "Created before (ISO 8601)"
					},
					"semantic": {
						"type": "boolean",
						"description": "Enable embedding similarity search"
					},
					"limit": {
						"type": "integer",
						"description": "Max results (default 20)"
					}
				},
				"required": ["query"]
			}
		},
		{
			"name": "bkb_get_document",
			"description": "Get full document by ID including content, cross-references, and related concepts.",
			"inputSchema": {
				"type": "object",
				"properties": {
					"id": {
						"type": "string",
						"description": "Document ID (e.g. github_issue:bitcoin/bitcoin:12345)"
					}
				},
				"required": ["id"]
			}
		}
	])
}

fn write_response(stdout: &mut std::io::Stdout, response: &JsonRpcResponse) -> Result<()> {
	use std::io::Write;
	let json = serde_json::to_string(response)?;
	writeln!(stdout, "{}", json)?;
	stdout.flush()?;
	Ok(())
}

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
	#[allow(dead_code)]
	jsonrpc: String,
	id: serde_json::Value,
	method: String,
	params: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
	jsonrpc: String,
	id: serde_json::Value,
	#[serde(skip_serializing_if = "Option::is_none")]
	result: Option<serde_json::Value>,
	#[serde(skip_serializing_if = "Option::is_none")]
	error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
	code: i32,
	message: String,
	#[serde(skip_serializing_if = "Option::is_none")]
	data: Option<serde_json::Value>,
}
