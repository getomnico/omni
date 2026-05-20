#!/usr/bin/env python3
"""
Fair query analysis: generate queries based ONLY on question text,
test them against semantic search, see if any would find the gold doc.
No peeking at gold docs for query formulation!
"""

import json
import psycopg

DB_URL = "postgresql://omni_bench:omni_bench_password@localhost:5432/omni_benchmark"


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


def semantic_search(query: str, gold_ids: list, limit: int = 10) -> dict:
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
            gold_rank = None
            for i, row in enumerate(cur.fetchall()):
                ext_id, title, dist = row
                if ext_id in gold_ids and gold_rank is None:
                    gold_rank = i + 1

            return {
                "gold_found": gold_rank is not None,
                "gold_rank": gold_rank,
            }


def get_agent_queries(qtext: str) -> list:
    queries = []
    with psycopg.connect(DB_URL) as conn:
        with conn.cursor() as cur:
            cur.execute(
                "SELECT id FROM chats WHERE title LIKE %s LIMIT 1",
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


def analyze_case(case: dict):
    qid = case["qid"]
    qtext = case["qtext"]
    gold_ids = case["gold_ids"]

    print(f"\n{'='*75}")
    print(f"QUESTION: {qid}")
    print(f"{'='*75}")
    print(f"Q: {qtext}")
    print(f"Gold docs: {gold_ids}")
    print()

    # Show agent's actual queries
    agent_queries = get_agent_queries(qtext)
    print(f"Agent's actual queries ({len(agent_queries)}):")
    for i, q in enumerate(agent_queries, 1):
        print(f"  {i}. {q}")
    print()

    # Test agent queries
    print("Agent queries -- semantic top-10:")
    for q in agent_queries:
        r = semantic_search(q, gold_ids, limit=10)
        status = f"FOUND at rank {r['gold_rank']}" if r["gold_found"] else "NOT FOUND"
        print(f"  {status}: {q}")
    print()

    # Test fair queries (based ONLY on question text)
    print("Fair queries (based ONLY on question text) -- semantic top-10:")
    for q in case["fair_queries"]:
        r = semantic_search(q, gold_ids, limit=10)
        status = f"FOUND at rank {r['gold_rank']}" if r["gold_found"] else "NOT FOUND"
        print(f"  {status}: {q}")
    print()

    # Test with top-30 for fair queries
    print("Fair queries -- semantic top-30:")
    for q in case["fair_queries"]:
        r = semantic_search(q, gold_ids, limit=30)
        status = f"FOUND at rank {r['gold_rank']}" if r["gold_found"] else "NOT FOUND"
        print(f"  {status}: {q}")
    print()


def main():
    # Test cases -- fair queries generated from question text ONLY
    # No peeking at gold docs!
    test_cases = [
        {
            "qid": "qst_0286",
            "qtext": "For a small mobile game studio adding realtime global chat and match lobby messaging, what latency goal did they set for typical short messages so players dont notice added delay?",
            "gold_ids": ["dsid_7711da5856a8485e80e0085ebd193690"],
            "fair_queries": [
                "mobile game studio chat latency goal",
                "game studio realtime messaging latency target",
                "match lobby messaging latency players notice",
                "small game studio chat message delay goal",
                "realtime global chat latency mobile game",
            ],
        },
        {
            "qid": "qst_0043",
            "qtext": "What caused the brief p99 latency jump on the hosted text generation endpoint in us-west-2, and what mitigation was applied to bring it back down?",
            "gold_ids": ["dsid_321eee6325824623833bbe754bf83a0b"],
            "fair_queries": [
                "p99 latency jump hosted text generation us-west-2",
                "hosted text generation endpoint latency spike",
                "us-west-2 text generation p99 spike mitigation",
                "brief p99 latency increase hosted endpoint",
                "latency jump mitigation text generation",
            ],
        },
        {
            "qid": "qst_0245",
            "qtext": "Which fintech support-chat prospect dropped us after their lawyers rejected non-audited compliance proof and chose a rival that could deliver a private install with formal audit paperwork?",
            "gold_ids": ["dsid_d79518c427654533a01173792f51f986"],
            "fair_queries": [
                "fintech support chat prospect lawyers rejected compliance",
                "fintech dropped non-audited compliance proof",
                "private install formal audit paperwork fintech",
                "fintech prospect rival private install audit",
                "support chat fintech lawyers compliance audit",
            ],
        },
        {
            "qid": "qst_0191",
            "qtext": "During a short surge in vector writes last March, a customer in the west coast region saw sustained 5-second delays. Which storage layer and configuration issue was identified?",
            "gold_ids": ["dsid_ef7f1d5cba204b728e1f4d7cddda7daa"],
            "fair_queries": [
                "vector writes surge March west coast delays",
                "storage layer configuration vector writes March",
                "west coast customer vector writes 5 second delays",
                "March vector surge storage configuration issue",
                "customer vector write surge storage layer problem",
            ],
        },
        {
            "qid": "qst_0177",
            "qtext": "What were the final concession terms approved for the retail partner pilot that involved a three-month free trial, a usage cap, and an opt-out clause for the store rollout?",
            "gold_ids": ["dsid_7d925287f5224f72b641acbd515beb2a"],
            "fair_queries": [
                "retail partner pilot three month free trial usage cap",
                "concession terms retail pilot opt-out clause",
                "store rollout pilot free trial usage cap opt-out",
                "retail partner concession terms approved",
                "three month trial usage cap store rollout",
            ],
        },
        {
            "qid": "qst_0227",
            "qtext": "During a late morning April 2025 incident, why did an interactive response pool fail to produce answers, and what was the immediate fix applied?",
            "gold_ids": ["dsid_618649e367d24a1eb3bdcacc0a2d7da7"],
            "fair_queries": [
                "April 2025 incident interactive response pool failed",
                "late morning April response pool answers failed",
                "interactive response pool failure April 2025 fix",
                "response pool not producing answers April",
                "April 2025 morning incident response pool fix",
            ],
        },
    ]

    for case in test_cases:
        analyze_case(case)


if __name__ == "__main__":
    main()
