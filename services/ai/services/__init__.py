"""Lazy exports for service-layer helpers.

Keep package import side-effect-light so submodules such as ``services.citations``
can be imported by focused unit tests without initializing provider/config code.
"""

__all__ = [
    "EmbeddingQueueService",
    "initialize_providers",
    "shutdown_providers",
    "start_batch_processor",
    "ConversationCompactor",
]


def __getattr__(name: str) -> object:
    if name == "EmbeddingQueueService":
        from services.embedding_queue import EmbeddingQueueService

        return EmbeddingQueueService
    if name == "ConversationCompactor":
        from services.compaction import ConversationCompactor

        return ConversationCompactor
    if name in {"initialize_providers", "shutdown_providers", "start_batch_processor"}:
        from services.providers import (
            initialize_providers,
            shutdown_providers,
            start_batch_processor,
        )

        lifecycle_fns = {
            "initialize_providers": initialize_providers,
            "shutdown_providers": shutdown_providers,
            "start_batch_processor": start_batch_processor,
        }
        return lifecycle_fns[name]
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")
