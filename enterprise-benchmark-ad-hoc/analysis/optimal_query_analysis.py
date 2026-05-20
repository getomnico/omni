#!/usr/bin/env python3
"""Deep-dive: Test optimal queries for semantic category failures using SQL."""

import json
import psycopg

DB_URL = "postgresql://omni_bench:omni_bench_password@localhost:5432/omni_benchmark"


def get_doc_by_external_id(external_id: str) -> dict | None:
    with psycopg.connect(DB_URL) as conn:
        with conn.cursor() as cur:
            cur.execute(
                "SELECT external_id, title, source_id, content_type, LEFT(content, 800) "
                "FROM documents WHERE external_id = %s",
                (external_id,),
            )
            row = cur.fetchone()
            if row:
                return {
                    "external_id": row[0],
                    "title": row[1],
                    "source_id": row[2],
                    "content_type": row[3],
                    "content_preview": row[4],
                }
            return None


def get_agent_queries(qtext: str) -> list:
    """Get actual agent search queries from DB by matching question text."""
    queries = []
    with psycopg.connect(DB_URL) as conn:
        with conn.cursor() as cur:
            # Match by question text prefix
            cur.execute(
                "SELECT id, title FROM chats WHERE title LIKE %s LIMIT 1",
                (f"%{qtext[:40]}%",),
            )
            row = cur.fetchone()
            if not row:
                return []
            chat_id = row[0]
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
                            queries.append(block.get("input", {}).get("query", ""))
    return queries


def get_query_embedding(query: str) -> list:
    import urllib.request

    req = urllib.request.Request(
        "http://172.18.0.1:18091/embed",
        data=json.dumps({"inputs": query}).encode(),
        headers={"Content-Type": "application/json"},
    )
    resp = urllib.request.urlopen(req, timeout=30)
    emb_data = json.loads(resp.read())
    if isinstance(emb_data, list) and len(emb_data) > 0:
        if isinstance(emb_data[0], list):
            return emb_data[0]
        return emb_data
    raise ValueError(f"Unexpected embed response: {emb_data}")


def search_sql(query: str, gold_ids: list, limit: int = 10) -> dict:
    """Search using direct SQL and check if gold is in results."""
    query_embedding = get_query_embedding(query)
    query_vec = "[" + ",".join(str(x) for x in query_embedding) + "]"

    with psycopg.connect(DB_URL) as conn:
        with conn.cursor() as cur:
            cur.execute(
                "SELECT d.external_id, d.title, "
                "(e.embedding)::vector(1024) <=> %s::vector(1024) as distance "
                "FROM embeddings e "
                "JOIN documents d ON d.id = e.document_id "
                "WHERE e.dimensions = 1024 "
                "ORDER BY distance LIMIT %s",
                (query_vec, limit),
            )
            results = []
            gold_rank = None
            for i, row in enumerate(cur.fetchall()):
                ext_id, title, dist = row
                results.append((ext_id, title, dist))
                if ext_id in gold_ids and gold_rank is None:
                    gold_rank = i + 1

            return {
                "results": results,
                "gold_found": gold_rank is not None,
                "gold_rank": gold_rank,
            }


def analyze_question(qid: str, qtext: str, gold_ids: list, optimal_queries: list):
    print(f"\n{'='*75}")
    print(f"QUESTION: {qid}")
    print(f"{'='*75}")
    print(f"Q: {qtext}")
    print(f"Gold docs: {gold_ids}")
    print()

    # Show gold doc preview
    for gid in gold_ids:
        doc = get_doc_by_external_id(gid)
        if doc:
            print(f"Gold doc: {gid}")
            print(f"  Title: {doc['title']}")
            print(f"  Source: {doc['source_id']} | {doc['content_type']}")
            print(f"  Preview: {doc['content_preview'][:400]}...")
            print()

    # Show agent's actual queries
    agent_queries = get_agent_queries(qtext)
    print(f"Agent queries ({len(agent_queries)}):")
    for i, q in enumerate(agent_queries, 1):
        print(f"  {i}. {q}")
    print()

    # Test each agent query
    if agent_queries:
        print("Agent query results (semantic top-10):")
        for q in agent_queries:
            r = search_sql(q, gold_ids, limit=10)
            status = (
                f"FOUND at rank {r['gold_rank']}" if r["gold_found"] else "NOT FOUND"
            )
            print(f"  '{q[:55]}...' -> {status}")
        print()

    # Test optimal queries
    print("OPTIMAL QUERY TESTING (semantic top-10):")
    for q in optimal_queries:
        r = search_sql(q, gold_ids, limit=10)
        status = f"FOUND at rank {r['gold_rank']}" if r["gold_found"] else "NOT FOUND"
        print(f"  '{q}' -> {status}")
        if not r["gold_found"]:
            print(f"    Top 3: {[(x[0][:20], x[1][:30]) for x in r['results'][:3]]}")
    print()

    # Test with larger limit
    print("OPTIMAL QUERY TESTING (semantic top-30):")
    for q in optimal_queries:
        r = search_sql(q, gold_ids, limit=30)
        status = f"FOUND at rank {r['gold_rank']}" if r["gold_found"] else "NOT FOUND"
        print(f"  '{q}' -> {status}")
    print()

    # Test hybrid (BM25) via SQL
    print("OPTIMAL QUERY TESTING (BM25 fulltext top-10):")
    with psycopg.connect(DB_URL) as conn:
        with conn.cursor() as cur:
            for q in optimal_queries:
                cur.execute(
                    "SELECT d.external_id, d.title, "
                    "paradedb.score(d.id) as bm25_score "
                    "FROM documents d "
                    "WHERE d.id @@@ paradedb.match('content', %s) "
                    "ORDER BY paradedb.score(d.id) DESC LIMIT 10",
                    (q,),
                )
                results = []
                gold_rank = None
                for i, row in enumerate(cur.fetchall()):
                    ext_id, title, score = row
                    results.append((ext_id, title, score))
                    if ext_id in gold_ids and gold_rank is None:
                        gold_rank = i + 1
                status = f"FOUND at rank {gold_rank}" if gold_rank else "NOT FOUND"
                print(f"  '{q}' -> {status}")
                if not gold_rank:
                    print(f"    Top 3: {[(x[0][:20], x[1][:30]) for x in results[:3]]}")
    print()


def main():
    test_cases = [
        {
            "qid": "qst_0286",
            "qtext": "For a small mobile game studio adding realtime global chat and match lobby messaging, what latency goal did they set for typical short messages so players dont notice added delay?",
            "gold_ids": ["dsid_7711da5856a8485e80e0085ebd193690"],
            "optimal_queries": [
                "Stellar Sparrow Games",
                "Stellar Sparrow latency",
                "small mobile game studio p50 latency 80ms",
            ],
        },
        {
            "qid": "qst_0043",
            "qtext": "What caused the brief p99 latency jump on the hosted text generation endpoint in us-west-2, and what mitigation was applied to bring it back down?",
            "gold_ids": ["dsid_321eee6325824623833bbe754bf83a0b"],
            "optimal_queries": [
                "eng-runtime p99 spike",
                "tenant-32 KV cache eviction us-west-2",
                "hosted text generation p99 latency jump mitigation",
            ],
        },
        {
            "qid": "qst_0245",
            "qtext": "Which fintech support-chat prospect dropped us after their lawyers rejected non-audited compliance proof and chose a rival that could deliver a private install with formal audit paperwork?",
            "gold_ids": ["dsid_d79518c427654533a01173792f51f986"],
            "optimal_queries": [
                "Onyx Cloud Labs",
                "Onyx Cloud Labs fintech compliance",
                "fintech prospect dropped lawyers compliance audit private install",
            ],
        },
        {
            "qid": "qst_0191",
            "qtext": "During a short surge in vector writes last March, a customer in the west coast region saw sustained 5-second delays. Which storage layer and configuration issue was identified?",
            "gold_ids": ["dsid_ef7f1d5cba204b728e1f4d7cddda7daa"],
            "optimal_queries": [
                "Falkon Systems vector writes March",
                "vector writes surge March west coast storage",
                "embedding index reconciliation mismatch",
            ],
        },
    ]

    for case in test_cases:
        analyze_question(
            case["qid"], case["qtext"], case["gold_ids"], case["optimal_queries"]
        )


if __name__ == "__main__":
    main()
