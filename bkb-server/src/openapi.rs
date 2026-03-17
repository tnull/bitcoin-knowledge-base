use axum::Json;

/// Handler for `GET /openapi.json` -- serves the OpenAPI 3.0.3 specification.
pub async fn openapi_spec() -> Json<serde_json::Value> {
	Json(spec())
}

fn spec() -> serde_json::Value {
	serde_json::json!({
		"openapi": "3.0.3",
		"info": {
			"title": "Bitcoin Knowledge Base (BKB)",
			"version": "0.1.0",
			"description": "Indexed knowledge from across the Bitcoin and Lightning development ecosystem. Search BIPs, BOLTs, bLIPs, LUDs, NUTs, GitHub issues/PRs/commits, mailing lists, IRC logs, Delving Bitcoin, BitcoinTalk, and Optech newsletters."
		},
		"servers": [
			{ "url": "https://bitcoinknowledge.dev" }
		],
		"paths": {
			"/search": search_path(),
			"/document/{id}": document_path(),
			"/references/{entity}": references_path(),
			"/bip/{number}": spec_path("BIP", "bip", "Bitcoin Improvement Proposal"),
			"/bolt/{number}": spec_path("BOLT", "bolt", "Lightning Network specification"),
			"/blip/{number}": spec_path("bLIP", "blip", "Bitcoin Lightning Improvement Proposal"),
			"/lud/{number}": spec_path("LUD", "lud", "LNURL Document"),
			"/nut/{number}": spec_path("NUT", "nut", "Cashu protocol specification"),
			"/timeline/{concept}": timeline_path(),
			"/find_commit": find_commit_path(),
		},
		"components": {
			"schemas": schemas()
		}
	})
}

fn search_path() -> serde_json::Value {
	serde_json::json!({
		"get": {
			"operationId": "search",
			"summary": "Full-text search across all indexed sources",
			"description": "Search the knowledge base using full-text queries. Supports filtering by source type, repository, author, and date range. Use query \"*\" with at least one filter to retrieve all matching documents.",
			"parameters": [
				{
					"name": "q",
					"in": "query",
					"required": true,
					"description": "Search query. Use \"*\" for wildcard (must combine with at least one filter).",
					"schema": { "type": "string" }
				},
				{
					"name": "source_type",
					"in": "query",
					"description": "Comma-separated source types to filter by.",
					"schema": { "type": "string", "enum": source_type_enum() }
				},
				{
					"name": "source_repo",
					"in": "query",
					"description": "Comma-separated GitHub repositories to filter by (e.g. \"bitcoin/bitcoin,lightningdevkit/rust-lightning\").",
					"schema": { "type": "string" }
				},
				{
					"name": "author",
					"in": "query",
					"description": "Filter by author name.",
					"schema": { "type": "string" }
				},
				{
					"name": "after",
					"in": "query",
					"description": "Only return documents created after this date (ISO 8601, e.g. \"2023-01-01\").",
					"schema": { "type": "string", "format": "date" }
				},
				{
					"name": "before",
					"in": "query",
					"description": "Only return documents created before this date (ISO 8601).",
					"schema": { "type": "string", "format": "date" }
				},
				{
					"name": "limit",
					"in": "query",
					"description": "Maximum number of results (default 20).",
					"schema": { "type": "integer", "default": 20 }
				}
			],
			"responses": {
				"200": {
					"description": "Search results with relevance scores.",
					"content": {
						"application/json": {
							"schema": { "$ref": "#/components/schemas/SearchResults" }
						}
					}
				}
			}
		}
	})
}

fn document_path() -> serde_json::Value {
	serde_json::json!({
		"get": {
			"operationId": "getDocument",
			"summary": "Get a document by ID",
			"description": "Retrieve the full content of a document including its body text, cross-references, and concept tags. Document IDs have the format \"source_type:source_id\" or \"source_type:owner/repo:source_id\".",
			"parameters": [
				{
					"name": "id",
					"in": "path",
					"required": true,
					"description": "The document ID (e.g. \"bip:340\", \"github_issue:bitcoin/bitcoin:12345\").",
					"schema": { "type": "string" }
				}
			],
			"responses": {
				"200": {
					"description": "Full document with references and concepts.",
					"content": {
						"application/json": {
							"schema": { "$ref": "#/components/schemas/DocumentContext" }
						}
					}
				},
				"404": {
					"description": "Document not found.",
					"content": {
						"application/json": {
							"schema": { "$ref": "#/components/schemas/Error" }
						}
					}
				}
			}
		}
	})
}

fn references_path() -> serde_json::Value {
	serde_json::json!({
		"get": {
			"operationId": "getReferences",
			"summary": "Find cross-references to an entity",
			"description": "Find all documents that reference a given entity. Entities can be specs (e.g. \"BIP-340\", \"BOLT-11\") or GitHub items (e.g. \"bitcoin/bitcoin#12345\").",
			"parameters": [
				{
					"name": "entity",
					"in": "path",
					"required": true,
					"description": "The entity to find references for (e.g. \"BIP-340\", \"BOLT-2\", \"bitcoin/bitcoin#1234\").",
					"schema": { "type": "string" }
				},
				{
					"name": "ref_type",
					"in": "query",
					"description": "Filter by reference type.",
					"schema": {
						"type": "string",
						"enum": [
							"mentions_issue", "mentions_pr", "fixes",
							"references_commit", "references_bip", "references_bolt",
							"references_blip", "references_lud", "references_nut",
							"replies_to"
						]
					}
				},
				{
					"name": "limit",
					"in": "query",
					"description": "Maximum number of references to return (default 50).",
					"schema": { "type": "integer", "default": 50 }
				}
			],
			"responses": {
				"200": {
					"description": "List of cross-references.",
					"content": {
						"application/json": {
							"schema": {
								"type": "array",
								"items": { "$ref": "#/components/schemas/Reference" }
							}
						}
					}
				}
			}
		}
	})
}

fn spec_path(name: &str, tag: &str, description: &str) -> serde_json::Value {
	serde_json::json!({
		"get": {
			"operationId": format!("lookup{}", name.replace('-', "")),
			"summary": format!("Look up a {} by number", name),
			"description": format!("Retrieve a {} ({}) with its full content, all incoming cross-references from other documents, and concept tags.", name, description),
			"parameters": [
				{
					"name": "number",
					"in": "path",
					"required": true,
					"description": format!("The {} number (e.g. 340 for {}-340).", name, tag.to_uppercase()),
					"schema": { "type": "integer" }
				}
			],
			"responses": {
				"200": {
					"description": format!("{} document with references and concepts.", name),
					"content": {
						"application/json": {
							"schema": { "$ref": "#/components/schemas/DocumentContext" }
						}
					}
				},
				"404": {
					"description": format!("{} not found.", name),
					"content": {
						"application/json": {
							"schema": { "$ref": "#/components/schemas/Error" }
						}
					}
				}
			}
		}
	})
}

fn timeline_path() -> serde_json::Value {
	serde_json::json!({
		"get": {
			"operationId": "getTimeline",
			"summary": "Get a chronological timeline of a concept",
			"description": "Retrieve a chronological timeline of events related to a Bitcoin/Lightning concept across all sources. Concept slugs include: taproot, schnorr, musig2, htlc, ptlc, covenants, silent-payments, payjoin, miniscript, psbt, channel-jamming, splicing, trampoline-routing, bolt12, and many more.",
			"parameters": [
				{
					"name": "concept",
					"in": "path",
					"required": true,
					"description": "The concept slug (e.g. \"taproot\", \"channel-jamming\", \"silent-payments\").",
					"schema": { "type": "string" }
				},
				{
					"name": "after",
					"in": "query",
					"description": "Only include events after this date (ISO 8601).",
					"schema": { "type": "string", "format": "date" }
				},
				{
					"name": "before",
					"in": "query",
					"description": "Only include events before this date (ISO 8601).",
					"schema": { "type": "string", "format": "date" }
				}
			],
			"responses": {
				"200": {
					"description": "Chronological timeline of events.",
					"content": {
						"application/json": {
							"schema": { "$ref": "#/components/schemas/Timeline" }
						}
					}
				}
			}
		}
	})
}

fn find_commit_path() -> serde_json::Value {
	serde_json::json!({
		"get": {
			"operationId": "findCommit",
			"summary": "Find commits matching a description",
			"description": "Search for git commits and associated pull requests matching a text query. Optionally filter by repository.",
			"parameters": [
				{
					"name": "q",
					"in": "query",
					"required": true,
					"description": "Search query describing the commit.",
					"schema": { "type": "string" }
				},
				{
					"name": "repo",
					"in": "query",
					"description": "Filter by repository (e.g. \"bitcoin/bitcoin\").",
					"schema": { "type": "string" }
				}
			],
			"responses": {
				"200": {
					"description": "Matching commits with associated PRs.",
					"content": {
						"application/json": {
							"schema": {
								"type": "array",
								"items": { "$ref": "#/components/schemas/CommitContext" }
							}
						}
					}
				}
			}
		}
	})
}

fn source_type_enum() -> serde_json::Value {
	serde_json::json!([
		"github_issue",
		"github_pr",
		"github_comment",
		"github_review",
		"github_review_comment",
		"github_discussion",
		"github_discussion_comment",
		"commit",
		"mailing_list_msg",
		"irc_log",
		"delving_topic",
		"delving_post",
		"bip",
		"bolt",
		"blip",
		"lud",
		"nut",
		"optech_newsletter",
		"optech_topic",
		"optech_blog",
		"bitcointalk_topic",
		"bitcointalk_post"
	])
}

fn schemas() -> serde_json::Value {
	serde_json::json!({
		"SearchResults": {
			"type": "object",
			"properties": {
				"results": {
					"type": "array",
					"items": { "$ref": "#/components/schemas/SearchResult" }
				},
				"total_count": {
					"type": "integer",
					"description": "Total number of matching documents."
				}
			}
		},
		"SearchResult": {
			"type": "object",
			"properties": {
				"id": { "type": "string", "description": "Canonical document ID." },
				"source_type": { "type": "string", "description": "The source type." },
				"source_repo": { "type": "string", "nullable": true, "description": "GitHub repository (owner/repo) if applicable." },
				"title": { "type": "string", "nullable": true },
				"snippet": { "type": "string", "nullable": true, "description": "FTS excerpt with matching terms highlighted." },
				"author": { "type": "string", "nullable": true },
				"created_at": { "type": "string", "format": "date-time" },
				"score": { "type": "number", "description": "Relevance score." },
				"url": { "type": "string", "nullable": true, "description": "Canonical URL for this document." },
				"concepts": {
					"type": "array",
					"items": { "type": "string" },
					"description": "Bitcoin/Lightning concept tags."
				}
			}
		},
		"DocumentContext": {
			"type": "object",
			"properties": {
				"document": { "$ref": "#/components/schemas/Document" },
				"url": { "type": "string", "nullable": true, "description": "Canonical URL." },
				"outgoing_refs": {
					"type": "array",
					"items": { "$ref": "#/components/schemas/Reference" },
					"description": "References from this document to other entities."
				},
				"incoming_refs": {
					"type": "array",
					"items": { "$ref": "#/components/schemas/Reference" },
					"description": "References from other documents to this one."
				},
				"concepts": {
					"type": "array",
					"items": { "type": "string" },
					"description": "Bitcoin/Lightning concept tags."
				}
			}
		},
		"Document": {
			"type": "object",
			"properties": {
				"id": { "type": "string" },
				"source_type": { "type": "string" },
				"source_repo": { "type": "string", "nullable": true },
				"source_id": { "type": "string" },
				"title": { "type": "string", "nullable": true },
				"body": { "type": "string", "nullable": true, "description": "Full document body text." },
				"author": { "type": "string", "nullable": true },
				"author_id": { "type": "string", "nullable": true },
				"created_at": { "type": "string", "format": "date-time" },
				"updated_at": { "type": "string", "format": "date-time", "nullable": true },
				"parent_id": { "type": "string", "nullable": true },
				"metadata": { "type": "object", "nullable": true }
			}
		},
		"Reference": {
			"type": "object",
			"properties": {
				"id": { "type": "integer", "nullable": true },
				"from_doc_id": { "type": "string", "description": "ID of the document containing the reference." },
				"to_doc_id": { "type": "string", "nullable": true, "description": "ID of the referenced document (if it exists in the knowledge base)." },
				"ref_type": {
					"type": "string",
					"description": "Type of reference.",
					"enum": [
						"mentions_issue", "mentions_pr", "fixes",
						"references_commit", "references_bip", "references_bolt",
						"references_blip", "references_lud", "references_nut",
						"replies_to"
					]
				},
				"to_external": { "type": "string", "nullable": true, "description": "External entity identifier (e.g. \"BIP-340\")." },
				"context": { "type": "string", "nullable": true }
			}
		},
		"Timeline": {
			"type": "object",
			"properties": {
				"concept": { "type": "string", "description": "The concept slug." },
				"events": {
					"type": "array",
					"items": { "$ref": "#/components/schemas/TimelineEvent" }
				}
			}
		},
		"TimelineEvent": {
			"type": "object",
			"properties": {
				"date": { "type": "string", "description": "Event date." },
				"type": { "type": "string", "description": "Source type of the event." },
				"title": { "type": "string", "nullable": true },
				"id": { "type": "string", "description": "Document ID." },
				"url": { "type": "string", "nullable": true }
			}
		},
		"CommitContext": {
			"type": "object",
			"properties": {
				"document": { "$ref": "#/components/schemas/Document" },
				"url": { "type": "string", "nullable": true },
				"associated_prs": {
					"type": "array",
					"items": { "$ref": "#/components/schemas/SearchResult" },
					"description": "Pull requests associated with this commit."
				}
			}
		},
		"Error": {
			"type": "object",
			"properties": {
				"error": { "type": "string", "description": "Error message." }
			}
		}
	})
}
