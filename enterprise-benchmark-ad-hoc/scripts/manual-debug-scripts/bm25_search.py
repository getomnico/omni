#!/usr/bin/env python3
import psycopg

DB_URL = "postgresql://omni_bench:omni_bench_password@localhost:5432/omni_benchmark"

queries = [
    "uneven prompt lengths",
    "slower attention implementation",
    "temporary scratch memory",
    "scratch memory growth",
    "GPU load test prompt lengths",
    "attention implementation switch",
    "prevented runtime switching attention",
    "load test uneven prompt",
    "tail latency reduction attention",
    "kernel switch scratch",
]

with psycopg.connect(DB_URL) as conn:
    with conn.cursor() as cur:
        for q in queries:
            try:
                cur.execute(
                    "SELECT d.external_id, d.title, paradedb.score(d.id) as s "
                    "FROM documents d WHERE d.id @@@ paradedb.match('content', %s) "
                    "ORDER BY paradedb.score(d.id) DESC LIMIT 5",
                    (q,),
                )
                print(f"BM25 QUERY: '{q}'")
                for i, row in enumerate(cur.fetchall()):
                    print(f"  {i+1}. [{row[2]:.2f}] {row[1][:50]} | {row[0]}")
                print()
            except Exception as e:
                print(f"BM25 QUERY: '{q}' -> ERROR: {e}")
