"""Run each question in questions_subset.jsonl through Omni's /search and write
the EnterpriseRAG-Bench submission JSONL (question_id, document_ids, answer="").

Retrieval-only: the `answer` field is left empty; the Onyx eval scores only
Document Recall and Invalid Extra Documents in this mode.
"""

from __future__ import annotations

import argparse
import json
import logging
import os
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import httpx
from tqdm import tqdm

logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")
log = logging.getLogger("run_search")


@dataclass(frozen=True)
class Question:
    question_id: str
    text: str
    source_types: list[str]


@dataclass(frozen=True)
class Answer:
    question_id: str
    document_ids: list[str]


def _parse_question(row: dict[str, Any]) -> Question:
    qid = row["question_id"]
    # Bench uses "question" as the prompt field; fail loud if missing rather
    # than silently submitting an empty query.
    text = row["question"]
    src = row.get("source_types") or []
    return Question(question_id=qid, text=text, source_types=list(src))


def search_one(
    client: httpx.Client,
    url: str,
    q: Question,
    mode: str,
    limit: int,
    source_types: list[str],
) -> Answer:
    body: dict[str, Any] = {
        "query": q.text,
        "mode": mode,
        "limit": limit,
    }
    if source_types:
        body["source_types"] = source_types
    resp = client.post(url, json=body, timeout=60.0)
    resp.raise_for_status()
    payload = resp.json()
    doc_ids: list[str] = []
    seen: set[str] = set()
    for r in payload.get("results", []):
        ext = r["document"]["external_id"]
        if ext in seen:
            continue
        seen.add(ext)
        doc_ids.append(ext)
    return Answer(question_id=q.question_id, document_ids=doc_ids)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--questions",
        type=Path,
        default=Path(__file__).parent / "questions_subset.jsonl",
    )
    parser.add_argument(
        "--searcher-url",
        default=os.environ.get(
            "BENCH_SEARCHER_URL",
            f"http://localhost:{os.environ.get('SEARCHER_PORT', '3001')}",
        ),
    )
    parser.add_argument(
        "--mode", choices=["fulltext", "semantic", "hybrid"], default="hybrid"
    )
    parser.add_argument("--limit", type=int, default=10)
    parser.add_argument(
        "--source-types",
        default="",
        help="comma-separated list of source types to filter by (default: empty = no filter)",
    )
    parser.add_argument("--concurrency", type=int, default=4)
    parser.add_argument(
        "--no-cache-flush",
        action="store_true",
        help="skip the Redis FLUSHALL before searching (default: flush, so each "
        "benchmark run executes a real semantic+BM25 query instead of replaying "
        "cached responses from a previous run)",
    )
    parser.add_argument(
        "--redis-container",
        default=os.environ.get("BENCH_REDIS_CONTAINER", "omni-redis-benchmark"),
        help="docker container name for the searcher's Redis cache",
    )
    parser.add_argument(
        "--output-dir", type=Path, default=Path(__file__).parent / "answer_evaluation"
    )
    parser.add_argument(
        "--output-suffix",
        default="",
        help="appended to answers filename, e.g. '_hybrid_k10'",
    )
    args = parser.parse_args()

    source_types = [s.strip() for s in args.source_types.split(",") if s.strip()]
    args.output_dir.mkdir(parents=True, exist_ok=True)
    out_path = args.output_dir / f"answers{args.output_suffix}.jsonl"

    if not args.no_cache_flush:
        # The searcher caches /search responses in Redis; without a flush, a
        # second run replays the first run's results regardless of changes to
        # mode, embeddings, or timeout config. We learned this the hard way
        # when a hybrid run cache-hit a degraded fulltext-fallback run.
        try:
            r = subprocess.run(
                ["docker", "exec", args.redis_container, "redis-cli", "FLUSHALL"],
                check=True,
                capture_output=True,
                text=True,
            )
            log.info(
                "redis cache flushed (%s): %s", args.redis_container, r.stdout.strip()
            )
        except (subprocess.CalledProcessError, FileNotFoundError) as exc:
            log.warning(
                "could not flush redis cache (%s); proceeding anyway: %s",
                args.redis_container,
                exc,
            )

    questions: list[Question] = []
    with args.questions.open("r", encoding="utf-8") as f:
        for line in f:
            line = line.rstrip("\n")
            if not line:
                continue
            questions.append(_parse_question(json.loads(line)))

    log.info(
        "loaded %d questions; mode=%s limit=%d source_types=%s",
        len(questions),
        args.mode,
        args.limit,
        source_types,
    )

    search_url = args.searcher_url.rstrip("/") + "/search"
    answers: dict[str, Answer] = {}

    with httpx.Client() as client, ThreadPoolExecutor(
        max_workers=args.concurrency
    ) as pool:
        futures = {
            pool.submit(
                search_one, client, search_url, q, args.mode, args.limit, source_types
            ): q
            for q in questions
        }
        for fut in tqdm(
            as_completed(futures), total=len(futures), unit="q", desc=args.mode
        ):
            q = futures[fut]
            try:
                answers[q.question_id] = fut.result()
            except Exception as exc:
                log.error("question %s failed: %s", q.question_id, exc)

    # Preserve input ordering in the output for deterministic diffs.
    with out_path.open("w", encoding="utf-8") as out:
        for q in questions:
            ans = answers.get(q.question_id)
            if ans is None:
                continue
            out.write(
                json.dumps(
                    {
                        "question_id": ans.question_id,
                        "answer": "",
                        "document_ids": ans.document_ids,
                    }
                )
                + "\n"
            )

    log.info("wrote %d answers to %s", len(answers), out_path)
    if len(answers) != len(questions):
        log.warning(
            "%d question(s) failed and were not written", len(questions) - len(answers)
        )
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
