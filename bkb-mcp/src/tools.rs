use std::io::BufRead;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tracing::{debug, error};

use bkb_core::model::{parse_datetime, SearchParams, SourceType};
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

		// JSON-RPC notifications have no `id` and must not receive a response.
		if request.id.is_none() {
			continue;
		}

		let response = handle_request(&store, &request).await;
		write_response(&mut stdout, &response)?;
	}

	Ok(())
}

async fn handle_request(store: &impl KnowledgeStore, request: &JsonRpcRequest) -> JsonRpcResponse {
	// Callers ensure `id` is always `Some` (notifications are filtered earlier).
	let id = request.id.clone().unwrap_or(serde_json::Value::Null);

	match request.method.as_str() {
		"initialize" => JsonRpcResponse {
			jsonrpc: "2.0".to_string(),
			id,
			result: Some(serde_json::json!({
				"protocolVersion": "2024-11-05",
				"capabilities": {
					"tools": { "listChanged": false }
				},
				"serverInfo": {
					"name": "bkb-mcp",
					"version": env!("CARGO_PKG_VERSION")
				}
			})),
			error: None,
		},
		"tools/list" => {
			let tools = tool_definitions();
			JsonRpcResponse {
				jsonrpc: "2.0".to_string(),
				id,
				result: Some(serde_json::json!({ "tools": tools })),
				error: None,
			}
		},
		"tools/call" => handle_tool_call(store, request).await,
		_ => JsonRpcResponse {
			jsonrpc: "2.0".to_string(),
			id,
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
	let id = request.id.clone().unwrap_or(serde_json::Value::Null);
	let params = request.params.as_ref();
	let tool_name = params.and_then(|p| p.get("name")).and_then(|n| n.as_str()).unwrap_or("");
	let arguments = params
		.and_then(|p| p.get("arguments"))
		.cloned()
		.unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

	let result = match tool_name {
		"bkb_search" => tool_search(store, &arguments).await,
		"bkb_get_document" => tool_get_document(store, &arguments).await,
		"bkb_get_references" => tool_get_references(store, &arguments).await,
		"bkb_lookup_bip" => tool_lookup_bip(store, &arguments).await,
		"bkb_lookup_bolt" => tool_lookup_bolt(store, &arguments).await,
		"bkb_lookup_blip" => tool_lookup_blip(store, &arguments).await,
		"bkb_lookup_lud" => tool_lookup_lud(store, &arguments).await,
		"bkb_lookup_nut" => tool_lookup_nut(store, &arguments).await,
		"bkb_timeline" => tool_timeline(store, &arguments).await,
		"bkb_find_commit" => tool_find_commit(store, &arguments).await,
		_ => Err(anyhow::anyhow!("unknown tool: {}", tool_name)),
	};

	match result {
		Ok(content) => JsonRpcResponse {
			jsonrpc: "2.0".to_string(),
			id,
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
				id,
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
		deduplicate: args.get("deduplicate").and_then(|v| v.as_bool()),
		query,
		source_type,
		source_repo,
		author: args.get("author").and_then(|v| v.as_str()).map(|s| s.to_string()),
		after: args.get("after").and_then(|v| v.as_str()).and_then(parse_datetime),
		before: args.get("before").and_then(|v| v.as_str()).and_then(parse_datetime),
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

	let result = if args.get("raw").and_then(|v| v.as_bool()).unwrap_or(false) {
		store.get_raw_document(id).await?
	} else {
		store.get_document(id).await?
	};
	match result {
		Some(ctx) => Ok(serde_json::to_string_pretty(&ctx)?),
		None => Ok(format!("Document not found: {}", id)),
	}
}

async fn tool_get_references(
	store: &impl KnowledgeStore, args: &serde_json::Value,
) -> Result<String> {
	let entity = args
		.get("entity")
		.and_then(|v| v.as_str())
		.ok_or_else(|| anyhow::anyhow!("missing required parameter: entity"))?;
	let ref_type = args.get("ref_type").and_then(|v| v.as_str());
	let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as u32;

	let refs = store.get_references(entity, ref_type, limit).await?;
	Ok(serde_json::to_string_pretty(&refs)?)
}

async fn tool_lookup_bip(store: &impl KnowledgeStore, args: &serde_json::Value) -> Result<String> {
	let number = args
		.get("number")
		.and_then(|v| v.as_u64())
		.ok_or_else(|| anyhow::anyhow!("missing required parameter: number"))? as u32;

	match store.lookup_bip(number).await? {
		Some(ctx) => Ok(serde_json::to_string_pretty(&ctx)?),
		None => Ok(format!("BIP-{} not found", number)),
	}
}

async fn tool_lookup_bolt(store: &impl KnowledgeStore, args: &serde_json::Value) -> Result<String> {
	let number = args
		.get("number")
		.and_then(|v| v.as_u64())
		.ok_or_else(|| anyhow::anyhow!("missing required parameter: number"))? as u32;

	match store.lookup_bolt(number).await? {
		Some(ctx) => Ok(serde_json::to_string_pretty(&ctx)?),
		None => Ok(format!("BOLT-{} not found", number)),
	}
}

async fn tool_lookup_blip(store: &impl KnowledgeStore, args: &serde_json::Value) -> Result<String> {
	let number = args
		.get("number")
		.and_then(|v| v.as_u64())
		.ok_or_else(|| anyhow::anyhow!("missing required parameter: number"))? as u32;

	match store.lookup_blip(number).await? {
		Some(ctx) => Ok(serde_json::to_string_pretty(&ctx)?),
		None => Ok(format!("bLIP-{} not found", number)),
	}
}

async fn tool_lookup_lud(store: &impl KnowledgeStore, args: &serde_json::Value) -> Result<String> {
	let number = args
		.get("number")
		.and_then(|v| v.as_u64())
		.ok_or_else(|| anyhow::anyhow!("missing required parameter: number"))? as u32;

	match store.lookup_lud(number).await? {
		Some(ctx) => Ok(serde_json::to_string_pretty(&ctx)?),
		None => Ok(format!("LUD-{} not found", number)),
	}
}

async fn tool_lookup_nut(store: &impl KnowledgeStore, args: &serde_json::Value) -> Result<String> {
	let number = args
		.get("number")
		.and_then(|v| v.as_u64())
		.ok_or_else(|| anyhow::anyhow!("missing required parameter: number"))? as u32;

	match store.lookup_nut(number).await? {
		Some(ctx) => Ok(serde_json::to_string_pretty(&ctx)?),
		None => Ok(format!("NUT-{} not found", number)),
	}
}

async fn tool_timeline(store: &impl KnowledgeStore, args: &serde_json::Value) -> Result<String> {
	let concept = args
		.get("concept")
		.and_then(|v| v.as_str())
		.ok_or_else(|| anyhow::anyhow!("missing required parameter: concept"))?;

	let after = args
		.get("after")
		.and_then(|v| v.as_str())
		.and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
		.map(|dt| dt.with_timezone(&chrono::Utc));

	let before = args
		.get("before")
		.and_then(|v| v.as_str())
		.and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
		.map(|dt| dt.with_timezone(&chrono::Utc));

	let timeline = store.timeline(concept, after, before).await?;
	Ok(serde_json::to_string_pretty(&timeline)?)
}

async fn tool_find_commit(store: &impl KnowledgeStore, args: &serde_json::Value) -> Result<String> {
	let query = args
		.get("query")
		.and_then(|v| v.as_str())
		.ok_or_else(|| anyhow::anyhow!("missing required parameter: query"))?;
	let repo = args.get("repo").and_then(|v| v.as_str());

	let results = store.find_commit(query, repo).await?;
	Ok(serde_json::to_string_pretty(&results)?)
}

fn tool_definitions() -> serde_json::Value {
	serde_json::json!([
		{
			"name": "bkb_search",
			"description": "Search the Bitcoin knowledge base across all sources (code, issues, PRs, mailing lists, IRC logs, Delving Bitcoin, BIPs, BOLTs, Optech). Returns matching documents with snippets. Use query \"*\" with at least one filter (source_type, source_repo, author, after, before) to list all matching documents without a text query -- e.g. all commits in a repo within a date range.",
			"inputSchema": {
				"type": "object",
				"properties": {
					"query": {
						"type": "string",
						"description": "Search query (use \"*\" to match all documents when filters are set)"
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
					"deduplicate": {
						"type": "boolean",
						"default": true,
						"description": "Collapse verified cross-forge copies; false returns original records"
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
					"raw": { "type": "boolean", "default": false, "description": "Return this exact source record without resolving its canonical alias" },
					"id": {
						"type": "string",
						"description": "Document ID (e.g. github_issue:bitcoin/bitcoin:12345)"
					}
				},
				"required": ["id"]
			}
		},
		{
			"name": "bkb_get_references",
			"description": "Find all documents referencing a given entity (BIP, BOLT, issue, commit, or concept).",
			"inputSchema": {
				"type": "object",
				"properties": {
					"entity": {
						"type": "string",
						"description": "Entity to look up (e.g. 'BIP-340', 'bitcoin/bitcoin#12345', commit SHA, or concept slug)"
					},
					"ref_type": {
						"type": "string",
						"description": "Filter by reference type (e.g. 'references_bip', 'fixes')"
					},
					"limit": {
						"type": "integer",
						"description": "Max results (default 50)"
					}
				},
				"required": ["entity"]
			}
		},
		{
			"name": "bkb_lookup_bip",
			"description": "Get comprehensive context for a BIP: spec text, all referencing discussions, PRs, and Optech summary.",
			"inputSchema": {
				"type": "object",
				"properties": {
					"number": {
						"type": "integer",
						"description": "BIP number (e.g. 340)"
					}
				},
				"required": ["number"]
			}
		},
		{
			"name": "bkb_lookup_bolt",
			"description": "Get comprehensive context for a BOLT: spec text, all referencing discussions, PRs, and Optech summary.",
			"inputSchema": {
				"type": "object",
				"properties": {
					"number": {
						"type": "integer",
						"description": "BOLT number (e.g. 11)"
					}
				},
				"required": ["number"]
			}
		},
		{
			"name": "bkb_lookup_blip",
			"description": "Get comprehensive context for a bLIP (Bitcoin Lightning Improvement Proposal): spec text, all referencing discussions, PRs, and related documents.",
			"inputSchema": {
				"type": "object",
				"properties": {
					"number": {
						"type": "integer",
						"description": "bLIP number (e.g. 1)"
					}
				},
				"required": ["number"]
			}
		},
		{
			"name": "bkb_lookup_lud",
			"description": "Get comprehensive context for a LUD (LNURL Document): spec text, all referencing discussions, PRs, and related documents.",
			"inputSchema": {
				"type": "object",
				"properties": {
					"number": {
						"type": "integer",
						"description": "LUD number (e.g. 6)"
					}
				},
				"required": ["number"]
			}
		},
		{
			"name": "bkb_lookup_nut",
			"description": "Get comprehensive context for a NUT (Cashu protocol specification): spec text, all referencing discussions, PRs, and related documents.",
			"inputSchema": {
				"type": "object",
				"properties": {
					"number": {
						"type": "integer",
						"description": "NUT number (e.g. 0)"
					}
				},
				"required": ["number"]
			}
		},
		{
			"name": "bkb_timeline",
			"description": "Chronological timeline of a concept across all sources: mailing list proposals, BIPs, implementation PRs, Optech coverage.",
			"inputSchema": {
				"type": "object",
				"properties": {
					"concept": {
						"type": "string",
						"description": "Concept slug or search term (e.g. 'taproot', 'package-relay')"
					},
					"after": {
						"type": "string",
						"description": "Start date (ISO 8601)"
					},
					"before": {
						"type": "string",
						"description": "End date (ISO 8601)"
					}
				},
				"required": ["concept"]
			}
		},
		{
			"name": "bkb_find_commit",
			"description": "Find which commit(s) introduced a change, with the associated PR and discussion context.",
			"inputSchema": {
				"type": "object",
				"properties": {
					"query": {
						"type": "string",
						"description": "Description of change, function name, or code pattern"
					},
					"repo": {
						"type": "string",
						"description": "Limit to a specific repo (e.g. 'bitcoin/bitcoin')"
					}
				},
				"required": ["query"]
			}
		}
	])
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn initialize_reports_package_version() {
		let store = crate::remote_store::RemoteApiStore::new("http://127.0.0.1:1");
		let request = JsonRpcRequest {
			jsonrpc: "2.0".into(),
			id: Some(serde_json::json!(1)),
			method: "initialize".into(),
			params: None,
		};
		let response = handle_request(&store, &request).await;
		assert!(response.error.is_none());
		assert_eq!(
			response.result.unwrap()["serverInfo"]["version"],
			env!("CARGO_PKG_VERSION"),
			"MCP initialization must identify the installed package version"
		);
	}
}

#[cfg(test)]
mod cross_forge_tests {
	use super::*;
	use tokio::io::{AsyncReadExt, AsyncWriteExt};
	#[tokio::test]
	async fn mcp_forwards_raw_and_deduplication_options() {
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let addr = listener.local_addr().unwrap();
		let handle = tokio::spawn(async move {
			let mut requests = vec![];
			for (status, body) in
				[("200 OK", r#"{"results":[],"total_count":0}"#), ("404 Not Found", "{}")]
			{
				let (mut socket, _) = listener.accept().await.unwrap();
				let mut buf = [0; 8192];
				let n = socket.read(&mut buf).await.unwrap();
				requests.push(String::from_utf8_lossy(&buf[..n]).into_owned());
				socket.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
			}
			requests
		});
		let store = crate::remote_store::RemoteApiStore::new(&format!("http://{addr}"));
		tool_search(&store,&serde_json::json!({"query":"Electrum","source_repo":"git.rust-bitcoin.org/lightningdevkit/rust-lightning","deduplicate":false})).await.unwrap();
		tool_get_document(&store,&serde_json::json!({"id":"forgejo_issue:git.rust-bitcoin.org/lightningdevkit/rust-lightning:4866","raw":true})).await.unwrap();
		let requests = handle.await.unwrap();
		assert!(requests[0].contains("deduplicate=false"));
		assert!(requests[0].contains("git.rust-bitcoin.org/lightningdevkit/rust-lightning"));
		assert!(requests[1].contains("?raw=true"));
		let defs = tool_definitions();
		assert_eq!(defs[0]["inputSchema"]["properties"]["deduplicate"]["default"], true);
		assert_eq!(defs[1]["inputSchema"]["properties"]["raw"]["default"], false);
	}
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
	id: Option<serde_json::Value>,
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
