#!/usr/bin/env python3
"""Compare question-only retrieval with oracle title retrieval for semantic questions."""

from __future__ import annotations

import json
import random
import time
import urllib.request
from pathlib import Path

import psycopg


DB = "postgresql://omni_bench:omni_bench_password@localhost:5432/omni_benchmark"
SEARCH_URL = "http://localhost:3001/search"
SEED = 7331
N = 7
LIMIT = 50


def search(query: str, mode: str) -> tuple[list[dict] | None, float, str | None]:
    body = {"query": query, "mode": mode, "limit": LIMIT}
    request = urllib.request.Request(
        SEARCH_URL,
        data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json"},
    )
    started = time.time()
    try:
        with urllib.request.urlopen(request, timeout=100) as response:
            payload = json.loads(response.read())
    except Exception as exc:  # noqa: BLE001 - probe should report failures.
        return None, time.time() - started, repr(exc)
    return payload.get("results", []), time.time() - started, None


def find_rank(results: list[dict] | None, gold_ids: set[str]) -> int | None:
    if results is None:
        return None
    for index, item in enumerate(results, 1):
        if item.get("document", {}).get("external_id") in gold_ids:
            return index
    return None


def top_titles(results: list[dict] | None) -> str:
    if not results:
        return ""
    return " | ".join(
        (item.get("document", {}).get("title") or "")[:34] for item in results[:3]
    )


def main() -> None:
    questions = [
        json.loads(line)
        for line in Path("data/questions.jsonl").open(encoding="utf-8")
        if json.loads(line).get("question_type") == "semantic"
    ]
    random.Random(SEED).shuffle(questions)
    questions = questions[:N]

    with psycopg.connect(DB) as conn:
        with conn.cursor() as cur:
            for question in questions:
                gold_ids = set(question["expected_doc_ids"])
                gold_id = next(iter(gold_ids))
                cur.execute(
                    "SELECT title, LEFT(content, 700) "
                    "FROM documents WHERE external_id = %s",
                    (gold_id,),
                )
                row = cur.fetchone()
                title = row[0] if row else "<missing>"
                preview = (row[1] or "").replace("\n", " ") if row else ""

                print("\n" + "=" * 100)
                print(
                    f"{question['question_id']} source={question.get('source_types')} "
                    f"gold={gold_id}"
                )
                print(f"Q: {question['question']}")
                print(f"Gold title: {title}")
                print(f"Gold preview: {preview[:450]}")

                variants = [
                    ("original", question["question"]),
                    ("gold_title", title),
                    ("title_plus_question", f"{title} {question['question'][:160]}"),
                ]
                for label, query in variants:
                    print(f"\nQUERY {label}: {query[:220]}")
                    for mode in ("fulltext", "semantic", "hybrid"):
                        results, elapsed, error = search(query, mode)
                        if error:
                            print(f"  {mode:8s} ERR {error} t={elapsed:.1f}s")
                            continue
                        rank = find_rank(results, gold_ids)
                        rank_text = str(rank) if rank is not None else "-"
                        print(
                            f"  {mode:8s} rank={rank_text:>2} n={len(results):2d} "
                            f"t={elapsed:4.1f}s top3={top_titles(results)}"
                        )


if __name__ == "__main__":
    main()
