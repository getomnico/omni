#!/usr/bin/env python3
"""Probe semantic benchmark questions against Omni search modes.

This intentionally uses only question-derived query variants. Gold documents are
used only for scoring ranks after retrieval.
"""

from __future__ import annotations

import json
import random
import re
import time
import urllib.request
from pathlib import Path


QUESTIONS = Path("data/questions.jsonl")
SEARCH_URL = "http://localhost:3001/search"
SEED = 7331
N = 7
LIMIT = 50

STOP_WORDS = set(
    """
    a an and are as at be because been but by can could did do does during for
    from had has have how in into is it its of on or our over so some that the
    their they this through to under was were what when where which who why with
    without would new old after before between about also any both each few more
    most other same such than then there these those very via we us
    """.split()
)

NORMALIZATIONS = [
    (r"\bEU Central\b", "eu-central-1"),
    (r"\bIndia South\b", "ap-south-1"),
    (r"\bwestern US\b", "us-west-2"),
    (r"\bwest coast\b", "us-west-2"),
    (r"\btoo many requests\b", "429"),
    (r"\bservice unavailable\b", "503"),
    (r"\btail latency\b", "p99 latency"),
    (r"\berror rate\b", "5xx rate"),
    (r"\battention memory\b", "KV cache"),
    (r"\btraffic gatekeeper\b", "rate limit proxy quota"),
    (r"\bmasking\b", "redaction"),
    (r"\blawful-ground\b", "legal_basis"),
    (r"\blawful ground\b", "legal_basis"),
    (r"\btrusted timestamp\b", "RFC3161 timestamp"),
    (r"\btop end 80GB accelerator\b", "H200 80GB GPU accelerator"),
]


def tokens(text: str) -> list[str]:
    return re.findall(r"[A-Za-z0-9][A-Za-z0-9_./+-]*", text)


def keyword_query(text: str) -> str:
    kept: list[str] = []
    for token in tokens(text):
        lower = token.lower()
        if len(lower) <= 2 and not re.search(r"\d", lower):
            continue
        if lower in STOP_WORDS:
            continue
        kept.append(token)
    return " ".join(kept[:18])


def normalized_query(text: str) -> str:
    normalized = text
    for pattern, value in NORMALIZATIONS:
        normalized = re.sub(pattern, value, normalized, flags=re.I)
    return keyword_query(normalized)


def search(query: str, mode: str, source_types: list[str]) -> dict:
    body: dict[str, object] = {"query": query, "mode": mode, "limit": LIMIT}
    if source_types:
        body["source_types"] = source_types
    request = urllib.request.Request(
        SEARCH_URL,
        data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(request, timeout=90) as response:
            payload = json.loads(response.read())
    except Exception as exc:  # noqa: BLE001 - probe should report failures.
        return {"error": repr(exc), "results": []}

    results = []
    for item in payload.get("results", []):
        doc = item.get("document", {})
        results.append(
            {
                "id": doc.get("external_id"),
                "title": doc.get("title"),
                "score": item.get("score"),
            }
        )
    return {"results": results}


def main() -> None:
    semantic_questions = []
    for line in QUESTIONS.open(encoding="utf-8"):
        question = json.loads(line)
        if question.get("question_type") == "semantic":
            semantic_questions.append(question)

    random.Random(SEED).shuffle(semantic_questions)
    sample = semantic_questions[:N]
    print(f"sample_seed={SEED} n={N} limit={LIMIT}")

    totals = {
        (mode, variant): 0
        for mode in ("fulltext", "semantic", "hybrid")
        for variant in ("original", "keywords", "normalized")
    }

    for question in sample:
        qid = question["question_id"]
        gold = set(question["expected_doc_ids"])
        source_types = question.get("source_types") or []
        variants = []
        seen = set()
        for label, query in [
            ("original", question["question"]),
            ("keywords", keyword_query(question["question"])),
            ("normalized", normalized_query(question["question"])),
        ]:
            if query and query not in seen:
                seen.add(query)
                variants.append((label, query))

        print("\n" + "=" * 88)
        print(f"{qid} sources={source_types} gold={sorted(gold)}")
        print(f"Q: {question['question']}")

        for label, query in variants:
            print(f"\nQUERY[{label}]: {query}")
            for mode in ("fulltext", "semantic", "hybrid"):
                started = time.time()
                response = search(query, mode, source_types)
                elapsed = time.time() - started
                if response.get("error"):
                    print(f"  {mode:8s} ERROR {response['error']} ({elapsed:.1f}s)")
                    continue

                results = response["results"]
                rank = None
                for index, result in enumerate(results, 1):
                    if result["id"] in gold:
                        rank = index
                        break
                if rank is not None:
                    totals[(mode, label)] += 1
                top = " | ".join(
                    f"{index + 1}:{(result.get('title') or '')[:42]}"
                    for index, result in enumerate(results[:3])
                )
                rank_text = str(rank) if rank is not None else "-"
                print(
                    f"  {mode:8s} rank={rank_text:>2} returned={len(results):2d} "
                    f"time={elapsed:5.1f}s top3={top}"
                )

    print("\nSUMMARY found-in-top-50 counts")
    for mode in ("fulltext", "semantic", "hybrid"):
        parts = [
            f"{variant}={totals[(mode, variant)]}/{N}"
            for variant in ("original", "keywords", "normalized")
        ]
        print(f"{mode:8s} " + " ".join(parts))


if __name__ == "__main__":
    main()
