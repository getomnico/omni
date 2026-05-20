#!/usr/bin/env python3
"""Convenience CLI for Omni searcher /search calls.

Examples:
  python searcher_search.py "retention duration audit traces exports"
  python searcher_search.py --mode semantic --limit 20 "low bit math pass rate"
  python searcher_search.py --document-id 01KABC...
  python searcher_search.py --document-id 01KABC... --start-line 120 --end-line 180
  python searcher_search.py --document-id 01KABC... "latency vector drift risk score"
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import textwrap
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any


def _parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "query_parts",
        nargs="*",
        help="Query text. Optional when --document-id is provided.",
    )
    parser.add_argument(
        "--query",
        help="Query text. Overrides positional query text when provided.",
    )
    parser.add_argument(
        "--document-id",
        help="Internal Omni document id to read or search within.",
    )
    parser.add_argument(
        "--start-line",
        type=int,
        help="1-indexed inclusive start line for document reads.",
    )
    parser.add_argument(
        "--end-line",
        type=int,
        help="1-indexed inclusive end line for document reads.",
    )
    parser.add_argument(
        "--mode",
        choices=["fulltext", "semantic", "hybrid"],
        default="hybrid",
        help="Search mode.",
    )
    parser.add_argument("--limit", type=int, default=10, help="Number of results.")
    parser.add_argument("--offset", type=int, default=0, help="Result offset.")
    parser.add_argument(
        "--source-types",
        default="",
        help="Optional comma-separated source types, e.g. slack,jira.",
    )
    parser.add_argument(
        "--content-types",
        default="",
        help="Optional comma-separated content types.",
    )
    parser.add_argument("--user-email", help="Optional user email for permissions.")
    parser.add_argument("--user-id", help="Optional user id for permissions.")
    parser.add_argument(
        "--facets",
        action="store_true",
        help="Request facets. Disabled by default for faster investigation calls.",
    )
    parser.add_argument(
        "--raw",
        action="store_true",
        help="Print raw JSON response instead of a compact summary.",
    )
    parser.add_argument(
        "--save",
        type=Path,
        help="Write raw JSON response to this path.",
    )
    parser.add_argument(
        "--searcher-url",
        default=os.environ.get("BENCH_SEARCHER_URL", "http://localhost:3001"),
        help="Base URL for omni-searcher.",
    )
    parser.add_argument("--timeout", type=float, default=120.0, help="HTTP timeout.")
    return parser.parse_args()


def _csv(value: str) -> list[str]:
    return [item.strip() for item in value.split(",") if item.strip()]


def _build_body(args: argparse.Namespace) -> dict[str, Any]:
    query = args.query if args.query is not None else " ".join(args.query_parts)
    query = query.strip()
    if not query and not args.document_id:
        raise ValueError("provide a query or --document-id")
    if (
        args.start_line is not None or args.end_line is not None
    ) and not args.document_id:
        raise ValueError("--start-line/--end-line require --document-id")
    if args.start_line is not None and args.start_line < 1:
        raise ValueError("--start-line must be >= 1")
    if args.end_line is not None and args.end_line < 1:
        raise ValueError("--end-line must be >= 1")
    if (
        args.start_line is not None
        and args.end_line is not None
        and args.end_line < args.start_line
    ):
        raise ValueError("--end-line must be >= --start-line")

    body: dict[str, Any] = {
        "query": query,
        "mode": args.mode,
        "limit": args.limit,
        "offset": args.offset,
        "include_facets": args.facets,
    }
    if args.document_id:
        body["document_id"] = args.document_id
    if args.start_line is not None:
        body["document_content_start_line"] = args.start_line
    if args.end_line is not None:
        body["document_content_end_line"] = args.end_line
    if args.source_types:
        body["source_types"] = _csv(args.source_types)
    if args.content_types:
        body["content_types"] = _csv(args.content_types)
    if args.user_email:
        body["user_email"] = args.user_email
    if args.user_id:
        body["user_id"] = args.user_id
    return body


def _post_search(
    searcher_url: str, body: dict[str, Any], timeout: float
) -> dict[str, Any]:
    url = searcher_url.rstrip("/") + "/search"
    request = urllib.request.Request(
        url,
        data=json.dumps(body).encode("utf-8"),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=timeout) as response:
        return json.loads(response.read())


def _one_line(value: Any, width: int = 220) -> str:
    if value is None:
        return ""
    text = str(value).replace("\n", " ")
    return textwrap.shorten(text, width=width, placeholder=" ...")


def _print_summary(payload: dict[str, Any]) -> None:
    print(
        f"query={payload.get('query')!r} total={payload.get('total_count')} "
        f"returned={len(payload.get('results', []))} time_ms={payload.get('query_time_ms')}"
    )
    for idx, result in enumerate(payload.get("results", []), start=1):
        doc = result.get("document") or {}
        print()
        print(
            f"{idx}. score={result.get('score')} match={result.get('match_type')} "
            f"source={result.get('source_type') or doc.get('source_type')}"
        )
        print(f"   id:          {doc.get('id')}")
        print(f"   external_id: {doc.get('external_id')}")
        print(f"   title:       {doc.get('title')}")
        if doc.get("url"):
            print(f"   url:         {doc.get('url')}")

        highlights = result.get("highlights") or []
        for h_idx, highlight in enumerate(highlights[:3], start=1):
            print(f"   highlight {h_idx}: {_one_line(highlight)}")
        if len(highlights) > 3:
            print(f"   ... {len(highlights) - 3} more highlight(s)")


def main() -> int:
    args = _parse_args()
    try:
        body = _build_body(args)
        payload = _post_search(args.searcher_url, body, args.timeout)
    except ValueError as exc:
        sys.stderr.write(f"error: {exc}\n")
        return 2
    except urllib.error.HTTPError as exc:
        sys.stderr.write(f"HTTP {exc.code}: {exc.read().decode('utf-8', 'replace')}\n")
        return 1
    except Exception as exc:  # noqa: BLE001 - CLI should report any failure.
        sys.stderr.write(f"search failed: {exc}\n")
        return 1

    if args.save:
        args.save.parent.mkdir(parents=True, exist_ok=True)
        args.save.write_text(
            json.dumps(payload, indent=2, sort_keys=True), encoding="utf-8"
        )
        print(f"saved raw JSON to {args.save}", file=sys.stderr)

    if args.raw:
        print(json.dumps(payload, indent=2, sort_keys=True))
    else:
        _print_summary(payload)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
