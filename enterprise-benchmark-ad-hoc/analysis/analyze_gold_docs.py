#!/usr/bin/env python3
"""Analyze why gold documents were missed."""

import json
import psycopg

DB_URL = "postgresql://omni_bench:omni_bench_password@localhost:5432/omni_benchmark"


def get_doc_by_external_id(external_id: str) -> dict | None:
    with psycopg.connect(DB_URL) as conn:
        with conn.cursor() as cur:
            cur.execute(
                "SELECT id, external_id, title, source_id, content_type, url, LEFT(content, 500) as content_preview FROM documents WHERE external_id = %s",
                (external_id,),
            )
            row = cur.fetchone()
            if row:
                return {
                    "id": row[0],
                    "external_id": row[1],
                    "title": row[2],
                    "source_id": row[3],
                    "content_type": row[4],
                    "url": row[5],
                    "content_preview": row[6],
                }
            return None


def get_chat_searches(chat_id: str) -> list:
    """Extract search queries from a chat."""
    searches = []
    with psycopg.connect(DB_URL) as conn:
        with conn.cursor() as cur:
            cur.execute(
                "SELECT message FROM chat_messages WHERE chat_id = %s ORDER BY message_seq_num",
                (chat_id,),
            )
            for row in cur.fetchall():
                msg = row[0]
                if msg.get("role") == "assistant":
                    for block in msg.get("content", []):
                        if (
                            block.get("type") == "tool_use"
                            and block.get("name") == "search_documents"
                        ):
                            searches.append(block.get("input", {}).get("query", ""))
    return searches


def analyze_question(qid: str, questions: dict, answers: dict, results: dict):
    q = questions[qid]
    a = answers.get(qid, {})
    r = results.get(qid, {})

    print(f"\n{'='*70}")
    print(f"QUESTION: {qid}")
    print(f"{'='*70}")
    print(f"Text: {q['question']}")
    print()

    gold_docs = q.get("expected_doc_ids", [])
    print(f"Gold docs ({len(gold_docs)}):")
    for gd in gold_docs:
        doc = get_doc_by_external_id(gd)
        if doc:
            print(f"  {gd}")
            print(f"    Title: {doc['title']}")
            print(f"    Source: {doc['source_id']} | {doc['content_type']}")
            print(f"    Preview: {doc['content_preview'][:200]}...")
        else:
            print(f"  {gd} - NOT FOUND IN INDEX")
    print()

    our_docs = a.get("document_ids", [])
    print(f"Our docs ({len(our_docs)}):")
    for d in our_docs[:5]:
        print(f"  {d}")
    print()

    # Find chat for this question
    with psycopg.connect(DB_URL) as conn:
        with conn.cursor() as cur:
            cur.execute(
                "SELECT id FROM chats WHERE title LIKE %s LIMIT 1",
                (f"%{q['question'][:40]}%",),
            )
            row = cur.fetchone()
            if row:
                chat_id = row[0]
                searches = get_chat_searches(chat_id)
                print(f"Agent searches ({len(searches)}):")
                for s in searches:
                    print(f"  - {s}")
            else:
                print("Chat not found")
    print()

    print(
        f"Scores: correct={r.get('answer_correct')}, complete={r.get('completeness_pct')}%, recall={r.get('document_recall_pct', 0)}%"
    )


def main():
    import sys

    qids = sys.argv[1:] if len(sys.argv) > 1 else ["qst_0286", "qst_0043", "qst_0245"]

    questions = {}
    with open("/tmp/smoke100_random.jsonl") as f:
        for line in f:
            q = json.loads(line)
            questions[q["question_id"]] = q

    answers = {}
    with open(
        "/root/omni/benchmark/answer_evaluation/answers_omni_agentic_deepseek_random100.jsonl"
    ) as f:
        for line in f:
            a = json.loads(line)
            answers[a["question_id"]] = a

    results_data = json.load(
        open(
            "/root/omni/benchmark/answer_evaluation/results_omni_agentic_deepseek_random100.json"
        )
    )
    results = {q["question_id"]: q for q in results_data["questions"]}

    for qid in qids:
        if qid in questions:
            analyze_question(qid, questions, answers, results)
        else:
            print(f"Question {qid} not found")


if __name__ == "__main__":
    main()
