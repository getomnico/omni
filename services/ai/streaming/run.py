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
from dataclasses import dataclass
from typing import Literal, TypedDict, cast

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
_RUN_LOCK_TTL = 300  # seconds; refreshed on every produced event and by the heartbeat below
_LOCK_REFRESH_INTERVAL = 60  # seconds; independent of event production, so a long
# silent gap in the agent loop (e.g. a slow tool call with no intermediate SSE
# events) can't let the lock expire while the producer is still running.
_STREAM_TTL = 300  # seconds a finished stream stays replayable
_STREAM_MAXLEN = 5000  # cap buffered events per run
_CANCEL_TTL = 300
_STEERING_TTL = 300
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


def _chat_key(kind: str, chat_id: str) -> str:
    """Build every chat-scoped Redis key in one place."""
    return f"chat:{kind}:{chat_id}"


def stream_key(chat_id: str) -> str:
    return _chat_key("stream", chat_id)


def run_lock_key(chat_id: str) -> str:
    return _chat_key("runlock", chat_id)


def cancel_key(chat_id: str) -> str:
    return _chat_key("cancel", chat_id)


def steering_queue_key(chat_id: str) -> str:
    return _chat_key("steering", chat_id)


def steering_dedupe_key(chat_id: str) -> str:
    return _chat_key("steering-dedupe", chat_id)


class SteeringTextBlock(TypedDict):
    type: Literal["text"]
    text: str


class SteeringUploadSource(TypedDict):
    type: Literal["omni_upload"]
    upload_id: str


class SteeringMentionSource(TypedDict):
    type: Literal["omni_mention"]
    document_id: str
    title: str
    source_type: str
    content_type: str


class SteeringDocumentBlock(TypedDict):
    type: Literal["document"]
    source: SteeringUploadSource | SteeringMentionSource


class SteeringMessage(TypedDict):
    role: Literal["user"]
    content: str | list[SteeringTextBlock | SteeringDocumentBlock]


@dataclass(frozen=True)
class SteeringQueueEntry:
    client_message_id: str
    message: SteeringMessage

    @classmethod
    def from_json(cls, raw: str) -> SteeringQueueEntry:
        payload = json.loads(raw)
        client_message_id = payload.get("client_message_id")
        message = payload.get("message")
        if not isinstance(client_message_id, str) or not client_message_id:
            raise ValueError("Steering queue entry has no client_message_id")
        if not isinstance(message, dict) or message.get("role") != "user":
            raise ValueError("Steering queue entry has an invalid user message")
        content = message.get("content")
        if not isinstance(content, (str, list)):
            raise ValueError("Steering queue entry has invalid content")
        return cls(
            client_message_id=client_message_id,
            message=cast(SteeringMessage, message),
        )

    def to_json(self) -> str:
        return json.dumps(
            {"client_message_id": self.client_message_id, "message": self.message},
            separators=(",", ":"),
        )


_ENQUEUE_STEERING_SCRIPT = """
local existing = redis.call('HGET', KEYS[3], ARGV[1])
if existing then
  if string.sub(existing, 1, 10) == 'persisted:' then
    return {3, existing}
  end
  return {2, existing}
end
if redis.call('GET', KEYS[1]) ~= 'open' then
  return {0, ''}
end
redis.call('RPUSH', KEYS[2], ARGV[2])
redis.call('HSET', KEYS[3], ARGV[1], 'pending')
redis.call('EXPIRE', KEYS[2], ARGV[3])
redis.call('EXPIRE', KEYS[3], ARGV[3])
return {1, 'pending'}
"""

_CLOSE_STEERING_SCRIPT = """
if redis.call('GET', KEYS[1]) ~= 'open' then
  return 0
end
if redis.call('LLEN', KEYS[2]) > 0 then
  return 1
end
redis.call('SET', KEYS[1], 'closing', 'EX', ARGV[1])
return 2
"""

_ACK_STEERING_SCRIPT = """
local queued = redis.call('LINDEX', KEYS[1], 0)
if queued == ARGV[3] then
  redis.call('LPOP', KEYS[1])
end
if redis.call('LLEN', KEYS[1]) == 0 then
  redis.call('DEL', KEYS[1])
else
  redis.call('EXPIRE', KEYS[1], ARGV[4])
end
redis.call('HSET', KEYS[2], ARGV[1], 'persisted:' .. ARGV[2])
redis.call('EXPIRE', KEYS[2], ARGV[4])
return 1
"""


async def enqueue_steering_message(
    redis_client,
    chat_id: str,
    client_message_id: str,
    message: SteeringMessage,
) -> str:
    """Atomically accept one FIFO steering message while a run is open.

    Returns ``accepted`` for a new or idempotent retry and ``closing`` when the
    run has won the final-check race.
    """
    entry = SteeringQueueEntry(client_message_id, message)
    result = await redis_client.eval(
        _ENQUEUE_STEERING_SCRIPT,
        3,
        run_lock_key(chat_id),
        steering_queue_key(chat_id),
        steering_dedupe_key(chat_id),
        client_message_id,
        entry.to_json(),
        str(_STEERING_TTL),
    )
    result_code = int(result[0])
    if result_code == 3:
        persisted = result[1]
        if isinstance(persisted, bytes):
            persisted = persisted.decode()
        if not isinstance(persisted, str) or not persisted.startswith("persisted:"):
            raise ValueError("Redis returned an invalid persisted steering result")
        return persisted
    return "accepted" if result_code in (1, 2) else "closing"


async def peek_steering_message(redis_client, chat_id: str) -> SteeringQueueEntry | None:
    raw = await redis_client.lindex(steering_queue_key(chat_id), 0)
    if raw is None:
        return None
    return SteeringQueueEntry.from_json(raw)


async def steering_queue_has_pending(redis_client, chat_id: str) -> bool:
    return bool(await redis_client.llen(steering_queue_key(chat_id)))


async def close_run_if_steering_empty(redis_client, chat_id: str) -> bool:
    """Close the run only if the FIFO is empty in the same Redis operation."""
    result = await redis_client.eval(
        _CLOSE_STEERING_SCRIPT,
        2,
        run_lock_key(chat_id),
        steering_queue_key(chat_id),
        str(_RUN_LOCK_TTL),
    )
    return int(result) == 2


async def acknowledge_steering_message(
    redis_client,
    chat_id: str,
    entry: SteeringQueueEntry,
    persisted_message_id: str,
) -> None:
    await redis_client.eval(
        _ACK_STEERING_SCRIPT,
        2,
        steering_queue_key(chat_id),
        steering_dedupe_key(chat_id),
        entry.client_message_id,
        persisted_message_id,
        entry.to_json(),
        str(_STEERING_TTL),
    )


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
            gen, chat_id, messages_repo, parent_id, redis_client=redis_client
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
            redis_client.expire(steering_queue_key(chat_id), _STEERING_TTL),
            redis_client.expire(steering_dedupe_key(chat_id), _STEERING_TTL),
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
        resp = await redis_client.xread({sk: last}, block=_STREAM_HEARTBEAT_MS, count=200)
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
