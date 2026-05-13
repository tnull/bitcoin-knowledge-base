#!/usr/bin/env python3
"""Small stdio MCP proxy for https://bitcoinknowledge.dev.

The upstream repository ships a Rust `bkb-mcp` crate. This Python proxy keeps
the Codex install usable even when Cargo is not available in the local PATH.
"""

from __future__ import annotations

import json
import os
import sys
import urllib.error
import urllib.parse
import urllib.request
from typing import Any


API_URL = os.environ.get("BKB_API_URL", "https://bitcoinknowledge.dev").rstrip("/")
SERVER_INFO = {"name": "bkb-mcp-python-proxy", "version": "0.2.1"}


def _url(path: str, query: dict[str, Any] | None = None) -> str:
    url = f"{API_URL}{path}"
    if query:
        clean = {
            key: value
            for key, value in query.items()
            if value is not None and value != ""
        }
        if clean:
            url = f"{url}?{urllib.parse.urlencode(clean)}"
    return url


def _get_json(url: str) -> Any:
    request = urllib.request.Request(url, headers={"Accept": "application/json"})
    try:
        with urllib.request.urlopen(request, timeout=60) as response:
            return json.loads(response.read().decode("utf-8"))
    except urllib.error.HTTPError as exc:
        body = exc.read().decode("utf-8", errors="replace")
        raise RuntimeError(f"BKB API returned HTTP {exc.code}: {body}") from exc


def _tool_text(payload: Any) -> dict[str, Any]:
    return {
        "content": [
            {
                "type": "text",
                "text": json.dumps(payload, ensure_ascii=False, indent=2),
            }
        ]
    }


def _tool_error(message: str) -> dict[str, Any]:
    return {"content": [{"type": "text", "text": f"Error: {message}"}], "isError": True}


def tool_definitions() -> list[dict[str, Any]]:
    return [
        {
            "name": "bkb_search",
            "description": (
                "Search the Bitcoin knowledge base across BIPs, BOLTs, GitHub, "
                "mailing lists, IRC logs, Delving Bitcoin, BitcoinTalk, and Optech."
            ),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string"},
                    "source_type": {"type": "string"},
                    "source_repo": {"type": "string"},
                    "author": {"type": "string"},
                    "after": {"type": "string"},
                    "before": {"type": "string"},
                    "semantic": {"type": "boolean"},
                    "limit": {"type": "integer"},
                },
                "required": ["query"],
            },
        },
        {
            "name": "bkb_get_document",
            "description": "Get a full BKB document by ID.",
            "inputSchema": {
                "type": "object",
                "properties": {"id": {"type": "string"}},
                "required": ["id"],
            },
        },
        {
            "name": "bkb_get_references",
            "description": "Find documents referencing an entity such as BIP-340.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "entity": {"type": "string"},
                    "ref_type": {"type": "string"},
                    "limit": {"type": "integer"},
                },
                "required": ["entity"],
            },
        },
        *_lookup_tool("bkb_lookup_bip", "BIP", "number"),
        *_lookup_tool("bkb_lookup_bolt", "BOLT", "number"),
        *_lookup_tool("bkb_lookup_blip", "bLIP", "number"),
        *_lookup_tool("bkb_lookup_lud", "LUD", "number"),
        *_lookup_tool("bkb_lookup_nut", "NUT", "number"),
        {
            "name": "bkb_timeline",
            "description": "Return a chronological timeline for a Bitcoin/Lightning concept.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "concept": {"type": "string"},
                    "after": {"type": "string"},
                    "before": {"type": "string"},
                },
                "required": ["concept"],
            },
        },
        {
            "name": "bkb_find_commit",
            "description": "Find commits and associated PR context matching a description.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string"},
                    "repo": {"type": "string"},
                },
                "required": ["query"],
            },
        },
    ]


def _lookup_tool(name: str, label: str, field: str) -> list[dict[str, Any]]:
    return [
        {
            "name": name,
            "description": f"Look up {label} context and related BKB documents.",
            "inputSchema": {
                "type": "object",
                "properties": {field: {"type": "integer"}},
                "required": [field],
            },
        }
    ]


def call_tool(name: str, args: dict[str, Any]) -> dict[str, Any]:
    try:
        if name == "bkb_search":
            query = args.get("query")
            if not query:
                raise ValueError("missing required parameter: query")
            payload = _get_json(
                _url(
                    "/search",
                    {
                        "q": query,
                        "source_type": args.get("source_type"),
                        "source_repo": args.get("source_repo"),
                        "author": args.get("author"),
                        "after": args.get("after"),
                        "before": args.get("before"),
                        "semantic": "true" if args.get("semantic") else None,
                        "limit": args.get("limit"),
                    },
                )
            )
        elif name == "bkb_get_document":
            doc_id = args.get("id")
            if not doc_id:
                raise ValueError("missing required parameter: id")
            payload = _get_json(f"{API_URL}/document/{urllib.parse.quote(str(doc_id), safe='')}")
        elif name == "bkb_get_references":
            entity = args.get("entity")
            if not entity:
                raise ValueError("missing required parameter: entity")
            payload = _get_json(
                _url(
                    f"/references/{urllib.parse.quote(str(entity), safe='')}",
                    {"ref_type": args.get("ref_type"), "limit": args.get("limit")},
                )
            )
        elif name == "bkb_lookup_bip":
            payload = lookup_number(args, "number", "/bip")
        elif name == "bkb_lookup_bolt":
            payload = lookup_number(args, "number", "/bolt")
        elif name == "bkb_lookup_blip":
            payload = lookup_number(args, "number", "/blip")
        elif name == "bkb_lookup_lud":
            payload = lookup_number(args, "number", "/lud")
        elif name == "bkb_lookup_nut":
            payload = lookup_number(args, "number", "/nut")
        elif name == "bkb_timeline":
            concept = args.get("concept")
            if not concept:
                raise ValueError("missing required parameter: concept")
            payload = _get_json(
                _url(
                    f"/timeline/{urllib.parse.quote(str(concept), safe='')}",
                    {"after": args.get("after"), "before": args.get("before")},
                )
            )
        elif name == "bkb_find_commit":
            query = args.get("query")
            if not query:
                raise ValueError("missing required parameter: query")
            payload = _get_json(_url("/find_commit", {"q": query, "repo": args.get("repo")}))
        else:
            raise ValueError(f"unknown tool: {name}")
        return _tool_text(payload)
    except Exception as exc:
        return _tool_error(str(exc))


def lookup_number(args: dict[str, Any], field: str, path: str) -> Any:
    number = args.get(field)
    if number is None:
        raise ValueError(f"missing required parameter: {field}")
    return _get_json(f"{API_URL}{path}/{int(number)}")


def response(request_id: Any, result: Any = None, error: dict[str, Any] | None = None) -> dict[str, Any]:
    out = {"jsonrpc": "2.0", "id": request_id}
    if error is not None:
        out["error"] = error
    else:
        out["result"] = result
    return out


def handle(request: dict[str, Any]) -> dict[str, Any] | None:
    request_id = request.get("id")
    if request_id is None:
        return None

    method = request.get("method")
    if method == "initialize":
        return response(
            request_id,
            {
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {"listChanged": False}},
                "serverInfo": SERVER_INFO,
            },
        )
    if method == "tools/list":
        return response(request_id, {"tools": tool_definitions()})
    if method == "tools/call":
        params = request.get("params") or {}
        return response(
            request_id,
            call_tool(params.get("name", ""), params.get("arguments") or {}),
        )
    return response(
        request_id,
        error={"code": -32601, "message": f"method not found: {method}"},
    )


def main() -> int:
    for line in sys.stdin:
        if not line.strip():
            continue
        try:
            request = json.loads(line)
            out = handle(request)
        except json.JSONDecodeError as exc:
            out = response(None, error={"code": -32700, "message": f"parse error: {exc}"})
        if out is not None:
            sys.stdout.write(json.dumps(out, ensure_ascii=False) + "\n")
            sys.stdout.flush()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
