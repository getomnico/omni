"""Chat-loop evaluation runner.

For each golden entry with a reference_answer, drive the production agent
loop (run_agent_loop) headlessly, then score the resulting (query, contexts,
response) tuples with RAGAS via _score_samples().

Requires:
  - DATABASE_*         — agent model is loaded from the platform's default
                         model in the DB (configured in the admin UI)
  - EVAL_SEARCHER_URL  — base URL for omni-searcher
  - EVAL_OPENAI_API_KEY  — judge model API key
  - EVAL_OPENAI_API_BASE — judge model base URL (optional;
                           defaults to https://api.openai.com)
  - EVAL_MODEL         — judge model

Usage:
    cd services/ai
    export EVAL_SEARCHER_URL=http://localhost:3001
    export EVAL_OPENAI_API_KEY=sk-...
    uv run python -m evaluation.runners.chat_loop_runner
"""

from __future__ import annotations

import asyncio
import logging
import os
import uuid
from pathlib import Path
from typing import Optional

import yaml
from dotenv import load_dotenv
from urllib.parse import quote_plus

# Default to the dev stack defined in docker/docker-compose.dev.yml — the
# Postgres container exposes 5432 on the host with these credentials. The
# runner talks to that same DB to load models and persist eval scores.
os.environ.setdefault("DATABASE_HOST", "localhost")
os.environ.setdefault("DATABASE_PORT", "5432")
os.environ.setdefault("DATABASE_USERNAME", "omni_dev")
os.environ.setdefault("DATABASE_PASSWORD", "omni_dev_password")
os.environ.setdefault("DATABASE_NAME", "omni_dev")

# config.py is shared with the AI service and requires several vars the eval
# runner never uses at runtime. Fill in harmless placeholders so the import
# chain (services.providers → config) succeeds.
os.environ.setdefault("PORT", "3003")
os.environ.setdefault("MODEL_PATH", "/tmp")
os.environ.setdefault("CONNECTOR_MANAGER_URL", "http://localhost:3004")
os.environ.setdefault("REDIS_URL", "redis://unused")  # load_models never touches Redis

# Pull in shared secrets from the repo-root .env (ENCRYPTION_KEY decrypts
# model-provider config rows in the DB — same key inside and outside the
# container). override=False so our setdefault values above stick.
_repo_root = Path(__file__).resolve().parents[4]
load_dotenv(_repo_root / ".env", override=False)

# db.connection prefers DATABASE_URL over the individual components. Rebuild
# it from DATABASE_* so the .env's DATABASE_URL (which may point at a
# different host/password) can't win over the vars above.
os.environ["DATABASE_URL"] = (
    f"postgresql://{quote_plus(os.environ['DATABASE_USERNAME'])}"
    f":{quote_plus(os.environ['DATABASE_PASSWORD'])}"
    f"@{os.environ['DATABASE_HOST']}:{os.environ['DATABASE_PORT']}"
    f"/{os.environ['DATABASE_NAME']}"
)

from anthropic.types import MessageParam  # noqa: E402
from agent_loop import LoopComplete, run_agent_loop  # noqa: E402
from db.documents import DocumentsRepository  # noqa: E402
from evaluation.config import EvalConfig  # noqa: E402
from evaluation.reporters.console import print_results  # noqa: E402
from evaluation.runners.runner import _score_samples  # noqa: E402
from prompts import build_chat_system_prompt  # noqa: E402
from providers import LLMProvider  # noqa: E402
from services.providers import load_models  # noqa: E402
from state import AppState  # noqa: E402
from storage import PostgresContentStorage  # noqa: E402
from tools import (  # noqa: E402
    DocumentToolHandler,
    SearcherTool,
    SearchToolHandler,
    ToolContext,
    ToolRegistry,
)
from config import (  # noqa: E402
    DEFAULT_MAX_TOKENS,
    DEFAULT_TEMPERATURE,
    DEFAULT_TOP_P,
)

logger = logging.getLogger(__name__)
logging.basicConfig(level=logging.INFO)


_READ_DOCUMENT_BANNER_PREFIXES = (
    "Document saved to workspace:",
    "Document not found:",
    "Failed to fetch file:",
    "Missing required parameter:",
    "read_document error:",
    "Unknown tool:",
)


def _extract_contexts(final_messages: list[MessageParam]) -> list[str]:
    """Pull document text out of tool_result blocks, mirroring what the
    production agent actually consumed.

    Two block shapes carry retrieval evidence:
      * search_result blocks (from search_documents) — contain a mix of
        bracketed metadata lines ([Document ID: ...], etc.) and the
        BM25 highlight text. Only the highlights are evidence.
      * plain text blocks (from read_document) — the full document body
        when under the size threshold. read_document also produces
        non-evidence status banners ("Document saved to workspace: ...",
        error messages) which must be excluded.
    """
    contexts: list[str] = []

    def _append_text(text: str) -> None:
        if not text or not text.strip():
            return
        stripped = text.lstrip()
        # Skip search_result metadata lines like "[Document ID: ...]".
        if stripped.startswith("["):
            return
        # Skip read_document status banners (sandbox saves, errors).
        if stripped.startswith(_READ_DOCUMENT_BANNER_PREFIXES):
            return
        contexts.append(text)

    for msg in final_messages:
        if msg.get("role") != "user":
            continue
        content = msg.get("content")
        if not isinstance(content, list):
            continue
        for block in content:
            if not isinstance(block, dict) or block.get("type") != "tool_result":
                continue
            # An errored tool result is not evidence the agent answered from.
            if block.get("is_error"):
                continue
            inner = block.get("content")
            if not isinstance(inner, list):
                continue
            for sub in inner:
                if not isinstance(sub, dict):
                    continue
                if sub.get("type") == "search_result":
                    for item in sub.get("content") or []:
                        if isinstance(item, dict) and item.get("type") == "text":
                            _append_text(item.get("text", ""))
                elif sub.get("type") == "text":
                    _append_text(sub.get("text", ""))
    return contexts


def _extract_response(final_messages: list[MessageParam]) -> str:
    last_assistant = None
    for msg in reversed(final_messages):
        if msg.get("role") == "assistant":
            last_assistant = msg
            break
    if last_assistant is None:
        return ""
    content = last_assistant.get("content")
    if isinstance(content, str):
        return content.strip()
    if not isinstance(content, list):
        return ""
    parts = []
    for block in content:
        if isinstance(block, dict) and block.get("type") == "text":
            parts.append(block.get("text", ""))
    return "".join(parts).strip()


async def _build_llm_provider() -> LLMProvider:
    """Resolve the platform's default chat model from the DB (same source the
    chat router uses)."""
    state = AppState()
    await load_models(state)
    if not state.models:
        raise RuntimeError("No models configured in DB — add one in admin UI first")
    if state.default_model_id and state.default_model_id in state.models:
        return state.models[state.default_model_id]
    logger.warning("No default model flagged in DB; falling back to first available")
    return next(iter(state.models.values()))


def _build_registry() -> ToolRegistry:
    """Build a registry mirroring the production chat agent's read-only path:
    search_documents + read_document. Without read_document the eval agent
    can't fetch full chunks, so the judge would see only BM25 highlights —
    which structurally penalizes faithfulness compared to production.

    SEARCHER_URL must already be exported in the environment (the runner
    sets it from EVAL_SEARCHER_URL before calling this). Sandbox/connector-
    manager URLs are intentionally omitted: text reads under the size
    threshold return content directly (the path the judge needs), and
    binary fetches are skipped.
    """
    registry = ToolRegistry()
    registry.register(SearchToolHandler(searcher_tool=SearcherTool()))
    registry.register(
        DocumentToolHandler(
            content_storage=PostgresContentStorage(),
            documents_repo=DocumentsRepository(),
        )
    )
    return registry


async def _run_one(
    *,
    entry: dict,
    config: EvalConfig,
    llm_provider: LLMProvider,
    registry: ToolRegistry,
    system_prompt: str,
) -> Optional[dict]:
    ctx = ToolContext(
        chat_id=config.eval_chat_id,
        user_id=config.eval_user_id,
        skip_permission_check=True,
        original_user_query=entry["query"],
    )
    messages: list[MessageParam] = [MessageParam(role="user", content=entry["query"])]

    final_messages: list[MessageParam] = []
    async for ev in run_agent_loop(
        llm_provider=llm_provider,
        messages=messages,
        system_prompt=system_prompt,
        tools=registry.get_all_tools(),
        registry=registry,
        tool_context=ctx,
        max_iterations=config.max_iterations,
        max_tokens=DEFAULT_MAX_TOKENS,
        temperature=DEFAULT_TEMPERATURE,
        top_p=DEFAULT_TOP_P,
    ):
        if isinstance(ev, LoopComplete):
            final_messages = ev.result.final_messages
            if ev.result.stopped_reason not in ("no_tool_calls", "max_iterations"):
                logger.warning(
                    f"Entry {entry['id']} stopped early: {ev.result.stopped_reason}"
                )

    contexts = _extract_contexts(final_messages)
    response = _extract_response(final_messages)
    if not contexts or not response:
        logger.warning(f"Entry {entry['id']} produced empty contexts/response — skipping")
        return None

    return {
        "id": entry["id"],
        "query": entry["query"],
        "reference_answer": entry["reference_answer"],
        "contexts": contexts,
        "response": response,
        "source": "chat_loop",
        "trace_id": str(uuid.uuid4()),
    }


async def run_chat_loop_evaluation(
    config: Optional[EvalConfig] = None,
) -> dict[str, float]:
    if config is None:
        config = EvalConfig.from_env()

    searcher_url = os.environ.get("EVAL_SEARCHER_URL", "").rstrip("/")
    if not searcher_url:
        raise RuntimeError("EVAL_SEARCHER_URL is not set")
    # SearcherClient reads SEARCHER_URL from the environment at construction
    # time, so bridge the eval-specific knob into it before building tools.
    os.environ["SEARCHER_URL"] = searcher_url

    golden_path = Path(config.golden_set_path)
    if not golden_path.exists():
        raise FileNotFoundError(f"Golden set not found: {golden_path}")

    with open(golden_path) as f:
        golden = yaml.safe_load(f) or []
    entries = [e for e in golden if e.get("reference_answer") and e.get("query")]
    logger.info(
        f"Driving chat loop on {len(entries)} golden entries via {searcher_url}"
    )

    llm_provider = await _build_llm_provider()
    logger.info(f"Using agent model: {llm_provider.model_name}")
    registry = _build_registry()
    system_prompt = build_chat_system_prompt(
        sources=[],
        connector_actions=[],
        user_name=None,
        user_email=None,
    )

    sem = asyncio.Semaphore(int(os.environ.get("EVAL_LOOP_CONCURRENCY", "2")))

    async def _bounded(entry):
        async with sem:
            return await _run_one(
                entry=entry,
                config=config,
                llm_provider=llm_provider,
                registry=registry,
                system_prompt=system_prompt,
            )

    raw_samples = await asyncio.gather(*[_bounded(e) for e in entries])
    samples = [s for s in raw_samples if s is not None]

    if not samples:
        logger.warning("No samples with usable response/contexts — is the searcher running and indexed?")
        return {}

    from db.connection import get_db_pool
    pool = await get_db_pool()
    logger.info(f"Scoring {len(samples)} samples with RAGAS...")
    scores = await _score_samples(samples, config, pool)

    thresholds = {
        "faithfulness": config.faithfulness_threshold,
        "context_recall": config.context_recall_threshold,
    }
    print_results(scores, thresholds)
    return scores


if __name__ == "__main__":
    asyncio.run(run_chat_loop_evaluation())
