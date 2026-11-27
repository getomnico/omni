"""Helpers for async iterators whose resources require explicit closure."""

import logging
from collections.abc import AsyncIterator
from contextlib import asynccontextmanager
from typing import Protocol, TypeVar

logger = logging.getLogger(__name__)

T_co = TypeVar("T_co", covariant=True)


class ClosableAsyncIterator(AsyncIterator[T_co], Protocol[T_co]):
    async def aclose(self) -> None: ...


@asynccontextmanager
async def close_async_iterator(
    stream: ClosableAsyncIterator[T_co],
) -> AsyncIterator[ClosableAsyncIterator[T_co]]:
    """Close an owned iterator, retaining iteration failures over close failures."""
    try:
        yield stream
    except BaseException:
        try:
            await stream.aclose()
        except Exception:
            logger.warning("Failed to close async stream after iteration failed", exc_info=True)
        raise
    else:
        await stream.aclose()
