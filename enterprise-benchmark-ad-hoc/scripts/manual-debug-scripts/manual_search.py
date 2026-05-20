#!/usr/bin/env python3
"""Run a manual Omni search query and print the raw JSON response."""

from __future__ import annotations

import argparse
import json
import os
import sys
import urllib.error
import urllib.request


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("query", help="Search query text.")
    parser.add_argument(
        "--mode",
        choices=["fulltext", "semantic", "hybrid"],
        default="hybrid",
        help="Search mode to use.",
    )
    parser.add_argument("--limit", type=int, default=10, help="Number of results.")
    parser.add_argument(
        "--source-types",
        default="",
        help="Optional comma-separated source types, e.g. slack,jira.",
    )
    parser.add_argument(
        "--searcher-url",
        default=os.environ.get("BENCH_SEARCHER_URL", "http://localhost:3001"),
        help="Base URL for omni-searcher.",
    )
    args = parser.parse_args()

    body: dict[str, object] = {
        "query": args.query,
        "mode": args.mode,
        "limit": args.limit,
    }
    source_types = [s.strip() for s in args.source_types.split(",") if s.strip()]
    if source_types:
        body["source_types"] = source_types

    url = args.searcher_url.rstrip("/") + "/search"
    request = urllib.request.Request(
        url,
        data=json.dumps(body).encode("utf-8"),
        headers={"Content-Type": "application/json"},
        method="POST",
    )

    try:
        with urllib.request.urlopen(request, timeout=120) as response:
            payload = json.loads(response.read())
    except urllib.error.HTTPError as exc:
        sys.stderr.write(f"HTTP {exc.code}: {exc.read().decode('utf-8', 'replace')}\n")
        return 1
    except Exception as exc:  # noqa: BLE001 - CLI should report any failure.
        sys.stderr.write(f"Search failed: {exc}\n")
        return 1

    print(json.dumps(payload, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
