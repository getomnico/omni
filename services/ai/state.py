"""Typed application state for FastAPI app.state"""

from dataclasses import dataclass

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
    llm_provider: LLMProvider | None = None
    searcher_tool: SearcherTool | None = None
    content_storage: ContentStorage | None = None
