import urllib.request
import json
import psycopg

DB_URL = "postgresql://omni_bench:omni_bench_password@localhost:5432/omni_benchmark"

queries = {
    "p99 latency jump hosted text generation endpoint us-west-2": [
        "dsid_321eee6325824623833bbe754bf83a0b"
    ],
    "mobile game studio realtime global chat match lobby messaging latency goal": [
        "dsid_7711da5856a8485e80e0085ebd193690"
    ],
    "fintech support-chat prospect lawyers rejected non-audited compliance proof rival private install formal audit paperwork": [
        "dsid_d79518c427654533a01173792f51f986"
    ],
}

for query, gold_ids in queries.items():
    # Get query embedding from TEI
    req = urllib.request.Request(
        "http://172.18.0.1:18091/embed",
        data=json.dumps({"inputs": query}).encode(),
        headers={"Content-Type": "application/json"},
    )
    resp = urllib.request.urlopen(req, timeout=30)
    emb_data = json.loads(resp.read())
    if isinstance(emb_data, list) and len(emb_data) > 0:
        if isinstance(emb_data[0], list):
            query_embedding = emb_data[0]
        else:
            query_embedding = emb_data
    else:
        print(f"Unexpected embed response: {emb_data}")
        continue

    query_vec = "[" + ",".join(str(x) for x in query_embedding) + "]"

    with psycopg.connect(DB_URL) as conn:
        with conn.cursor() as cur:
            # Check rank of gold doc
            cur.execute(
                "SELECT d.external_id, d.title, "
                "(e.embedding)::vector(1024) <=> %s::vector(1024) as distance "
                "FROM documents d "
                "JOIN embeddings e ON e.document_id = d.id "
                "WHERE d.external_id = ANY(%s) "
                "ORDER BY distance",
                (query_vec, gold_ids),
            )
            print(f"Query: {query[:50]}...")
            print("Gold doc distances (lower = better):")
            for row in cur.fetchall():
                print(f"  {row[0][:20]}... | {row[1][:30]:30s} | dist={row[2]:.4f}")

            # Check top 10 overall
            cur.execute(
                "SELECT d.external_id, d.title, "
                "(e.embedding)::vector(1024) <=> %s::vector(1024) as distance "
                "FROM embeddings e "
                "JOIN documents d ON d.id = e.document_id "
                "WHERE e.dimensions = 1024 "
                "ORDER BY distance LIMIT 10",
                (query_vec,),
            )
            print("Top 10 semantic results:")
            for row in cur.fetchall():
                print(f"  {row[0][:20]}... | {row[1][:30]:30s} | dist={row[2]:.4f}")
            print()
