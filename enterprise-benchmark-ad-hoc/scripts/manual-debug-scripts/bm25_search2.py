#!/usr/bin/env python3
import psycopg

DB_URL = "postgresql://omni_bench:omni_bench_password@localhost:5432/omni_benchmark"

gold = "dsid_0aebc4d1e7264c6c90136b6b780a0c67"

queries = [
    "load test uneven prompt lengths",
    "load test attention scratch memory",
    "GPU load test attention",
    "serving load test prompt length variation",
    "tail latency load test attention kernel",
    "scratch memory growth attention kernel",
    "prevent attention kernel switch",
    "runtime attention implementation load test",
    "continuous batching load test uneven prompts",
    "kernel selector load test uneven prompts",
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
