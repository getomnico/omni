from __future__ import annotations

import asyncio
import hashlib
import json
import logging
import math
import os
import tempfile
from collections.abc import AsyncIterator, Awaitable, Callable
from contextlib import AsyncExitStack, asynccontextmanager
from dataclasses import dataclass, field
from datetime import timedelta
from pathlib import Path
from typing import Any, TypeVar

from mcp.client.session import ClientSession
from mcp.client.stdio import StdioServerParameters, stdio_client
from pydantic import AnyUrl

from .models import (
    ActionDefinition,
    ActionResponse,
    McpPromptArgument,
    McpPromptDefinition,
    McpResourceDefinition,
)

logger = logging.getLogger(__name__)

T = TypeVar("T")

MCP_AUTH_STATUS_FILE_ENV = "OMNI_MCP_AUTH_STATUS_FILE"
MCP_POOL_SOURCE_ID_ENV = "OMNI_MCP_POOL_SOURCE_ID"
MCP_POOL_USER_ID_ENV = "OMNI_MCP_POOL_USER_ID"
MCP_AUTH_REQUIRED_MESSAGE = "MCP authentication required"
MCP_PROCESS_BUSY_MESSAGE = "MCP process capacity is temporarily unavailable"
MCP_PROCESS_CLOSED_MESSAGE = "MCP process was closed or replaced before the request ran"


class McpProcessCapacityError(RuntimeError):
    """No idle Salesforce MCP process can be evicted to free capacity."""


class McpProcessClosedError(RuntimeError):
    """The pooled MCP process expired or was replaced before the request ran."""


@dataclass
class _PersistentRequest:
    callback: Callable[[ClientSession], Awaitable[Any]]
    future: asyncio.Future[Any]
    timeout_seconds: float
    started: bool = False


@dataclass
class _PersistentProcess:
    key: str
    fingerprint: str
    env: dict[str, str]
    queue: asyncio.Queue[_PersistentRequest | None] = field(default_factory=asyncio.Queue)
    task: asyncio.Task[None] | None = None
    ready: asyncio.Future[None] | None = None
    busy: bool = False
    closing: bool = False
    startup_waiters: int = 0
    last_used: float = 0.0
    auth_status_file: str | None = None
    current_request: _PersistentRequest | None = None


@dataclass(frozen=True)
class StdioMcpServer:
    """Configuration for an MCP server reached via stdio (subprocess)."""

    command: str
    args: list[str] = field(default_factory=list)
    env: dict[str, str] | None = None
    cwd: str | None = None
    persistent: bool = False


@dataclass(frozen=True)
class HttpMcpServer:
    """Configuration for a remote MCP server reached via Streamable HTTP."""

    url: str
    headers: dict[str, str] = field(default_factory=dict)
    timeout_seconds: float = 30.0
    sse_read_timeout_seconds: float = 300.0


McpServer = StdioMcpServer | HttpMcpServer


class McpAdapter:
    """Bridges an external MCP server into Omni's connector protocol.

    Supports two transports:
    - stdio: spawns the MCP server as a subprocess and talks JSON-RPC over
      stdin/stdout. Works with MCP servers written in any language.
    - Streamable HTTP: connects to a remote MCP endpoint per the MCP spec.

    By default every operation opens a fresh session and tears it down
    afterwards. Tool/resource/prompt definitions are cached in memory after
    the first successful discovery so manifest builds don't require live
    auth.

    Stdio servers can opt into persistent process reuse (``persistent=True``
    on ``StdioMcpServer``, or ``persistent_stdio=True`` here). In that mode
    one MCP subprocess is kept per explicit (source, user) identity supplied
    through the ``OMNI_MCP_POOL_SOURCE_ID`` / ``OMNI_MCP_POOL_USER_ID`` env
    vars; calls for the same identity are serialized through the live
    process, credential rotation replaces it, and the pool enforces a
    process cap with an idle timeout. Pool settings are only read and
    validated for this opt-in mode; without the pool identity env the adapter
    falls back to the per-call behavior, and all other MCP behavior is
    unchanged.
    """

    def __init__(
        self,
        server: McpServer,
        *,
        persistent_stdio: bool = False,
        process_limit: int | None = None,
        idle_timeout_seconds: float | None = None,
    ) -> None:
        self._server = server
        self._persistent_stdio = isinstance(server, StdioMcpServer) and (
            persistent_stdio or server.persistent
        )
        # Pool configuration is only meaningful for opt-in persistent mode;
        # parsing/validation of persistent-only env must not affect plain
        # stdio or Streamable HTTP adapters.
        self._process_limit = 0
        self._idle_timeout_seconds = 0.0
        if self._persistent_stdio:
            self._process_limit = (
                int(os.environ.get("OMNI_MCP_MAX_PROCESSES", "16"))
                if process_limit is None
                else process_limit
            )
            self._idle_timeout_seconds = (
                float(os.environ.get("OMNI_MCP_IDLE_TIMEOUT_SECONDS", "1800"))
                if idle_timeout_seconds is None
                else idle_timeout_seconds
            )
            if self._process_limit < 1:
                raise ValueError("MCP process limit must be positive")
            if not math.isfinite(self._idle_timeout_seconds) or self._idle_timeout_seconds <= 0:
                raise ValueError("MCP idle timeout must be finite and positive")
        self._persistent_processes: dict[str, _PersistentProcess] = {}
        self._all_processes: dict[int, _PersistentProcess] = {}
        self._process_lock = asyncio.Lock()
        self._closed = False
        self._cached_actions: list[ActionDefinition] | None = None
        self._cached_resources: list[McpResourceDefinition] | None = None
        self._cached_prompts: list[McpPromptDefinition] | None = None

    @asynccontextmanager
    async def _open_session(
        self,
        env: dict[str, str] | None,
        headers: dict[str, str] | None,
    ) -> AsyncIterator[ClientSession]:
        if isinstance(self._server, StdioMcpServer):
            merged_env = {**(self._server.env or {}), **(env or {})}
            params = StdioServerParameters(
                command=self._server.command,
                args=list(self._server.args),
                env=merged_env or None,
                cwd=self._server.cwd,
            )
            logger.debug(
                "Spawning MCP subprocess: %s %s (env keys: %s)",
                params.command,
                " ".join(params.args),
                sorted(merged_env.keys()),
            )
            async with stdio_client(params) as (read_stream, write_stream):
                async with ClientSession(read_stream, write_stream) as session:
                    await session.initialize()
                    yield session
        else:
            from mcp.client.streamable_http import streamablehttp_client

            merged_headers = {**self._server.headers, **(headers or {})}
            logger.debug(
                "Opening MCP HTTP session: %s (header keys: %s)",
                self._server.url,
                sorted(merged_headers.keys()),
            )
            async with streamablehttp_client(
                self._server.url,
                headers=merged_headers or None,
                timeout=timedelta(seconds=self._server.timeout_seconds),
                sse_read_timeout=timedelta(
                    seconds=self._server.sse_read_timeout_seconds
                ),
            ) as (read_stream, write_stream, _get_session_id):
                async with ClientSession(read_stream, write_stream) as session:
                    await session.initialize()
                    yield session

    async def _run(
        self,
        callback: Callable[[ClientSession], Awaitable[T]],
        *,
        env: dict[str, str] | None = None,
        headers: dict[str, str] | None = None,
    ) -> T:
        # Cancellation of this task closes stdio_client and terminates the
        # child process. Keep a hung official MCP/CLI process from surviving a
        # request indefinitely.
        try:
            timeout_seconds = min(
                max(float(os.environ.get("OMNI_MCP_TIMEOUT_SECONDS", "120")), 1.0),
                300.0,
            )
        except ValueError:
            timeout_seconds = 120.0
        try:
            persistent_identity_available = bool(
                (env or {}).get(MCP_POOL_SOURCE_ID_ENV) and (env or {}).get(MCP_POOL_USER_ID_ENV)
            )
            if self._persistent_stdio and persistent_identity_available:
                return await asyncio.wait_for(
                    self._run_persistent(callback, env or {}, timeout_seconds),
                    timeout=timeout_seconds,
                )
            return await asyncio.wait_for(
                self._run_unbounded(callback, env=env, headers=headers),
                timeout=timeout_seconds,
            )
        except Exception as exc:
            # A provider launcher may leave this short-lived marker after a
            # terminal OAuth rejection. Consume it here so bootstrap callers
            # also clean it up, while the normalized exception lets the HTTP
            # server return the standard 412 response. Marker IO is best
            # effort: a read or cleanup failure must never mask the original
            # MCP failure. A marker owned by a live pooled process is read by
            # its worker after startup settles, so the requester must not
            # unlink it first.
            status_file = (env or {}).get(MCP_AUTH_STATUS_FILE_ENV)
            requires_auth = False
            if isinstance(status_file, str) and status_file:
                try:
                    content = Path(status_file).read_text(encoding="utf-8").strip()
                    requires_auth = content == "needs_user_auth"
                except OSError:
                    pass
                if not self._marker_is_owned(status_file):
                    try:
                        Path(status_file).unlink(missing_ok=True)
                    except OSError:
                        pass
            if requires_auth:
                raise RuntimeError(MCP_AUTH_REQUIRED_MESSAGE) from exc
            raise
        except asyncio.CancelledError:
            status_file = (env or {}).get(MCP_AUTH_STATUS_FILE_ENV)
            if (
                isinstance(status_file, str)
                and status_file
                and not self._marker_is_owned(status_file)
            ):
                try:
                    Path(status_file).unlink(missing_ok=True)
                except OSError:
                    pass
            raise

    async def _run_persistent(
        self,
        callback: Callable[[ClientSession], Awaitable[T]],
        env: dict[str, str],
        timeout_seconds: float,
        *,
        _retries_left: int = 1,
    ) -> T:
        source_id = env.get(MCP_POOL_SOURCE_ID_ENV)
        user_id = env.get(MCP_POOL_USER_ID_ENV)
        if not source_id or not user_id:
            raise RuntimeError("Persistent Salesforce MCP requires explicit source and user IDs")
        identity = json.dumps((source_id, user_id), separators=(",", ":"))
        fingerprint_env = {
            key: value
            for key, value in env.items()
            if key not in {MCP_AUTH_STATUS_FILE_ENV, MCP_POOL_SOURCE_ID_ENV, MCP_POOL_USER_ID_ENV}
        }
        fingerprint = hashlib.sha256(
            json.dumps(fingerprint_env, sort_keys=True).encode("utf-8")
        ).hexdigest()
        auth_status_file = env.get(MCP_AUTH_STATUS_FILE_ENV)
        old_process: _PersistentProcess | None = None
        try:
            async with self._process_lock:
                if self._closed:
                    raise RuntimeError("MCP adapter is shut down")
                process = self._persistent_processes.get(identity)
                if process is not None and self._process_is_unusable(process):
                    self._persistent_processes.pop(identity, None)
                    old_process = process
                    process = None
                if process is not None and process.fingerprint != fingerprint:
                    process.closing = True
                    self._persistent_processes.pop(identity, None)
                    old_process = process
                    process = None
                if process is None and old_process is None:
                    if len(self._all_processes) >= self._process_limit:
                        idle = min(
                            (
                                item
                                for item in self._all_processes.values()
                                if not item.busy
                                and not item.closing
                                and item.queue.empty()
                                and item.ready is not None
                                and item.ready.done()
                                and item.ready.exception() is None
                            ),
                            key=lambda item: item.last_used,
                            default=None,
                        )
                        if idle is None:
                            raise McpProcessCapacityError(MCP_PROCESS_BUSY_MESSAGE)
                        idle.closing = True
                        self._persistent_processes.pop(idle.key, None)
                        old_process = idle
                if process is None and old_process is None:
                    process_env = dict(env)
                    status_file = auth_status_file
                    if status_file is None:
                        fd, status_file = tempfile.mkstemp(prefix="omni-mcp-auth-")
                        os.close(fd)
                        process_env[MCP_AUTH_STATUS_FILE_ENV] = status_file
                    process = _PersistentProcess(
                        key=identity,
                        fingerprint=fingerprint,
                        env=process_env,
                        last_used=asyncio.get_running_loop().time(),
                        auth_status_file=status_file,
                    )
                    process.ready = asyncio.get_running_loop().create_future()
                    process.task = asyncio.create_task(self._persistent_worker(process))
                    self._persistent_processes[identity] = process
                    self._all_processes[id(process)] = process
            if old_process is not None:
                await self._stop_process(old_process)
                if old_process.key != identity:
                    async with self._process_lock:
                        self._all_processes.pop(id(old_process), None)
                return await self._run_persistent(
                    callback, env, timeout_seconds, _retries_left=_retries_left
                )
            assert process is not None and process.ready is not None
            return await self._dispatch_persistent(
                process, callback, timeout_seconds, env, _retries_left
            )
        finally:
            # Covers creation, rotation/eviction stops, the recursion, and
            # dispatch: a caller marker must not outlive this call, and a
            # marker a pooled worker still owns (startup unsettled) is left
            # for the worker to consume.
            self._delete_unowned_auth_status_file(auth_status_file)

    async def _dispatch_persistent(
        self,
        process: _PersistentProcess,
        callback: Callable[[ClientSession], Awaitable[T]],
        timeout_seconds: float,
        env: dict[str, str],
        retries_left: int,
    ) -> T:
        ready = process.ready
        assert ready is not None
        # Startup is bounded by this request's timeout. The last waiter to
        # give up closes the worker so a hung child cannot hold a cap slot
        # (or idle until the idle timeout) indefinitely; earlier waiters let
        # their own bounded waits continue.
        process.startup_waiters += 1
        try:
            await asyncio.wait_for(asyncio.shield(ready), timeout=timeout_seconds)
        except BaseException:
            await self._finish_startup_wait(process)
            raise
        process.startup_waiters -= 1
        future: asyncio.Future[T] = asyncio.get_running_loop().create_future()
        request = _PersistentRequest(callback, future, timeout_seconds)
        # The enqueue decision is coordinated with the worker's exit decision
        # under the pool lock, so a request can never be enqueued onto a
        # closing or already-exited generation and then silently dropped.
        try:
            async with self._process_lock:
                if process.closing or self._persistent_processes.get(process.key) is not process:
                    raise McpProcessClosedError(MCP_PROCESS_CLOSED_MESSAGE)
                process.queue.put_nowait(request)
        except McpProcessClosedError:
            return await self._retry_persistent(callback, env, timeout_seconds, retries_left)
        try:
            return await asyncio.wait_for(asyncio.shield(future), timeout=timeout_seconds)
        except McpProcessClosedError:
            # The worker exited or was replaced after the request was queued
            # but before it started. A cold retry must never re-run a request
            # the worker already began (tool side effects).
            if request.started or retries_left <= 0:
                raise
            return await self._run_persistent(
                callback, env, timeout_seconds, _retries_left=retries_left - 1
            )
        except asyncio.CancelledError:
            future.cancel()
            if request.started:
                await self._invalidate_process(process)
            raise
        except TimeoutError:
            future.cancel()
            if request.started:
                await self._invalidate_process(process)
            raise

    async def _retry_persistent(
        self,
        callback: Callable[[ClientSession], Awaitable[T]],
        env: dict[str, str],
        timeout_seconds: float,
        retries_left: int,
    ) -> T:
        if retries_left <= 0:
            raise McpProcessClosedError(MCP_PROCESS_CLOSED_MESSAGE)
        return await self._run_persistent(
            callback, env, timeout_seconds, _retries_left=retries_left - 1
        )

    async def _finish_startup_wait(self, process: _PersistentProcess) -> None:
        process.startup_waiters -= 1
        if process.startup_waiters > 0:
            return
        ready = process.ready
        if ready is not None and not ready.done() and not process.closing:
            await self._invalidate_process(process)

    @staticmethod
    def _delete_auth_status_file(path: str | None) -> None:
        if path:
            try:
                Path(path).unlink(missing_ok=True)
            except OSError:
                pass

    def _marker_is_owned(self, path: str | None) -> bool:
        if not path:
            return False
        return any(process.auth_status_file == path for process in self._all_processes.values())

    def _delete_unowned_auth_status_file(self, path: str | None) -> None:
        # The pooled worker owns the marker until startup settles; deleting it
        # beforehand would erase the needs_user_auth signal before the worker
        # can observe it.
        if path and not self._marker_is_owned(path):
            self._delete_auth_status_file(path)

    async def _persistent_worker(self, process: _PersistentProcess) -> None:
        try:
            async with AsyncExitStack() as stack:
                server = self._server
                if not isinstance(server, StdioMcpServer):
                    raise RuntimeError("Persistent MCP reuse requires stdio transport")
                merged_env = {**(server.env or {}), **process.env}
                params = StdioServerParameters(
                    command=server.command,
                    args=list(server.args),
                    env=merged_env or None,
                    cwd=server.cwd,
                )
                read_stream, write_stream = await stack.enter_async_context(stdio_client(params))
                session = await stack.enter_async_context(ClientSession(read_stream, write_stream))
                await session.initialize()
                assert process.ready is not None
                if not process.ready.done():
                    process.ready.set_result(None)
                self._delete_auth_status_file(process.auth_status_file)
                process.auth_status_file = None
                process.last_used = asyncio.get_running_loop().time()
                while True:
                    # The exit decision is coordinated with request enqueues
                    # under the pool lock: either the enqueued request is
                    # seen and served, or the worker exits first and the
                    # dispatcher retries cold. Never both dropped and lost.
                    async with self._process_lock:
                        if process.closing:
                            break
                        elapsed = asyncio.get_running_loop().time() - process.last_used
                        if elapsed >= self._idle_timeout_seconds and process.queue.empty():
                            process.closing = True
                            break
                    remaining = self._idle_timeout_seconds - (
                        asyncio.get_running_loop().time() - process.last_used
                    )
                    try:
                        request = await asyncio.wait_for(
                            process.queue.get(), timeout=max(remaining, 0.001)
                        )
                    except TimeoutError:
                        request = None
                    if request is None:
                        continue
                    if request.future.cancelled():
                        continue
                    request.started = True
                    process.busy = True
                    process.current_request = request
                    try:
                        result = await asyncio.wait_for(
                            request.callback(session), timeout=request.timeout_seconds
                        )
                    except BaseException as exc:
                        if not request.future.done():
                            request.future.set_exception(exc)
                        raise
                    else:
                        if not request.future.done():
                            request.future.set_result(result)
                    finally:
                        process.busy = False
                        process.current_request = None
                        process.last_used = asyncio.get_running_loop().time()
                self._fail_queued_requests(process, None)
        except BaseException as exc:
            if process.ready is not None and not process.ready.done():
                auth_required = self._auth_status_requires_auth(process.auth_status_file)
                if isinstance(exc, asyncio.CancelledError):
                    process.ready.cancel()
                else:
                    process.ready.set_exception(
                        RuntimeError(MCP_AUTH_REQUIRED_MESSAGE) if auth_required else exc
                    )
            self._fail_queued_requests(process, exc)
            if not isinstance(exc, asyncio.CancelledError):
                logger.warning("Persistent Salesforce MCP process failed", exc_info=True)
        finally:
            process.closing = True
            self._delete_auth_status_file(process.auth_status_file)
            async with self._process_lock:
                if self._persistent_processes.get(process.key) is process:
                    self._persistent_processes.pop(process.key, None)
                self._all_processes.pop(id(process), None)

    @staticmethod
    def _process_is_unusable(process: _PersistentProcess) -> bool:
        if process.closing:
            return True
        if process.task is not None and process.task.done():
            return True
        return (
            process.ready is not None
            and process.ready.done()
            and process.ready.exception() is not None
        )

    @staticmethod
    def _fail_queued_requests(process: _PersistentProcess, exc: BaseException | None) -> None:
        while not process.queue.empty():
            request = process.queue.get_nowait()
            if request is None or request.future.done():
                continue
            if exc is None or isinstance(exc, asyncio.CancelledError):
                # The generation expired without running the request; a
                # distinguishable error lets the dispatcher cold-retry it
                # instead of surfacing a misleading cancellation.
                request.future.set_exception(McpProcessClosedError(MCP_PROCESS_CLOSED_MESSAGE))
            else:
                request.future.set_exception(exc)

    @staticmethod
    def _auth_status_requires_auth(status_file: str | None) -> bool:
        if not status_file:
            return False
        try:
            return Path(status_file).read_text(encoding="utf-8").strip() == "needs_user_auth"
        except OSError:
            return False

    async def _invalidate_process(self, process: _PersistentProcess) -> None:
        # Set synchronously first: if the requester's own cancellation interrupt
        # awaits below, the worker still exits on its own instead of idling
        # until the idle timeout with a dead request generation. The task
        # cancel is also synchronous so an interrupted await cannot leave a
        # hung startup (or mid-call) worker holding a cap slot.
        process.closing = True
        async with self._process_lock:
            if self._persistent_processes.get(process.key) is process:
                self._persistent_processes.pop(process.key, None)
        if process.task is not None and not process.task.done():
            process.task.cancel()
        await self._stop_process(process)

    async def _stop_process(self, process: _PersistentProcess) -> None:
        process.closing = True
        if process.task is not None and not process.task.done():
            process.task.cancel()
            try:
                await process.task
            except asyncio.CancelledError:
                pass

    async def shutdown(self) -> None:
        self._closed = True
        async with self._process_lock:
            processes = list(self._all_processes.values())
            for process in processes:
                process.closing = True
            self._persistent_processes.clear()
        await asyncio.gather(*(self._stop_process(process) for process in processes))

    async def _run_unbounded(
        self,
        callback: Callable[[ClientSession], Awaitable[T]],
        *,
        env: dict[str, str] | None = None,
        headers: dict[str, str] | None = None,
    ) -> T:
        async with self._open_session(env, headers) as session:
            return await callback(session)

    def _clear_catalog_cache(self) -> None:
        self._cached_actions = None
        self._cached_resources = None
        self._cached_prompts = None

    @property
    def _has_cached_catalog(self) -> bool:
        return (
            self._cached_actions is not None
            or self._cached_resources is not None
            or self._cached_prompts is not None
        )

    async def discover(
        self,
        env: dict[str, str] | None = None,
        headers: dict[str, str] | None = None,
    ) -> None:
        """Connect to MCP and cache tools plus optional resources/prompts.

        MCP servers are allowed to implement only tools. In particular, some
        official provider servers return ``Method not found`` for the
        optional resource and prompt list methods, so those failures must not
        discard a successfully discovered tool catalog.
        """

        # A failed discovery must not leave a removed or policy-disabled
        # catalog active. Publish a catalog only after this attempt succeeds.
        self._clear_catalog_cache()

        async def _discover(session: ClientSession) -> None:
            actions = await self._fetch_actions(session)
            try:
                resources = await self._fetch_resources(session)
            except Exception:
                logger.info("MCP server does not expose resources", exc_info=True)
                resources = []
            try:
                prompts = await self._fetch_prompts(session)
            except Exception:
                logger.info("MCP server does not expose prompts", exc_info=True)
                prompts = []

            # Publish a complete catalog only after every optional lookup has
            # settled. This prevents a partial failed discovery from looking
            # like a valid cache on the next manifest registration.
            self._cached_actions = actions
            self._cached_resources = resources
            self._cached_prompts = prompts

        await self._run(_discover, env=env, headers=headers)
        logger.info(
            "MCP discovery complete: %d tools, %d resources, %d prompts",
            len(self._cached_actions or []),
            len(self._cached_resources or []),
            len(self._cached_prompts or []),
        )

    async def get_action_definitions(
        self,
        env: dict[str, str] | None = None,
        headers: dict[str, str] | None = None,
    ) -> list[ActionDefinition]:
        if env is not None or headers is not None:
            try:

                async def _fetch(session: ClientSession) -> list[ActionDefinition]:
                    actions = await self._fetch_actions(session)
                    self._cached_actions = actions
                    logger.debug("Fetched %d action definitions (live)", len(actions))
                    return actions

                return await self._run(_fetch, env=env, headers=headers)
            except Exception:
                if self._cached_actions is not None:
                    logger.debug(
                        "Live fetch failed, returning %d cached actions",
                        len(self._cached_actions),
                    )
                    return self._cached_actions
                raise
        logger.debug(
            "No auth provided, returning %d cached actions",
            len(self._cached_actions or []),
        )
        return self._cached_actions or []

    async def get_resource_definitions(
        self,
        env: dict[str, str] | None = None,
        headers: dict[str, str] | None = None,
    ) -> list[McpResourceDefinition]:
        if env is not None or headers is not None:
            try:

                async def _fetch(session: ClientSession) -> list[McpResourceDefinition]:
                    resources = await self._fetch_resources(session)
                    self._cached_resources = resources
                    return resources

                return await self._run(_fetch, env=env, headers=headers)
            except Exception:
                logger.info("MCP resource discovery unavailable", exc_info=True)
                return self._cached_resources or []
        return self._cached_resources or []

    async def get_prompt_definitions(
        self,
        env: dict[str, str] | None = None,
        headers: dict[str, str] | None = None,
    ) -> list[McpPromptDefinition]:
        if env is not None or headers is not None:
            try:

                async def _fetch(session: ClientSession) -> list[McpPromptDefinition]:
                    prompts = await self._fetch_prompts(session)
                    self._cached_prompts = prompts
                    return prompts

                return await self._run(_fetch, env=env, headers=headers)
            except Exception:
                logger.info("MCP prompt discovery unavailable", exc_info=True)
                return self._cached_prompts or []
        return self._cached_prompts or []

    async def execute_tool(
        self,
        name: str,
        arguments: dict[str, Any],
        env: dict[str, str] | None = None,
        headers: dict[str, str] | None = None,
    ) -> ActionResponse:
        try:

            async def _call(session: ClientSession) -> ActionResponse:
                result = await session.call_tool(name, arguments)
                text_parts: list[str] = []
                for block in result.content:
                    if hasattr(block, "text"):
                        text_parts.append(block.text)
                    elif hasattr(block, "data"):
                        text_parts.append(
                            f"[binary: {getattr(block, 'mimeType', 'unknown')}]"
                        )
                content = "\n".join(text_parts)
                if result.isError:
                    return ActionResponse.failure(content)
                return ActionResponse.success({"content": content})

            return await self._run(_call, env=env, headers=headers)
        except McpProcessCapacityError:
            raise
        except McpProcessClosedError:
            raise
        except Exception as e:
            logger.error("MCP tool %s failed: %s", name, e)
            return ActionResponse.failure(str(e))

    async def read_resource(
        self,
        uri: str,
        env: dict[str, str] | None = None,
        headers: dict[str, str] | None = None,
    ) -> dict[str, Any]:
        async def _read(session: ClientSession) -> dict[str, Any]:
            result = await session.read_resource(AnyUrl(uri))
            items: list[dict[str, Any]] = []
            for item in result.contents:
                entry: dict[str, Any] = {"uri": str(item.uri)}
                if hasattr(item, "text") and item.text is not None:
                    entry["text"] = item.text
                if hasattr(item, "mimeType") and item.mimeType:
                    entry["mime_type"] = item.mimeType
                items.append(entry)
            return {"contents": items}

        return await self._run(_read, env=env, headers=headers)

    async def get_prompt(
        self,
        name: str,
        arguments: dict[str, Any] | None = None,
        env: dict[str, str] | None = None,
        headers: dict[str, str] | None = None,
    ) -> dict[str, Any]:
        async def _get(session: ClientSession) -> dict[str, Any]:
            result = await session.get_prompt(name, arguments)
            messages: list[dict[str, Any]] = []
            for msg in result.messages:
                content_data: dict[str, Any]
                if hasattr(msg.content, "text"):
                    content_data = {"type": "text", "text": msg.content.text}
                else:
                    content_data = {"type": "unknown"}
                messages.append({"role": msg.role, "content": content_data})
            return {"description": result.description, "messages": messages}

        return await self._run(_get, env=env, headers=headers)

    # -- internal helpers to convert MCP types to Omni models --

    @staticmethod
    async def _fetch_actions(session: ClientSession) -> list[ActionDefinition]:
        result = await session.list_tools()
        actions: list[ActionDefinition] = []
        for tool in result.tools:
            is_read_only = bool(tool.annotations and tool.annotations.readOnlyHint)
            actions.append(
                ActionDefinition(
                    name=tool.name,
                    description=tool.description or "",
                    input_schema=tool.inputSchema
                    or {"type": "object", "properties": {}},
                    mode="read" if is_read_only else "write",
                    credential_scope="user",
                    # MCP tools are not admin-only by default.
                    admin_only=False,
                    origin="mcp",
                )
            )
        return actions

    @staticmethod
    async def _fetch_resources(session: ClientSession) -> list[McpResourceDefinition]:
        definitions: list[McpResourceDefinition] = []

        templates_result = await session.list_resource_templates()
        for tmpl in templates_result.resourceTemplates:
            definitions.append(
                McpResourceDefinition(
                    uri_template=str(tmpl.uriTemplate),
                    name=tmpl.name,
                    description=tmpl.description,
                    mime_type=tmpl.mimeType,
                )
            )

        resources_result = await session.list_resources()
        for res in resources_result.resources:
            definitions.append(
                McpResourceDefinition(
                    uri_template=str(res.uri),
                    name=res.name,
                    description=res.description,
                    mime_type=res.mimeType,
                )
            )

        return definitions

    @staticmethod
    async def _fetch_prompts(session: ClientSession) -> list[McpPromptDefinition]:
        result = await session.list_prompts()
        definitions: list[McpPromptDefinition] = []
        for prompt in result.prompts:
            args = [
                McpPromptArgument(
                    name=arg.name,
                    description=arg.description,
                    required=arg.required or False,
                )
                for arg in (prompt.arguments or [])
            ]
            definitions.append(
                McpPromptDefinition(
                    name=prompt.name,
                    description=prompt.description,
                    arguments=args,
                )
            )
        return definitions
