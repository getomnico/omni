import psycopg
import sys

DB_URL = "postgresql://omni_bench:omni_bench_password@localhost:5432/omni_benchmark"

question_keyword = sys.argv[1] if len(sys.argv) > 1 else "p99 latency jump"

with psycopg.connect(DB_URL) as conn:
    with conn.cursor() as cur:
        cur.execute(
            "SELECT id, title FROM chats WHERE title LIKE %s LIMIT 1",
            (f"%{question_keyword}%",),
        )
        row = cur.fetchone()
        if not row:
            print(f"No chat found for keyword: {question_keyword}")
            sys.exit(1)

        chat_id, title = row
        print(f"Chat: {chat_id}")
        print(f"Title: {title[:80]}")
        print("=" * 70)

        cur.execute(
            "SELECT role, content FROM chat_messages WHERE chat_id = %s ORDER BY created_at",
            (chat_id,),
        )
        for role, content in cur.fetchall():
            print(f"\n[{role.upper()}]")
            print(content[:400])
            if len(content) > 400:
                print("...")
