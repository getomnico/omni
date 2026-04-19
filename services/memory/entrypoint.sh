#!/bin/sh
set -e

echo "Memory service: fetching config from ${AI_SERVICE_URL}..."

MAX_ATTEMPTS=30
ATTEMPT=0
while [ $ATTEMPT -lt $MAX_ATTEMPTS ]; do
    CONFIG=$(curl -sf "${AI_SERVICE_URL}/internal/memory/llm-config" 2>/dev/null || true)
    if [ -n "$CONFIG" ]; then
        break
    fi
    ATTEMPT=$((ATTEMPT + 1))
    echo "Waiting for AI service (attempt $ATTEMPT/$MAX_ATTEMPTS)..."
    sleep 2
done

if [ -z "$CONFIG" ]; then
    echo "ERROR: Could not fetch config after $MAX_ATTEMPTS attempts" >&2
    exit 1
fi

LLM_PROVIDER=$(echo "$CONFIG" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d['llm']['provider'])")
LLM_MODEL=$(echo "$CONFIG" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d['llm']['config']['model'])")
EMBED_PROVIDER=$(echo "$CONFIG" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d['embedder']['provider'])")
echo "Memory service: llm=$LLM_PROVIDER/$LLM_MODEL embedder=$EMBED_PROVIDER"

# Build /tmp/mem0_config.json by merging the API response with pgvector credentials.
# Pass CONFIG via env var so the heredoc Python script can read it cleanly.
export MEM0_RAW_CONFIG="$CONFIG"

python3 <<'PYEOF'
import hashlib, json, os, sys
import psycopg

api = json.loads(os.environ["MEM0_RAW_CONFIG"])

db_host     = os.environ["DATABASE_HOST"]
db_port     = int(os.environ.get("DATABASE_PORT", "5432"))
db_user     = os.environ["DATABASE_USERNAME"]
db_password = os.environ["DATABASE_PASSWORD"]
db_name     = os.environ["DATABASE_NAME"]

# Separate database for mem0 — keeps memory tables out of the main app schema.
# Falls back to <main_db>_mem0 if MEMORY_DATABASE_NAME is not set.
mem0_db = os.environ.get("MEMORY_DATABASE_NAME") or (db_name + "_mem0")

# Ensure the mem0 database exists.
# Requires CREATEDB privilege. In production with a restricted DB user,
# pre-create the database manually: CREATE DATABASE "<mem0_db>";
with psycopg.connect(
    host=db_host, port=db_port, dbname=db_name,
    user=db_user, password=db_password,
    autocommit=True,
) as conn:
    row = conn.execute(
        "SELECT 1 FROM pg_database WHERE datname = %s", (mem0_db,)
    ).fetchone()
    if not row:
        try:
            conn.execute(f'CREATE DATABASE "{mem0_db}"')
            print(f"Memory service: created database {mem0_db!r}")
        except Exception as e:
            print(
                f"ERROR: Cannot create database {mem0_db!r}: {e}\n"
                f"If the DB user lacks CREATEDB privilege, create it manually:\n"
                f"  CREATE DATABASE \"{mem0_db}\";\n"
                f"  GRANT ALL PRIVILEGES ON DATABASE \"{mem0_db}\" TO {db_user};",
                file=sys.stderr,
            )
            sys.exit(1)
    else:
        print(f"Memory service: using existing database {mem0_db!r}")

# Drop any mem0_memories* tables that were previously created in the main app db.
with psycopg.connect(
    host=db_host, port=db_port, dbname=db_name,
    user=db_user, password=db_password,
) as conn:
    tables = conn.execute(
        "SELECT tablename FROM pg_tables "
        "WHERE schemaname = 'public' AND tablename LIKE 'mem0_memories%'"
    ).fetchall()
    for (table,) in tables:
        conn.execute(f'DROP TABLE IF EXISTS "{table}"')
        print(f"Memory service: removed stale table {table!r} from {db_name!r}")
    conn.commit()

# Fingerprint collection name by embedder (provider+model+dims). Switching
# embedders creates a new collection instead of failing on dim mismatch.
embed_dims = api["embedder"]["config"].get("embedding_dims")
fp_str = "{provider}:{model}:{dims}".format(
    provider=api["embedder"]["provider"],
    model=api["embedder"]["config"].get("model", ""),
    dims=embed_dims or 0,
)
fp = hashlib.sha256(fp_str.encode()).hexdigest()[:12]
collection_name = f"mem0_memories_{fp}"
print(f"Memory service: collection={collection_name} ({fp_str})")

vector_store_config = {
    "host":            db_host,
    "port":            db_port,
    "dbname":          mem0_db,
    "user":            db_user,
    "password":        db_password,
    "collection_name": collection_name,
}
if embed_dims:
    vector_store_config["embedding_model_dims"] = embed_dims

config = {
    "vector_store": {"provider": "pgvector", "config": vector_store_config},
    "llm":          api["llm"],
    "embedder":     api["embedder"],
    "history_db_path": "/tmp/mem0_history.db",
}

with open("/tmp/mem0_config.json", "w") as f:
    json.dump(config, f, indent=2)
print("Memory service: config written")
PYEOF

exec uvicorn server:app --host 0.0.0.0 --port 8888
