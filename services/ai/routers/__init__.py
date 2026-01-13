from .chat import router as chat_router
from .health import router as health_router
from .embeddings import router as embeddings_router
from .prompts import router as prompts_router

__all__ = [
    "chat_router",
    "health_router",
    "embeddings_router",
    "prompts_router",
]
