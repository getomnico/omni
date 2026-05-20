"""fix_doc_ids.py — Post-process an agentic answers file to resolve internal
Omni ULIDs back to external dsid_* IDs required by the EnterpriseRAG-Bench eval.

Usage:
    uv run python fix_doc_ids.py --input answers_omni_agentic_deepseek.jsonl

Writes a new file with _fixed suffix.
"""

import argparse
import json
from pathlib import Path

import psycopg


def _resolve_doc_ids(conn: psycopg.Connection, doc_ids: list[str]) -> list[str]:
    """Map internal Omni ULIDs back to external dsid_* IDs."""
    resolved: list[str] = []
    with conn.cursor() as cur:
        for doc_id in doc_ids:
            if doc_id.startswith("dsid_"):
                resolved.append(doc_id)
                continue
            cur.execute(
                "SELECT external_id FROM documents WHERE id = %s",
                (doc_id,),
            )
            row = cur.fetchone()
            if row and row[0]:
                resolved.append(row[0])
            else:
                resolved.append(doc_id)
    return resolved


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", type=Path, required=True)
    parser.add_argument("--output", type=Path, default=None)
    parser.add_argument("--db-host", default="localhost")
    parser.add_argument("--db-port", type=int, default=5432)
    parser.add_argument("--db-name", default="omni_benchmark")
    parser.add_argument("--db-user", default="omni_bench")
    parser.add_argument("--db-password", default="omni_bench_password")
    args = parser.parse_args()

    out_path = args.output or args.input.with_suffix(".fixed.jsonl")

    db_dsn = (
        f"host={args.db_host} port={args.db_port} dbname={args.db_name} "
        f"user={args.db_user} password={args.db_password}"
    )

    fixed_count = 0
    with psycopg.connect(db_dsn) as conn, args.input.open("r") as fin, out_path.open(
        "w"
    ) as fout:
        for line in fin:
            obj = json.loads(line)
            raw_ids = obj.get("document_ids", [])
            fixed_ids = _resolve_doc_ids(conn, raw_ids)
            if fixed_ids != raw_ids:
                fixed_count += 1
            obj["document_ids"] = fixed_ids
            fout.write(json.dumps(obj) + "\n")

    print(f"Wrote {out_path} — fixed {fixed_count} rows")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
