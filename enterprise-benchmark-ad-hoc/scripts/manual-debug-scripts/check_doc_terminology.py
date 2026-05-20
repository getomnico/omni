import json
import urllib.request
import psycopg

DB_URL = "postgresql://omni_bench:omni_bench_password@localhost:5432/omni_benchmark"
TEI_URL = "http://172.18.0.1:18091/embed"

gold = "dsid_0aebc4d1e7264c6c90136b6b780a0c67"

queries = [
    "paged-attn block_size=64 sequence variance",
    "kernel fallback workspace estimate grows",
    "salvage-first watermark kernel fallback",
    "load test paged attention block size 64",
    "high sequence variance workspace estimate",
    "paged-attn micro-evicts perf cliff",
    "watermark prevent kernel fallback",
]

for query in queries:
    req = urllib.request.Request(
        TEI_URL,
        data=json.dumps({"inputs": query}).encode(),
        headers={"Content-Type": "application/json"},
    )
    resp = urllib.request.urlopen(req, timeout=30)
    emb_data = json.loads(resp.read())
    query_embedding = emb_data[0] if isinstance(emb_data[0], list) else emb_data
    query_vec = "[" + ",".join(str(x) for x in query_embedding) + "]"

    with psycopg.connect(DB_URL) as conn:
        with conn.cursor() as cur:
            cur.execute(
                "SELECT d.external_id, d.title, (e.embedding)::vector(1024) <=> %s::vector(1024) as dist FROM embeddings e JOIN documents d ON d.id = e.document_id WHERE e.dimensions = 1024 ORDER BY dist LIMIT 10",
                (query_vec,),
            )
            rows = cur.fetchall()
            ids = [r[0] for r in rows]
            found = gold in ids
            rank = ids.index(gold) + 1 if found else None
            print(f"QUERY: {query}")
            print(f"  gold in top-10: {found} | rank: {rank}")
            if not found:
                for i, row in enumerate(rows[:3]):
                    print(f"  {i+1}. [{row[2]:.4f}] {row[1][:45]}")
            print()
