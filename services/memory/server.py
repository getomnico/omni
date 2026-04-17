"""Minimal mem0 REST server for Omni memory integration."""
import json
import logging
import os
import psycopg
import sqlite3
from contextlib import asynccontextmanager
from typing import Any

from fastapi import FastAPI
from fastapi.concurrency import run_in_threadpool
from fastapi.responses import JSONResponse
from mem0 import Memory
from opentelemetry import trace
from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter
from opentelemetry.instrumentation.fastapi import FastAPIInstrumentor
from opentelemetry.instrumentation.httpx import HTTPXClientInstrumentor
from opentelemetry.sdk.resources import Resource, SERVICE_NAME, SERVICE_VERSION, DEPLOYMENT_ENVIRONMENT
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import BatchSpanProcessor
from pydantic import BaseModel

logger = logging.getLogger(__name__)


def _init_telemetry(app: FastAPI) -> None:
    otlp_endpoint = os.getenv("OTEL_EXPORTER_OTLP_ENDPOINT")
    resource = Resource(attributes={
        SERVICE_NAME: "omni-memory",
        SERVICE_VERSION: os.getenv("SERVICE_VERSION", "0.1.0"),
        DEPLOYMENT_ENVIRONMENT: os.getenv("OTEL_DEPLOYMENT_ENVIRONMENT", "development"),
    })
    provider = TracerProvider(resource=resource)
    if otlp_endpoint:
        provider.add_span_processor(
            BatchSpanProcessor(OTLPSpanExporter(endpoint=f"{otlp_endpoint}/v1/traces"))
        )
    trace.set_tracer_provider(provider)
    FastAPIInstrumentor.instrument_app(app)
    HTTPXClientInstrumentor().instrument()

_memory: Memory | None = None
_db_config: dict = {}


def _load_memory() -> Memory:
    with open("/tmp/mem0_config.json") as f:
        config = json.load(f)
    return Memory.from_config(config)


@asynccontextmanager
async def lifespan(app: FastAPI):
    global _memory, _db_config
    _memory = await run_in_threadpool(_load_memory)
    with open("/tmp/mem0_config.json") as f:
        _db_config = json.load(f)["vector_store"]["config"]
    yield


app = FastAPI(lifespan=lifespan)
_init_telemetry(app)


class AddRequest(BaseModel):
    messages: list[dict[str, Any]]
    user_id: str


class SearchRequest(BaseModel):
    query: str
    user_id: str
    top_k: int = 5


def _purge_user_across_all_collections(user_id: str) -> int:
    """Delete user's rows from every mem0_memories* table.

    The entrypoint creates a per-embedder fingerprinted collection
    (mem0_memories_<hash>). When the embedder changes, the old collection
    stays in postgres. This function clears the user from all of them so
    Delete All is truly complete.
    """
    try:
        with psycopg.connect(
            host=_db_config["host"],
            port=_db_config.get("port", 5432),
            dbname=_db_config["dbname"],
            user=_db_config["user"],
            password=_db_config["password"],
        ) as conn:
            tables = conn.execute(
                "SELECT tablename FROM pg_tables "
                "WHERE schemaname = 'public' AND tablename LIKE 'mem0_memories%'",
            ).fetchall()

            total = 0
            for (table,) in tables:
                result = conn.execute(
                    f"DELETE FROM {table} WHERE payload->>'user_id' = %s",
                    (user_id,),
                )
                total += result.rowcount
            conn.commit()
        return total
    except Exception as e:
        logger.warning(f"Multi-collection purge failed for user {user_id}: {e}")
        return 0


@app.get("/health")
async def health():
    try:
        await run_in_threadpool(
            lambda: psycopg.connect(
                host=_db_config["host"],
                port=_db_config.get("port", 5432),
                dbname=_db_config["dbname"],
                user=_db_config["user"],
                password=_db_config["password"],
            ).close()
        )
    except Exception as e:
        logger.warning(f"Health check DB probe failed: {e}")
        return JSONResponse(status_code=503, content={"status": "unhealthy", "reason": str(e)})
    return {"status": "ok"}


@app.post("/memories")
async def add_memories(req: AddRequest):
    # mem0's parse_vision_messages crashes on list content when no vision LLM
    # is configured — it calls get_image_description(msg, llm=None) for ANY list
    # content, not just actual images. Flatten to plain strings first.
    sanitized = []
    for m in req.messages:
        content = m.get("content", "")
        if isinstance(content, list):
            content = " ".join(
                b.get("text", "") for b in content
                if isinstance(b, dict) and b.get("type") == "text"
            )
        if content and isinstance(content, str):
            sanitized.append({"role": m.get("role", "user"), "content": content})
    if not sanitized:
        return {}
    return await run_in_threadpool(_memory.add, sanitized, user_id=req.user_id)


@app.post("/search")
async def search_memories(req: SearchRequest):
    # mem0 v2 requires entity IDs via filters={} rather than as top-level kwargs.
    results = await run_in_threadpool(
        _memory.search, req.query, top_k=req.top_k, filters={"user_id": req.user_id}
    )
    if isinstance(results, dict):
        return results
    return {"results": results}


@app.get("/memories")
async def list_memories(user_id: str):
    # mem0 v2 requires entity IDs via filters={} rather than as top-level kwargs.
    # Shows only the active embedder's collection — consistent with what search uses.
    results = await run_in_threadpool(
        _memory.get_all, filters={"user_id": user_id}
    )
    if isinstance(results, dict):
        return results
    return {"results": results}


@app.delete("/memories/{memory_id}")
async def delete_memory(memory_id: str):
    await run_in_threadpool(_memory.delete, memory_id)
    return {"status": "deleted"}


def _delete_all_user_memories(user_id: str) -> int:
    _memory.delete_all(user_id=user_id)

    # Clear the messages ring-buffer that mem0 feeds into the LLM extraction
    # prompt on the next add() call — without this, deleted facts get re-extracted.
    with sqlite3.connect(_memory.db.db_path) as conn:
        conn.execute(
            "DELETE FROM messages WHERE session_scope = ?",
            (f"user_id={user_id}",),
        )

    return _purge_user_across_all_collections(user_id)


@app.delete("/memories")
async def delete_all_memories(user_id: str):
    deleted = await run_in_threadpool(_delete_all_user_memories, user_id)
    return {"status": "deleted", "rows_deleted": deleted}
