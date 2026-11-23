import asyncio
import logging

import pytest

from providers.stream_utils import bounded_cleanup, next_stream_item

pytestmark = pytest.mark.unit


@pytest.mark.asyncio
async def test_next_stream_item_cancels_pending_read_when_parent_is_cancelled():
    started = asyncio.Event()
    release = asyncio.Event()
    cancelled = asyncio.Event()
    finished = asyncio.Event()

    class PendingIterator:
        def __aiter__(self):
            return self

        async def __anext__(self):
            started.set()
            try:
                await release.wait()
                return "unexpected"
            except asyncio.CancelledError:
                cancelled.set()
                raise
            finally:
                finished.set()

    task = asyncio.create_task(
        next_stream_item(
            PendingIterator(),
            deadline=asyncio.get_running_loop().time() + 10,
        )
    )
    try:
        await started.wait()
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await task
        assert cancelled.is_set()
    finally:
        release.set()
        await asyncio.wait_for(finished.wait(), timeout=1.0)


@pytest.mark.asyncio
async def test_bounded_cleanup_logs_cleanup_failures(caplog):
    async def fail_cleanup():
        raise RuntimeError("stream close failed")

    with caplog.at_level(logging.WARNING, logger="providers.stream_utils"):
        await bounded_cleanup(fail_cleanup())

    assert "stream close failed" in caplog.text
