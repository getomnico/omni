#!/usr/bin/env python3
"""Pre-evaluation cleanup script for Omni benchmark runs.

Clears DB chats/messages, old answer files, and resets state for a clean run.
"""
import glob
import json
import os
import sys

import psycopg

DB_URL = "postgresql://omni_bench:omni_bench_password@localhost:5432/omni_benchmark"
ANSWER_DIR = "/root/omni/benchmark/answer_evaluation"


def clear_db():
    """Truncate chats and chat_messages tables."""
    print("Clearing DB chats and messages...")
    with psycopg.connect(DB_URL) as conn:
        with conn.cursor() as cur:
            cur.execute("TRUNCATE TABLE chat_messages, chats CASCADE;")
            cur.execute("SELECT COUNT(*) FROM chats;")
            chat_count = cur.fetchone()[0]
            cur.execute("SELECT COUNT(*) FROM chat_messages;")
            msg_count = cur.fetchone()[0]
            cur.execute("SELECT COUNT(*) FROM model_usage;")
            usage_count = cur.fetchone()[0]
    print(
        f"  DB state: {chat_count} chats, {msg_count} messages, {usage_count} usage records"
    )
    return chat_count == 0 and msg_count == 0


def clear_answer_files(system_name: str | None = None):
    """Remove old answer/result/trace files."""
    if system_name:
        patterns = [
            f"{ANSWER_DIR}/answers_{system_name}*.jsonl",
            f"{ANSWER_DIR}/results_{system_name}*.json",
            f"{ANSWER_DIR}/retrieval_trace_{system_name}*.jsonl",
        ]
    else:
        patterns = [
            f"{ANSWER_DIR}/answers_*.jsonl",
            f"{ANSWER_DIR}/results_*.json",
            f"{ANSWER_DIR}/retrieval_trace_*.jsonl",
        ]

    removed = 0
    for pattern in patterns:
        for f in glob.glob(pattern):
            # Keep smoke5_fixed files as reference
            if "smoke5_fixed" in f:
                continue
            os.remove(f)
            removed += 1
            print(f"  Removed: {os.path.basename(f)}")
    print(f"  Removed {removed} files")
    return removed


def verify_systems():
    """Quick health checks."""
    import urllib.request

    checks = {
        "searcher": "http://localhost:3001/health",
        "ai": "http://localhost:3003/health",
    }

    all_ok = True
    for name, url in checks.items():
        try:
            resp = urllib.request.urlopen(url, timeout=5)
            data = json.loads(resp.read())
            status = data.get("status", "unknown")
            print(f"  {name}: {status}")
            if status != "healthy":
                all_ok = False
        except Exception as e:
            print(f"  {name}: ERROR - {e}")
            all_ok = False

    return all_ok


def main():
    system_name = sys.argv[1] if len(sys.argv) > 1 else None

    print("=" * 60)
    print("Pre-evaluation cleanup")
    print("=" * 60)

    print("\n[1] Clearing DB...")
    db_ok = clear_db()

    print("\n[2] Clearing answer files...")
    clear_answer_files(system_name)

    print("\n[3] Health checks...")
    health_ok = verify_systems()

    print("\n" + "=" * 60)
    if db_ok and health_ok:
        print("✓ Cleanup complete. Ready to run benchmark.")
        return 0
    else:
        print("✗ Issues detected. Please fix before running.")
        return 1


if __name__ == "__main__":
    sys.exit(main())
