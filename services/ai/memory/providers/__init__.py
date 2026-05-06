"""Concrete MemoryProvider implementations.

Provider selection is a deployment-time backend choice (env var
`MEMORY_PROVIDER`), not a user-facing setting. The registry below maps
the env value to a small zero-arg async factory that constructs the
provider.

To add a new provider:
  1. Implement `MemoryProvider` under `memory/providers/<your_name>/`.
  2. Add a factory entry to `_REGISTRY` returning that implementation.
"""

import logging
from typing import Awaitable, Callable

from memory.provider import MemoryProvider
from memory.providers.mem0 import build_mem0_provider

logger = logging.getLogger(__name__)


_REGISTRY: dict[str, Callable[[object], Awaitable[MemoryProvider | None]]] = {
    "mem0": build_mem0_provider,
}


async def build_memory_provider(name: str, app_state) -> MemoryProvider | None:
    """Construct the provider chosen by `MEMORY_PROVIDER`.

    Returns None on construction failure — memory is non-critical
    infrastructure and the AI service must boot regardless.
    """
    factory = _REGISTRY.get(name)
    if factory is None:
        if not _REGISTRY:
            raise ValueError(
                f"Unknown MEMORY_PROVIDER={name!r}; no providers are registered"
            )
        valid = ", ".join(sorted(_REGISTRY))
        raise ValueError(f"Unknown MEMORY_PROVIDER={name!r}. Valid options: {valid}")
    return await factory(app_state)
