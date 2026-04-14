from .chat import router as chat_router
from .health import router as health_router
from .embeddings import router as embeddings_router
from .prompts import router as prompts_router
from .model_providers import router as model_providers_router
from .agents import router as agents_router
from .uploads import router as uploads_router
from .usage import router as usage_router

__all__ = [
    "chat_router",
    "health_router",
    "embeddings_router",
    "prompts_router",
    "model_providers_router",
    "agents_router",
    "uploads_router",
    "usage_router",
]
