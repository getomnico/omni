"""Typed application state for FastAPI app.state"""

from dataclasses import dataclass, field

import redis.asyncio as aioredis

from embeddings import EmbeddingProvider
from providers import LLMProvider
from tools import SearcherTool
from storage import ContentStorage


@dataclass
class AppState:
    """Typed application state for FastAPI app.state.

    This class provides proper type hints for IDE autocompletion
    when accessing app.state attributes.
    """

    embedding_provider: EmbeddingProvider | None = None
    models: dict[str, LLMProvider] = field(default_factory=dict)
    default_model_id: str | None = None
    searcher_tool: SearcherTool | None = None
    content_storage: ContentStorage | None = None
    redis_client: aioredis.Redis | None = None
