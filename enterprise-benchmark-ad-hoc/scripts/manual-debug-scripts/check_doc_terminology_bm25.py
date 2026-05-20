import psycopg

DB_URL = "postgresql://omni_bench:omni_bench_password@localhost:5432/omni_benchmark"

gold = "dsid_0aebc4d1e7264c6c90136b6b780a0c67"

queries = [
    "paged-attn block_size=64 sequence variance",
    "kernel fallback workspace estimate grows",
    "salvage-first watermark kernel fallback",
    "load test paged attention block size 64",
    "high sequence variance workspace estimate",
    "paged-attn micro-evicts perf cliff",
    "watermark prevent kernel fallback",
    "akshay load test paged-attn",
    "salvage-first watermark",
]

with psycopg.connect(DB_URL) as conn:
    with conn.cursor() as cur:
        for q in queries:
            try:
                cur.execute(
                    "SELECT d.external_id, d.title, paradedb.score(d.id) as s "
                    "FROM documents d WHERE d.id @@@ paradedb.match('content', %s) "
                    "ORDER BY paradedb.score(d.id) DESC LIMIT 10",
                    (q,),
                )
                rows = cur.fetchall()
                ids = [r[0] for r in rows]
                found = gold in ids
                rank = ids.index(gold) + 1 if found else None
                print(f"BM25: '{q}' | gold in top-10: {found} | rank: {rank}")
                if not found:
                    for i, row in enumerate(rows[:3]):
                        print(f"  {i+1}. [{row[2]:.1f}] {row[1][:45]} | {row[0]}")
                print()
            except Exception as e:
                print(f"BM25: '{q}' -> ERROR: {e}")
