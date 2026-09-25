"""Tests for the MCP adapter (stdio + Streamable HTTP transports)."""

import asyncio
import os
import socket
import subprocess
import sys
from pathlib import Path
from typing import Any

import httpx
import pytest

from omni_connector import Connector, HttpMcpServer, StdioMcpServer
from omni_connector.mcp_adapter import (
    MCP_AUTH_REQUIRED_MESSAGE,
    MCP_AUTH_STATUS_FILE_ENV,
    MCP_POOL_SOURCE_ID_ENV,
    MCP_POOL_USER_ID_ENV,
    McpAdapter,
    McpProcessCapacityError,
    McpProcessClosedError,
)

# Path to the test MCP server script (supports both stdio and http modes)
TEST_SERVER = os.path.join(os.path.dirname(__file__), "test_mcp_server.py")
TEST_STDIO_SERVER = StdioMcpServer(command=sys.executable, args=[TEST_SERVER])
# Dummy env to simulate having credentials (test server doesn't need real ones)
TEST_ENV: dict[str, str] = {"TEST_MODE": "1"}


def _free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


@pytest.fixture
async def http_server_url():
    """Spawn the test MCP server in Streamable HTTP mode on a random port."""
    port = _free_port()
    proc = subprocess.Popen(
        [sys.executable, TEST_SERVER, "http", str(port)],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    url = f"http://127.0.0.1:{port}/mcp"

    async def wait_ready():
        async with httpx.AsyncClient() as client:
            for _ in range(80):
                try:
                    # 4xx response means the server is up; the MCP endpoint
                    # rejects bare GETs but a TCP-level reply is enough.
                    await client.get(url, timeout=0.5)
                    return
                except (httpx.ConnectError, httpx.ReadError):
                    await asyncio.sleep(0.1)
        raise RuntimeError(f"HTTP MCP fixture did not start on {url}")

    try:
        await wait_ready()
        yield url
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()


class TestStdioAdapter:
    @pytest.fixture
    def adapter(self):
        return McpAdapter(TEST_STDIO_SERVER)

    async def test_get_action_definitions(self, adapter: McpAdapter):
        actions = await adapter.get_action_definitions(env=TEST_ENV)
        assert len(actions) == 2
        names = {a.name for a in actions}
        assert names == {"greet", "add"}

        greet_action = next(a for a in actions if a.name == "greet")
        assert greet_action.description == "Greet someone by name."
        assert "name" in greet_action.input_schema.get("properties", {})
        assert "name" in greet_action.input_schema.get("required", [])
        assert greet_action.input_schema["properties"]["name"]["type"] == "string"
        assert greet_action.mode == "read"

        add_action = next(a for a in actions if a.name == "add")
        assert "a" in add_action.input_schema.get("properties", {})
        assert "b" in add_action.input_schema.get("properties", {})
        assert add_action.mode == "write"

    async def test_get_resource_definitions(self, adapter: McpAdapter):
        resources = await adapter.get_resource_definitions(env=TEST_ENV)
        assert len(resources) == 1
        assert resources[0].name == "get_item"
        assert resources[0].uri_template == "test://item/{item_id}"

    async def test_get_prompt_definitions(self, adapter: McpAdapter):
        prompts = await adapter.get_prompt_definitions(env=TEST_ENV)
        assert len(prompts) == 1
        assert prompts[0].name == "summarize"
        assert prompts[0].description == "Summarize the given text."
        assert len(prompts[0].arguments) == 1
        assert prompts[0].arguments[0].name == "text"
        assert prompts[0].arguments[0].required is True

    async def test_execute_tool(self, adapter: McpAdapter):
        result = await adapter.execute_tool("greet", {"name": "World"}, env=TEST_ENV)
        assert result.status == "success"
        assert result.result is not None
        assert "Hello, World!" in result.result.get("content", "")

    async def test_execute_tool_error(self, adapter: McpAdapter):
        result = await adapter.execute_tool("nonexistent", {}, env=TEST_ENV)
        assert result.status == "error"

    async def test_read_resource(self, adapter: McpAdapter):
        result = await adapter.read_resource("test://item/42", env=TEST_ENV)
        assert "contents" in result
        contents = result["contents"]
        assert len(contents) >= 1

    async def test_get_prompt(self, adapter: McpAdapter):
        result = await adapter.get_prompt("summarize", {"text": "hello world"}, env=TEST_ENV)
        assert "messages" in result
        assert len(result["messages"]) >= 1
        msg = result["messages"][0]
        assert msg["role"] == "user"
        assert "hello world" in msg["content"]["text"]

    async def test_discover_caches_definitions(self, adapter: McpAdapter):
        """discover() populates cache, then no-auth calls return cached data."""
        await adapter.discover(env=TEST_ENV)
        actions = await adapter.get_action_definitions()
        assert len(actions) == 2
        resources = await adapter.get_resource_definitions()
        assert len(resources) == 1
        prompts = await adapter.get_prompt_definitions()
        assert len(prompts) == 1

    async def test_no_auth_no_cache_returns_empty(self, adapter: McpAdapter):
        """Without auth and without cache, returns empty lists."""
        assert await adapter.get_action_definitions() == []
        assert await adapter.get_resource_definitions() == []
        assert await adapter.get_prompt_definitions() == []

    async def test_persistent_stdio_reuses_per_identity_and_rotates_credentials(self, tmp_path):
        pid_file = str(tmp_path / "mcp-pid")
        adapter = McpAdapter(
            StdioMcpServer(command=sys.executable, args=[TEST_SERVER], persistent=True),
            idle_timeout_seconds=60,
        )
        first = {
            **TEST_ENV,
            MCP_POOL_SOURCE_ID_ENV: "source-1",
            MCP_POOL_USER_ID_ENV: "user-1",
            "TEST_PID_FILE": pid_file,
            "TOKEN": "one",
        }
        second = {
            **TEST_ENV,
            MCP_POOL_SOURCE_ID_ENV: "source-1",
            MCP_POOL_USER_ID_ENV: "user-2",
            "TOKEN": "one",
        }
        await adapter.execute_tool("greet", {"name": "first"}, env=first)
        process_one = adapter._persistent_processes['["source-1","user-1"]']
        pid_one = int(Path(pid_file).read_text())
        assert process_one.auth_status_file is None
        await adapter.execute_tool("greet", {"name": "again"}, env=first)
        assert adapter._persistent_processes['["source-1","user-1"]'] is process_one
        assert int(Path(pid_file).read_text()) == pid_one
        await adapter.execute_tool("greet", {"name": "other user"}, env=second)
        third = {**first, MCP_POOL_SOURCE_ID_ENV: "source-2"}
        await adapter.execute_tool("greet", {"name": "other source"}, env=third)
        assert len(adapter._persistent_processes) == 3

        rotated = {**first, "TOKEN": "rotated"}
        await adapter.execute_tool("greet", {"name": "rotated"}, env=rotated)
        assert adapter._persistent_processes['["source-1","user-1"]'] is not process_one
        rotated_pid = int(Path(pid_file).read_text())
        assert rotated_pid != pid_one
        await adapter.shutdown()
        assert adapter._persistent_processes == {}
        with pytest.raises(ProcessLookupError):
            os.kill(rotated_pid, 0)

    async def test_persistent_same_user_calls_share_startup_and_serialize(self, tmp_path):
        pid_file = str(tmp_path / "mcp-pid")
        adapter = McpAdapter(
            StdioMcpServer(command=sys.executable, args=[TEST_SERVER], persistent=True),
            idle_timeout_seconds=60,
        )
        env = {
            **TEST_ENV,
            MCP_POOL_SOURCE_ID_ENV: "source",
            MCP_POOL_USER_ID_ENV: "user",
            "TEST_PID_FILE": pid_file,
        }
        first_started = asyncio.Event()
        release_first = asyncio.Event()
        second_started = asyncio.Event()

        async def first(session):
            first_started.set()
            await release_first.wait()
            return "first"

        async def second(session):
            second_started.set()
            return "second"

        first_task = asyncio.create_task(adapter._run(first, env=env))
        await first_started.wait()
        second_task = asyncio.create_task(adapter._run(second, env=env))
        await asyncio.sleep(0.05)
        assert not second_started.is_set()
        assert len(adapter._all_processes) == 1
        process_pid = int(Path(pid_file).read_text())
        release_first.set()
        assert await first_task == "first"
        assert await second_task == "second"
        assert int(Path(pid_file).read_text()) == process_pid
        await adapter.shutdown()
        with pytest.raises(ProcessLookupError):
            os.kill(process_pid, 0)

    async def test_persistent_idle_expiry_and_lru_eviction(self, tmp_path):
        pid_file = str(tmp_path / "mcp-pid")
        adapter = McpAdapter(
            StdioMcpServer(command=sys.executable, args=[TEST_SERVER], persistent=True),
            process_limit=2,
            idle_timeout_seconds=30,
        )

        def env(source: str) -> dict[str, str]:
            return {
                **TEST_ENV,
                MCP_POOL_SOURCE_ID_ENV: source,
                MCP_POOL_USER_ID_ENV: "user",
                "TEST_PID_FILE": pid_file,
            }

        await adapter.execute_tool("greet", {"name": "one"}, env=env("one"))
        one_key = '["one","user"]'
        one_pid = int(Path(pid_file).read_text())
        await adapter.execute_tool("greet", {"name": "two"}, env=env("two"))
        two_key = '["two","user"]'
        two_pid = int(Path(pid_file).read_text())
        await adapter.execute_tool("greet", {"name": "one refreshed"}, env=env("one"))
        await adapter.execute_tool("greet", {"name": "three"}, env=env("three"))
        assert one_key in adapter._persistent_processes
        assert two_key not in adapter._persistent_processes
        with pytest.raises(ProcessLookupError):
            os.kill(two_pid, 0)
        await adapter.shutdown()
        with pytest.raises(ProcessLookupError):
            os.kill(one_pid, 0)

        expiring = McpAdapter(
            StdioMcpServer(command=sys.executable, args=[TEST_SERVER], persistent=True),
            idle_timeout_seconds=0.15,
        )
        await expiring.execute_tool("greet", {"name": "idle"}, env=env("idle"))
        idle_key = '["idle","user"]'
        idle_pid = int(Path(pid_file).read_text())
        await asyncio.sleep(0.3)
        assert idle_key not in expiring._persistent_processes
        with pytest.raises(ProcessLookupError):
            os.kill(idle_pid, 0)
        await expiring.shutdown()

    async def test_persistent_process_capacity_does_not_wait_for_active_process(self):
        adapter = McpAdapter(
            StdioMcpServer(command=sys.executable, args=[TEST_SERVER], persistent=True),
            process_limit=1,
            idle_timeout_seconds=60,
        )
        entered = asyncio.Event()
        release = asyncio.Event()

        async def hold(session):
            entered.set()
            await release.wait()
            return await session.list_tools()

        first = asyncio.create_task(
            adapter._run(
                hold,
                env={
                    **TEST_ENV,
                    MCP_POOL_SOURCE_ID_ENV: "source",
                    MCP_POOL_USER_ID_ENV: "user-1",
                },
            )
        )
        await entered.wait()
        with pytest.raises(McpProcessCapacityError):
            await adapter._run(
                lambda session: session.list_tools(),
                env={
                    **TEST_ENV,
                    MCP_POOL_SOURCE_ID_ENV: "source",
                    MCP_POOL_USER_ID_ENV: "user-2",
                },
            )
        release.set()
        await first
        await adapter.shutdown()

    async def test_persistent_cancellation_reaps_active_child(self, tmp_path):
        pid_file = str(tmp_path / "mcp-pid")
        env = {
            **TEST_ENV,
            MCP_POOL_SOURCE_ID_ENV: "source",
            MCP_POOL_USER_ID_ENV: "user",
            "TEST_PID_FILE": pid_file,
        }
        adapter = McpAdapter(
            StdioMcpServer(command=sys.executable, args=[TEST_SERVER], persistent=True),
            idle_timeout_seconds=60,
        )
        entered = asyncio.Event()

        async def wait_forever(session):
            entered.set()
            await asyncio.Event().wait()

        task = asyncio.create_task(adapter._run(wait_forever, env=env))
        await entered.wait()
        pid = int(Path(pid_file).read_text())
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await task
        assert '["source","user"]' not in adapter._persistent_processes
        with pytest.raises(ProcessLookupError):
            os.kill(pid, 0)
        await adapter.shutdown()

    async def test_persistent_request_timeout_invalidates_and_reaps(self, tmp_path, monkeypatch):
        pid_file = str(tmp_path / "mcp-pid")
        monkeypatch.setenv("OMNI_MCP_TIMEOUT_SECONDS", "1")
        env = {
            **TEST_ENV,
            MCP_POOL_SOURCE_ID_ENV: "source",
            MCP_POOL_USER_ID_ENV: "user",
            "TEST_PID_FILE": pid_file,
        }
        adapter = McpAdapter(
            StdioMcpServer(command=sys.executable, args=[TEST_SERVER], persistent=True),
            idle_timeout_seconds=60,
        )
        entered = asyncio.Event()

        async def wait_forever(session):
            entered.set()
            await asyncio.Event().wait()

        with pytest.raises(TimeoutError):
            await adapter._run(wait_forever, env=env)
        assert '["source","user"]' not in adapter._persistent_processes
        pid = int(Path(pid_file).read_text())
        with pytest.raises(ProcessLookupError):
            os.kill(pid, 0)
        await adapter.shutdown()

    async def test_persistent_never_ready_startup_reaped_and_frees_cap(self, tmp_path, monkeypatch):
        pid_file = str(tmp_path / "mcp-pid")
        monkeypatch.setenv("OMNI_MCP_TIMEOUT_SECONDS", "1")
        hang_env = {
            **TEST_ENV,
            MCP_POOL_SOURCE_ID_ENV: "source",
            MCP_POOL_USER_ID_ENV: "user",
            "TEST_PID_FILE": pid_file,
        }
        adapter = McpAdapter(
            StdioMcpServer(command=sys.executable, args=[TEST_SERVER, "hang"], persistent=True),
            process_limit=1,
            idle_timeout_seconds=60,
        )
        identity = '["source","user"]'
        with pytest.raises(TimeoutError):
            await adapter._run(lambda session: session.list_tools(), env=hang_env)
        assert identity not in adapter._persistent_processes

        pid = int(Path(pid_file).read_text())
        deadline = asyncio.get_running_loop().time() + 8
        while asyncio.get_running_loop().time() < deadline:
            try:
                os.kill(pid, 0)
            except ProcessLookupError:
                break
            await asyncio.sleep(0.05)
        with pytest.raises(ProcessLookupError):
            os.kill(pid, 0)

        deadline = asyncio.get_running_loop().time() + 8
        while asyncio.get_running_loop().time() < deadline and adapter._all_processes:
            await asyncio.sleep(0.05)
        assert not adapter._all_processes

        adapter._server = StdioMcpServer(
            command=sys.executable, args=[TEST_SERVER], persistent=True
        )
        result = await adapter.execute_tool("greet", {"name": "x"}, env=hang_env)
        assert result.status == "success"
        await adapter.shutdown()

    async def test_persistent_startup_timeout_only_reaped_by_last_waiter(self, tmp_path):
        pid_file = str(tmp_path / "mcp-pid")
        env = {
            **TEST_ENV,
            MCP_POOL_SOURCE_ID_ENV: "source",
            MCP_POOL_USER_ID_ENV: "user",
            "TEST_PID_FILE": pid_file,
        }
        adapter = McpAdapter(
            StdioMcpServer(command=sys.executable, args=[TEST_SERVER, "hang"], persistent=True),
            idle_timeout_seconds=60,
        )
        identity = '["source","user"]'
        first = asyncio.create_task(adapter._run_persistent(lambda s: s.initialize(), env, 0.3))
        await asyncio.sleep(0.1)
        second = asyncio.create_task(adapter._run_persistent(lambda s: s.initialize(), env, 1.5))
        with pytest.raises(TimeoutError):
            await first
        assert identity in adapter._persistent_processes
        assert adapter._all_processes
        with pytest.raises(TimeoutError):
            await second
        assert identity not in adapter._persistent_processes
        pid = int(Path(pid_file).read_text())
        deadline = asyncio.get_running_loop().time() + 8
        while asyncio.get_running_loop().time() < deadline:
            try:
                os.kill(pid, 0)
            except ProcessLookupError:
                break
            await asyncio.sleep(0.05)
        with pytest.raises(ProcessLookupError):
            os.kill(pid, 0)
        await adapter.shutdown()

    async def test_nonpersistent_adapters_ignore_pool_configuration(self, monkeypatch):
        monkeypatch.setenv("OMNI_MCP_MAX_PROCESSES", "not-a-number")
        monkeypatch.setenv("OMNI_MCP_IDLE_TIMEOUT_SECONDS", "not-a-number")
        adapter = McpAdapter(TEST_STDIO_SERVER)
        assert adapter._process_limit == 0
        assert adapter._idle_timeout_seconds == 0.0
        assert adapter._persistent_stdio is False
        result = await adapter.execute_tool("greet", {"name": "x"}, env=TEST_ENV)
        assert result.status == "success"
        http_adapter = McpAdapter(HttpMcpServer(url="http://127.0.0.1:9/mcp"))
        assert http_adapter._persistent_stdio is False

    async def test_persistent_adapter_validates_pool_configuration(self, monkeypatch):
        monkeypatch.setenv("OMNI_MCP_MAX_PROCESSES", "not-a-number")
        with pytest.raises(ValueError):
            McpAdapter(StdioMcpServer(command=sys.executable, args=[TEST_SERVER], persistent=True))
        monkeypatch.setenv("OMNI_MCP_MAX_PROCESSES", "1")
        monkeypatch.setenv("OMNI_MCP_IDLE_TIMEOUT_SECONDS", "-1")
        with pytest.raises(ValueError):
            McpAdapter(StdioMcpServer(command=sys.executable, args=[TEST_SERVER], persistent=True))
        adapter = McpAdapter(
            StdioMcpServer(command=sys.executable, args=[TEST_SERVER], persistent=True),
            process_limit=2,
            idle_timeout_seconds=30,
        )
        assert adapter._process_limit == 2
        assert adapter._idle_timeout_seconds == 30.0

    async def test_persistent_cancellation_during_rotation_cleans_caller_marker(
        self, tmp_path, monkeypatch
    ):
        pid_file = str(tmp_path / "mcp-pid")
        old_marker = tmp_path / "old-auth-status"
        old_marker.write_text("")
        new_marker = tmp_path / "new-auth-status"
        new_marker.write_text("")
        env_old = {
            **TEST_ENV,
            MCP_POOL_SOURCE_ID_ENV: "source",
            MCP_POOL_USER_ID_ENV: "user",
            "TEST_PID_FILE": pid_file,
            MCP_AUTH_STATUS_FILE_ENV: str(old_marker),
            "TOKEN": "old",
        }
        adapter = McpAdapter(
            StdioMcpServer(command=sys.executable, args=[TEST_SERVER], persistent=True),
            idle_timeout_seconds=60,
        )
        identity = '["source","user"]'

        await adapter.execute_tool("greet", {"name": "first"}, env=env_old)
        assert adapter._persistent_processes[identity].auth_status_file is None
        assert not old_marker.exists()

        entered = asyncio.Event()

        async def wait_forever(session):
            entered.set()
            await asyncio.Event().wait()

        busy_call = asyncio.create_task(adapter._run(wait_forever, env=env_old))
        await entered.wait()
        assert adapter._persistent_processes[identity].busy

        gate = asyncio.Event()
        stop_entered = asyncio.Event()
        real_stop = adapter._stop_process

        async def gated_stop(process):
            stop_entered.set()
            await gate.wait()
            await real_stop(process)

        monkeypatch.setattr(adapter, "_stop_process", gated_stop)

        env_new = {**env_old, "TOKEN": "new", MCP_AUTH_STATUS_FILE_ENV: str(new_marker)}
        rotation = asyncio.create_task(adapter.execute_tool("greet", {"name": "r"}, env=env_new))
        await asyncio.wait_for(stop_entered.wait(), timeout=5)
        assert new_marker.exists()

        rotation.cancel()
        with pytest.raises(asyncio.CancelledError):
            await rotation
        assert not new_marker.exists()
        assert not old_marker.exists()

        monkeypatch.setattr(adapter, "_stop_process", real_stop)
        gate.set()
        await adapter.shutdown()
        with pytest.raises(asyncio.CancelledError):
            await busy_call
        pid = int(Path(pid_file).read_text())
        with pytest.raises(ProcessLookupError):
            os.kill(pid, 0)

    async def test_persistent_queued_request_survives_generation_replacement(self, tmp_path):
        pid_file = str(tmp_path / "mcp-pid")
        env = {
            **TEST_ENV,
            MCP_POOL_SOURCE_ID_ENV: "source",
            MCP_POOL_USER_ID_ENV: "user",
            "TEST_PID_FILE": pid_file,
        }
        adapter = McpAdapter(
            StdioMcpServer(command=sys.executable, args=[TEST_SERVER], persistent=True),
            idle_timeout_seconds=60,
        )
        identity = '["source","user"]'
        await adapter.execute_tool("greet", {"name": "first"}, env=env)
        process = adapter._persistent_processes[identity]
        pid_one = int(Path(pid_file).read_text())

        entered = asyncio.Event()
        release = asyncio.Event()

        async def hold(session):
            entered.set()
            await release.wait()
            return "held"

        busy_call = asyncio.create_task(adapter._run(hold, env=env))
        await entered.wait()

        calls: list[int] = []

        async def counted(session):
            calls.append(1)
            return "queued-done"

        queued_call = asyncio.create_task(adapter._run(counted, env=env))
        await asyncio.sleep(0.05)
        assert process.busy
        assert not queued_call.done()

        # Rotation/timeout invalidation cancels the started call but must
        # cold-retry the queued, never-started one instead of dropping it.
        await adapter._invalidate_process(process)
        with pytest.raises(asyncio.CancelledError):
            await busy_call
        result = await queued_call
        assert result == "queued-done"
        assert len(calls) == 1

        new_process = adapter._persistent_processes[identity]
        assert new_process is not process
        pid_two = int(Path(pid_file).read_text())
        assert pid_two != pid_one
        with pytest.raises(ProcessLookupError):
            os.kill(pid_one, 0)
        await adapter.shutdown()

    async def test_persistent_dispatch_refuses_closing_generation_and_cold_starts(self, tmp_path):
        pid_file = str(tmp_path / "mcp-pid")
        env = {
            **TEST_ENV,
            MCP_POOL_SOURCE_ID_ENV: "source",
            MCP_POOL_USER_ID_ENV: "user",
            "TEST_PID_FILE": pid_file,
        }
        adapter = McpAdapter(
            StdioMcpServer(command=sys.executable, args=[TEST_SERVER], persistent=True),
            idle_timeout_seconds=60,
        )
        identity = '["source","user"]'
        await adapter.execute_tool("greet", {"name": "first"}, env=env)
        process = adapter._persistent_processes[identity]
        pid_one = int(Path(pid_file).read_text())

        # A dispatch that finds its generation closing at the enqueue line is
        # refused instead of dropped; without retry budget it surfaces as a
        # retryable error rather than a cancellation of the HTTP request.
        process.closing = True
        with pytest.raises(McpProcessClosedError):
            await adapter._dispatch_persistent(
                process, lambda session: session.list_tools(), 60, env, 0
            )

        result = await adapter.execute_tool("greet", {"name": "second"}, env=env)
        assert result.status == "success"
        new_process = adapter._persistent_processes[identity]
        assert new_process is not process
        pid_two = int(Path(pid_file).read_text())
        assert pid_two != pid_one
        with pytest.raises(ProcessLookupError):
            os.kill(pid_one, 0)
        await adapter.shutdown()

    async def test_persistent_busy_rotation_cancels_old_generation(self):
        adapter = McpAdapter(
            StdioMcpServer(command=sys.executable, args=[TEST_SERVER], persistent=True),
            idle_timeout_seconds=60,
        )
        env = {
            **TEST_ENV,
            MCP_POOL_SOURCE_ID_ENV: "source",
            MCP_POOL_USER_ID_ENV: "user",
            "TOKEN": "old",
        }
        entered = asyncio.Event()

        async def wait_forever(session):
            entered.set()
            await asyncio.Event().wait()

        old_call = asyncio.create_task(adapter._run(wait_forever, env=env))
        await entered.wait()
        rotated = await adapter.execute_tool(
            "greet", {"name": "new token"}, env={**env, "TOKEN": "new"}
        )
        assert rotated.status == "success"
        with pytest.raises(asyncio.CancelledError):
            await old_call
        await adapter.shutdown()

    async def test_persistent_auth_marker_and_failed_startup_recovery(self, tmp_path):
        marker = tmp_path / "auth-status"
        marker.write_text("needs_user_auth")
        adapter = McpAdapter(
            StdioMcpServer(command="missing-mcp-server", persistent=True),
            idle_timeout_seconds=60,
        )
        env = {
            **TEST_ENV,
            MCP_POOL_SOURCE_ID_ENV: "source",
            MCP_POOL_USER_ID_ENV: "user",
            MCP_AUTH_STATUS_FILE_ENV: str(marker),
        }
        first = asyncio.create_task(adapter._run(lambda session: session.list_tools(), env=env))
        second = asyncio.create_task(adapter._run(lambda session: session.list_tools(), env=env))
        for task in (first, second):
            with pytest.raises(RuntimeError, match=MCP_AUTH_REQUIRED_MESSAGE):
                await task
        assert not marker.exists()
        assert adapter._persistent_processes == {}
        adapter._server = StdioMcpServer(
            command=sys.executable, args=[TEST_SERVER], persistent=True
        )
        result = await adapter.execute_tool(
            "greet",
            {"name": "recovered"},
            env={
                **TEST_ENV,
                MCP_POOL_SOURCE_ID_ENV: "source",
                MCP_POOL_USER_ID_ENV: "user",
            },
        )
        assert result.status == "success"
        await adapter.shutdown()

    async def test_cache_survives_connection_failure(self):
        """After successful discovery, cache is returned if subprocess can't start."""
        adapter = McpAdapter(TEST_STDIO_SERVER)
        await adapter.discover(env=TEST_ENV)
        assert len(adapter._cached_actions or []) == 2

        # Replace command with something that will fail
        adapter._server = StdioMcpServer(command="nonexistent-binary", args=[])
        cached = await adapter.get_action_definitions(env=TEST_ENV)
        assert len(cached) == 2
        assert {a.name for a in cached} == {"greet", "add"}


class TestHttpAdapter:
    """Streamable HTTP transport against the same fixture server."""

    async def test_get_action_definitions(self, http_server_url: str):
        adapter = McpAdapter(HttpMcpServer(url=http_server_url))
        actions = await adapter.get_action_definitions(headers={"X-Test": "1"})
        names = {a.name for a in actions}
        assert names == {"greet", "add"}

    async def test_execute_tool(self, http_server_url: str):
        adapter = McpAdapter(HttpMcpServer(url=http_server_url))
        result = await adapter.execute_tool("greet", {"name": "Remote"}, headers={"X-Test": "1"})
        assert result.status == "success"
        assert result.result is not None
        assert "Hello, Remote!" in result.result.get("content", "")

    async def test_read_resource(self, http_server_url: str):
        adapter = McpAdapter(HttpMcpServer(url=http_server_url))
        result = await adapter.read_resource("test://item/99", headers={"X-Test": "1"})
        assert "contents" in result and len(result["contents"]) >= 1

    async def test_get_prompt(self, http_server_url: str):
        adapter = McpAdapter(HttpMcpServer(url=http_server_url))
        result = await adapter.get_prompt(
            "summarize", {"text": "remote text"}, headers={"X-Test": "1"}
        )
        assert "messages" in result and len(result["messages"]) >= 1

    async def test_discover_caches_definitions(self, http_server_url: str):
        adapter = McpAdapter(HttpMcpServer(url=http_server_url))
        await adapter.discover(headers={"X-Test": "1"})
        # No headers — returns cache
        assert {a.name for a in await adapter.get_action_definitions()} == {
            "greet",
            "add",
        }
        assert len(await adapter.get_resource_definitions()) == 1
        assert len(await adapter.get_prompt_definitions()) == 1

    async def test_static_headers_merged_with_per_call_headers(self, http_server_url: str):
        """Static headers on HttpMcpServer + per-call headers both reach the server."""
        adapter = McpAdapter(HttpMcpServer(url=http_server_url, headers={"X-Static": "yes"}))
        # No assertion on headers here (the test fixture doesn't expose them);
        # we just ensure the call succeeds with both sets configured.
        actions = await adapter.get_action_definitions(headers={"X-Per-Call": "yes"})
        assert len(actions) == 2


class TestConnectorMcpIntegration:
    """A Connector with an MCP server config delegates correctly."""

    @pytest.fixture
    def stdio_connector(self) -> Connector:
        class StdioMcpConnector(Connector):
            @property
            def name(self) -> str:
                return "mcp-test-stdio"

            @property
            def version(self) -> str:
                return "0.1.0"

            @property
            def source_types(self) -> list[str]:
                return ["mcp_test"]

            @property
            def mcp_server(self) -> StdioMcpServer:
                return TEST_STDIO_SERVER

            async def sync(
                self,
                source_config: dict[str, Any],
                credentials: dict[str, Any],
                checkpoint: dict[str, Any] | None,
                ctx: Any,
            ) -> None:
                pass

        return StdioMcpConnector()

    async def test_manifest_is_catalogless_without_authenticated_discovery(
        self, stdio_connector: Connector
    ):
        manifest = await stdio_connector.get_manifest(connector_url="http://test:8000")
        assert manifest.mcp_enabled is True
        assert manifest.mcp_catalog_loaded is False
        assert {a.name for a in manifest.actions}.isdisjoint({"greet", "add"})
        assert all(a.origin == "native" for a in manifest.actions)

    async def test_manifest_includes_mcp_tools_as_actions(self, stdio_connector: Connector):
        await stdio_connector.bootstrap_mcp({"token": "test"})
        manifest = await stdio_connector.get_manifest(connector_url="http://test:8000")
        assert manifest.mcp_enabled is True
        action_names = {a.name for a in manifest.actions}
        assert "greet" in action_names
        assert "add" in action_names
        assert all(a.origin == "mcp" for a in manifest.actions if a.name in {"greet", "add"})

    async def test_manifest_includes_resources(self, stdio_connector: Connector):
        await stdio_connector.bootstrap_mcp({"token": "test"})
        manifest = await stdio_connector.get_manifest(connector_url="http://test:8000")
        assert len(manifest.resources) == 1
        assert manifest.resources[0].uri_template == "test://item/{item_id}"

    async def test_manifest_includes_prompts(self, stdio_connector: Connector):
        await stdio_connector.bootstrap_mcp({"token": "test"})
        manifest = await stdio_connector.get_manifest(connector_url="http://test:8000")
        assert len(manifest.prompts) == 1
        assert manifest.prompts[0].name == "summarize"
        assert len(manifest.skills) == 1
        assert manifest.skills[0].id == "mcp:summarize"

    async def test_execute_action_delegates_to_mcp(self, stdio_connector: Connector):
        # MCP dispatch is owned by the HTTP server so connector-level native
        # actions cannot accidentally fall through to an MCP tool.
        result = await stdio_connector.mcp_adapter.execute_tool(
            "greet", {"name": "Omni"}, env=TEST_ENV
        )
        assert result.status == "success"

    async def test_execute_action_unknown_returns_not_supported(self, stdio_connector: Connector):
        result = await stdio_connector.mcp_adapter.execute_tool("unknown_action", {}, env=TEST_ENV)
        assert result.status == "error"

    async def test_http_connector_round_trip(self, http_server_url: str):
        """A Connector pointing at an HttpMcpServer surfaces tools and dispatches."""

        class HttpMcpConnector(Connector):
            @property
            def name(self) -> str:
                return "mcp-test-http"

            @property
            def version(self) -> str:
                return "0.1.0"

            @property
            def source_types(self) -> list[str]:
                return ["mcp_test_http"]

            @property
            def mcp_server(self) -> HttpMcpServer:
                return HttpMcpServer(url=http_server_url)

            def prepare_mcp_headers(self, credentials: dict[str, Any]) -> dict[str, str]:
                return {"Authorization": f"Bearer {credentials.get('token', '')}"}

            async def sync(self, *args: Any, **kwargs: Any) -> None:
                pass

        connector = HttpMcpConnector()
        await connector.bootstrap_mcp({"token": "abc"})
        manifest = await connector.get_manifest(connector_url="http://test:8000")
        assert manifest.mcp_enabled is True
        assert {a.name for a in manifest.actions} >= {"greet", "add"}

        result = await connector.mcp_adapter.execute_tool(
            "greet", {"name": "HTTP"}, headers={"Authorization": "Bearer abc"}
        )
        assert result.status == "success"

    async def test_non_mcp_connector_manifest(self):
        class PlainConnector(Connector):
            @property
            def name(self) -> str:
                return "plain"

            @property
            def version(self) -> str:
                return "0.1.0"

            @property
            def source_types(self) -> list[str]:
                return ["plain"]

            async def sync(self, *args: Any, **kwargs: Any) -> None:
                pass

        connector = PlainConnector()
        manifest = await connector.get_manifest(connector_url="http://test:8000")
        assert manifest.mcp_enabled is False
        assert manifest.resources == []
        assert manifest.prompts == []
