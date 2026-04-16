#!/bin/sh
set -e

echo "Memory service: discovering LLM config from AI service at ${AI_SERVICE_URL}..."

# Retry loop: AI service may not be ready yet at startup
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
    echo "ERROR: Could not fetch LLM config from AI service after $MAX_ATTEMPTS attempts" >&2
    exit 1
fi

# Parse JSON fields using Python (always available in this image)
PROVIDER=$(echo "$CONFIG" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('provider',''))")
MODEL=$(echo "$CONFIG" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('model',''))")
API_KEY=$(echo "$CONFIG" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('api_key') or '')")
BASE_URL=$(echo "$CONFIG" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('base_url') or '')")

echo "Memory service: provider=$PROVIDER model=$MODEL"

# Export mem0-compatible env vars based on provider type
case "$PROVIDER" in
    anthropic)
        export MEM0_LLM_PROVIDER=anthropic
        export ANTHROPIC_API_KEY="$API_KEY"
        export MEM0_LLM_MODEL="$MODEL"
        ;;
    openai|openai_compatible)
        export MEM0_LLM_PROVIDER=openai
        export OPENAI_API_KEY="$API_KEY"
        export MEM0_LLM_MODEL="$MODEL"
        if [ -n "$BASE_URL" ]; then
            export OPENAI_BASE_URL="$BASE_URL"
        fi
        ;;
    gemini)
        export MEM0_LLM_PROVIDER=gemini
        export GOOGLE_API_KEY="$API_KEY"
        export MEM0_LLM_MODEL="$MODEL"
        ;;
    bedrock)
        export MEM0_LLM_PROVIDER=bedrock
        export MEM0_LLM_MODEL="$MODEL"
        ;;
    *)
        echo "WARNING: Unknown provider '$PROVIDER', mem0 will use its own defaults" >&2
        ;;
esac

# mem0 pgvector config
export MEM0_VECTOR_STORE_PROVIDER=pgvector
export MEM0_VECTOR_STORE_HOST="${POSTGRES_HOST}"
export MEM0_VECTOR_STORE_PORT="${POSTGRES_PORT:-5432}"
export MEM0_VECTOR_STORE_DB="${POSTGRES_DB}"
export MEM0_VECTOR_STORE_USER="${POSTGRES_USER}"
export MEM0_VECTOR_STORE_PASSWORD="${POSTGRES_PASSWORD}"

# Disable graph memory (Phase 1)
export MEM0_GRAPH_STORE_PROVIDER=none

exec python -m uvicorn "mem0.server.main:app" --host 0.0.0.0 --port 8888
