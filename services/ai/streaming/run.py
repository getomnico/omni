"""Redis transport layer for the chat streaming pipeline.

Owns the run-lifecycle primitives: the producer task registry, Redis
stream/lock/cancel keys, the background producer that writes every SSE event to
a Redis Stream, and the consumer that tails the stream for SSE delivery to
clients.

All module-level state (``_run_tasks_by_chat``) is consolidated here — the
process-local task registry used for cross-worker best-effort in-process
cancellation.
"""

from __future__ import annotations

import asyncio
import json
import logging

from streaming.persist import (
    EndOfStreamReason,
    StreamErrorEvent,
    end_of_stream,
    persist_and_transform,
    sse_event,
    sse_event_type,
    stream_error_event,
)

logger = logging.getLogger(__name__)


# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

SSE_HEADERS = {"Cache-Control": "no-cache", "Connection": "keep-alive"}

_STREAM_HEARTBEAT_MS = 15000  # idle ping interval (keeps proxies from timing out)
_RUN_LOCK_TTL = (
    300  # seconds; refreshed on every produced event and by the heartbeat below
)
_LOCK_REFRESH_INTERVAL = 60  # seconds; independent of event production, so a long
# silent gap in the agent loop (e.g. a slow tool call with no intermediate SSE
# events) can't let the lock expire while the producer is still running.
_STREAM_TTL = 300  # seconds a finished stream stays replayable
_STREAM_MAXLEN = 5000  # cap buffered events per run
_CANCEL_TTL = 300
_CANCEL_CHECK_INTERVAL_SECONDS = 1.0


# ---------------------------------------------------------------------------
# Producer task registry  (process-local; cross-worker best-effort only)
# ---------------------------------------------------------------------------

_run_tasks_by_chat: dict[str, asyncio.Task] = {}


def get_producer_task(chat_id: str) -> asyncio.Task | None:
    return _run_tasks_by_chat.get(chat_id)


def set_producer_task(chat_id: str, task: asyncio.Task) -> None:
    _run_tasks_by_chat[chat_id] = task


def clear_producer_task(chat_id: str, task: asyncio.Task) -> None:
    if _run_tasks_by_chat.get(chat_id) is task:
        del _run_tasks_by_chat[chat_id]


# ---------------------------------------------------------------------------
# Redis key helpers
# ---------------------------------------------------------------------------


def stream_key(chat_id: str) -> str:
    return f"chat:stream:{chat_id}"


def run_lock_key(chat_id: str) -> str:
    return f"chat:runlock:{chat_id}"


def cancel_key(chat_id: str) -> str:
    return f"chat:cancel:{chat_id}"


# ---------------------------------------------------------------------------
# Run-lock management
# ---------------------------------------------------------------------------


async def is_run_cancelled(redis_client, chat_id: str) -> bool:
    """Check whether the Redis cancel flag is set."""
    if redis_client is None:
        return False
    try:
        return bool(await redis_client.exists(cancel_key(chat_id)))
    except Exception:
        return False


async def _refresh_lock_periodically(redis_client, lock_key):
    """Keep the run lock alive independently of event production, so a long
    silent gap in the agent loop doesn't let it expire mid-run."""
    while True:
        await asyncio.sleep(_LOCK_REFRESH_INTERVAL)
        await redis_client.expire(lock_key, _RUN_LOCK_TTL)


# ---------------------------------------------------------------------------
# Producer
# ---------------------------------------------------------------------------


async def run_producer(redis_client, chat_id, gen, messages_repo, parent_id):
    """Background task: drive the agent loop to completion independently of any
    client connection, buffering every SSE event in a Redis Stream."""
    sk = stream_key(chat_id)
    lk = run_lock_key(chat_id)
    refresh_task = asyncio.create_task(_refresh_lock_periodically(redis_client, lk))
    try:
        async for event_str in persist_and_transform(
            gen, chat_id, messages_repo, parent_id
        ):
            await redis_client.xadd(
                sk,
                {"e": event_str},
                maxlen=_STREAM_MAXLEN,
                approximate=True,
            )
            await redis_client.expire(lk, _RUN_LOCK_TTL)
    except asyncio.CancelledError:
        # Explicit Stop cancelled this producer task.  Emit a terminal event so
        # any still-attached consumer ends cleanly instead of seeing the lock
        # disappear and reporting "Generation ended unexpectedly".
        try:
            await redis_client.xadd(sk, {"e": end_of_stream(EndOfStreamReason.STOPPED)})
        except Exception:
            pass
        raise
    except Exception as e:
        logger.error(f"Producer failed for chat {chat_id}: {e}", exc_info=True)
        try:
            await redis_client.xadd(
                sk,
                {"e": sse_event("stream_error", stream_error_event(e))},
            )
        except Exception:
            pass
    finally:
        refresh_task.cancel()
        try:
            await refresh_task
        except asyncio.CancelledError:
            pass
        for coro in (
            redis_client.expire(sk, _STREAM_TTL),
            redis_client.delete(lk),
            redis_client.delete(cancel_key(chat_id)),
        ):
            try:
                await coro
            except Exception:
                pass


# ---------------------------------------------------------------------------
# Consumer
# ---------------------------------------------------------------------------


async def consume_run(redis_client, chat_id, start_id):
    """Thin consumer: tail the Redis Stream from ``start_id``, prefixing each
    event with its Redis id (SSE ``id:``) for Last-Event-ID resume.  Emits
    heartbeats while idle and a terminal event if the producer vanished.

    Important id: invariant: no producer may template an ``id:`` line into its
    own event body because this function prepends ``id: {entry_id}`` (the Redis
    stream entry id) unconditionally.  Heartbeats and synthetic terminal events
    intentionally lack an id so they don't advance the resume position.
    """
    sk = stream_key(chat_id)
    lk = run_lock_key(chat_id)
    last = start_id or "0"
    while True:
        resp = await redis_client.xread(
            {sk: last}, block=_STREAM_HEARTBEAT_MS, count=200
        )
        if resp:
            for _key, entries in resp:
                for entry_id, fields in entries:
                    last = entry_id
                    event_str = fields.get("e", "")
                    yield f"id: {entry_id}\n{event_str}"
                    if sse_event_type(event_str) in ("end_of_stream", "stream_error"):
                        return
            continue
        # Idle: no new events within the heartbeat window.
        if not await redis_client.exists(sk):
            if await redis_client.exists(lk):
                # Producer just started and hasn't written its first event yet.
                yield sse_event("heartbeat", {})
                continue
            yield "event: not_resumable\ndata: \n\n"
            return
        if not await redis_client.exists(lk):
            # Producer is gone but never wrote a terminal event we forwarded.
            yield sse_event(
                "stream_error",
                StreamErrorEvent(message="Generation ended unexpectedly."),
            )
            return
        yield sse_event("heartbeat", {})
