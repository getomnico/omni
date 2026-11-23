"""Small bounded-read and cleanup utilities for provider streams."""

import asyncio
import logging
from collections.abc import AsyncIterator, Awaitable
from typing import TypeVar, cast

T = TypeVar("T")

FINAL_USAGE_DRAIN_SECONDS = 0.25
STREAM_CLEANUP_TIMEOUT_SECONDS = 0.25
logger = logging.getLogger(__name__)
_detached_tasks: set[asyncio.Future[object]] = set()


def _observe_task(task: asyncio.Future[T]) -> None:
    _detached_tasks.discard(cast(asyncio.Future[object], task))
    if task.cancelled():
        return
    try:
        task.result()
    except Exception as error:
        logger.warning(
            "Detached provider stream task failed",
            exc_info=(type(error), error, error.__traceback__),
        )


def detach_task(task: asyncio.Future[T]) -> None:
    _detached_tasks.add(cast(asyncio.Future[object], task))
    task.add_done_callback(_observe_task)
    task.cancel()


async def next_stream_item(iterator: AsyncIterator[T], *, deadline: float | None) -> T:
    """Read one item, optionally bounding the wait by an absolute loop deadline."""
    task: asyncio.Future[T] = asyncio.ensure_future(iterator.__anext__())
    if deadline is None:
        return await task

    remaining = deadline - asyncio.get_running_loop().time()
    if remaining <= 0:
        detach_task(task)
        raise TimeoutError
    try:
        done, _ = await asyncio.wait((task,), timeout=remaining)
    except BaseException:
        detach_task(task)
        raise
    if not done:
        detach_task(task)
        raise TimeoutError
    return task.result()


async def bounded_task(
    task: asyncio.Future[object], *, timeout: float = STREAM_CLEANUP_TIMEOUT_SECONDS
) -> bool:
    """Wait briefly for a task and log its failure if it finishes."""
    try:
        done, _ = await asyncio.wait((task,), timeout=timeout)
    except BaseException:
        detach_task(task)
        raise
    if not done:
        logger.warning("Provider stream task exceeded %.3fs cleanup wait", timeout)
        detach_task(task)
        return False
    try:
        task.result()
    except Exception as error:
        logger.warning(
            "Provider stream cleanup failed",
            exc_info=(type(error), error, error.__traceback__),
        )
    return True


async def bounded_cleanup(
    awaitable: Awaitable[object], *, timeout: float = STREAM_CLEANUP_TIMEOUT_SECONDS
) -> None:
    """Wait briefly for cleanup, then cancel and observe any late task result."""
    task = asyncio.ensure_future(awaitable)
    await bounded_task(task, timeout=timeout)
