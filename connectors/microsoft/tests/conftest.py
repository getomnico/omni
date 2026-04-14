"""Integration test fixtures for the Microsoft connector.

Session-scoped: harness, mock Graph API server, connector server, connector-manager.
Function-scoped: seed helper, per-type source_id fixtures, httpx client.
"""

from __future__ import annotations

import logging
import socket
import threading
import time
from typing import Any

import httpx
import pytest
import pytest_asyncio
import uvicorn
from starlette.applications import Starlette
from starlette.requests import Request
from starlette.responses import JSONResponse, Response
from starlette.routing import Route

from omni_connector.testing import OmniTestHarness, SeedHelper

logger = logging.getLogger(__name__)


# ---------------------------------------------------------------------------
# Mock Graph API
# ---------------------------------------------------------------------------


class MockGraphAPI:
    """Controllable mock of the Microsoft Graph API v1.0 endpoints."""

    def __init__(self) -> None:
        self.users: list[dict[str, Any]] = []
        self.drive_items: dict[str, list[dict[str, Any]]] = {}
        self.mail_messages: dict[str, list[dict[str, Any]]] = {}
        self.calendar_events: dict[str, list[dict[str, Any]]] = {}
        self.sites: list[dict[str, Any]] = []
        self.site_drive_items: dict[str, list[dict[str, Any]]] = {}
        # site_id -> [drive dicts]; drive_id -> [item dicts]
        self.site_drives: dict[str, list[dict[str, Any]]] = {}
        self.drive_items_by_drive: dict[str, list[dict[str, Any]]] = {}
        # site_id -> {"status": int, "body": dict} for forcing failures
        self.site_drives_errors: dict[str, dict[str, Any]] = {}
        self.drive_delta_errors: dict[str, list[dict[str, Any]]] = {}
        self.site_diagnostics: dict[str, dict[str, Any]] = {}
        self.file_contents: dict[str, bytes] = {}
        self.groups: list[dict[str, Any]] = []
        self.group_members: dict[str, list[dict[str, Any]]] = {}
        self.item_permissions: dict[str, list[dict[str, Any]]] = {}
        # Teams
        self.teams: list[dict[str, Any]] = []
        self.team_channels: dict[str, list[dict[str, Any]]] = {}
        self.channel_messages: dict[str, list[dict[str, Any]]] = {}
        self.message_replies: dict[str, list[dict[str, Any]]] = {}
        self.channel_members: dict[str, list[dict[str, Any]]] = {}
        self.share_drive_items: dict[str, dict[str, Any]] = {}
        self.message_attachments: dict[str, list[dict[str, Any]]] = {}

    def reset(self) -> None:
        self.users.clear()
        self.drive_items.clear()
        self.mail_messages.clear()
        self.calendar_events.clear()
        self.sites.clear()
        self.site_drive_items.clear()
        self.site_drives.clear()
        self.drive_items_by_drive.clear()
        self.site_drives_errors.clear()
        self.drive_delta_errors.clear()
        self.site_diagnostics.clear()
        self.file_contents.clear()
        self.groups.clear()
        self.group_members.clear()
        self.item_permissions.clear()
        self.teams.clear()
        self.team_channels.clear()
        self.channel_messages.clear()
        self.message_replies.clear()
        self.channel_members.clear()
        self.share_drive_items.clear()
        self.message_attachments.clear()

    def add_user(self, user: dict[str, Any]) -> None:
        self.users.append(user)

    def add_drive_item(self, user_id: str, item: dict[str, Any]) -> None:
        self.drive_items.setdefault(user_id, []).append(item)

    def add_mail_message(self, user_id: str, message: dict[str, Any]) -> None:
        self.mail_messages.setdefault(user_id, []).append(message)

    def add_calendar_event(self, user_id: str, event: dict[str, Any]) -> None:
        self.calendar_events.setdefault(user_id, []).append(event)

    def add_site(self, site: dict[str, Any]) -> None:
        self.sites.append(site)

    def add_site_drive(self, site_id: str, drive: dict[str, Any]) -> None:
        self.site_drives.setdefault(site_id, []).append(drive)

    def add_drive_item(self, drive_id: str, item: dict[str, Any]) -> None:
        self.drive_items_by_drive.setdefault(drive_id, []).append(item)

    def add_site_drive_item(self, site_id: str, item: dict[str, Any]) -> None:
        """Backwards-compat helper: registers a default drive for the site
        (deriving the drive id from the item's parentReference) and adds the
        item to that drive."""
        drive_id = item.get("parentReference", {}).get("driveId")
        if drive_id is None:
            raise ValueError(
                "add_site_drive_item now requires parentReference.driveId on the item"
            )
        self.site_drive_items.setdefault(site_id, []).append(item)
        if not any(d["id"] == drive_id for d in self.site_drives.get(site_id, [])):
            self.add_site_drive(
                site_id,
                {"id": drive_id, "name": "Documents", "driveType": "documentLibrary"},
            )
        self.add_drive_item(drive_id, item)

    def set_site_drives_error(
        self, site_id: str, status: int, body: dict[str, Any] | None = None
    ) -> None:
        self.site_drives_errors[site_id] = {"status": status, "body": body or {}}

    def queue_drive_delta_error(
        self, drive_id: str, status: int, body: dict[str, Any] | None = None
    ) -> None:
        """Queue a one-shot error response for the next /drives/{id}/root/delta
        call. Subsequent calls return the normal item list."""
        self.drive_delta_errors.setdefault(drive_id, []).append(
            {"status": status, "body": body or {}}
        )

    def set_site_diagnostic(self, site_id: str, payload: dict[str, Any]) -> None:
        self.site_diagnostics[site_id] = payload

    def set_file_content(self, drive_id: str, item_id: str, content: bytes) -> None:
        self.file_contents[f"{drive_id}:{item_id}"] = content

    def add_group(self, group: dict[str, Any]) -> None:
        self.groups.append(group)

    def add_group_member(self, group_id: str, member: dict[str, Any]) -> None:
        self.group_members.setdefault(group_id, []).append(member)

    def set_item_permissions(
        self, drive_id: str, item_id: str, permissions: list[dict[str, Any]]
    ) -> None:
        self.item_permissions[f"{drive_id}:{item_id}"] = permissions

    def add_team(self, team: dict[str, Any]) -> None:
        self.teams.append(team)

    def add_team_channel(self, team_id: str, channel: dict[str, Any]) -> None:
        self.team_channels.setdefault(team_id, []).append(channel)

    def add_channel_message(
        self, team_id: str, channel_id: str, message: dict[str, Any]
    ) -> None:
        key = f"{team_id}:{channel_id}"
        self.channel_messages.setdefault(key, []).append(message)

    def add_message_reply(
        self, team_id: str, channel_id: str, message_id: str, reply: dict[str, Any]
    ) -> None:
        key = f"{team_id}:{channel_id}:{message_id}"
        self.message_replies.setdefault(key, []).append(reply)

    def add_channel_member(
        self, team_id: str, channel_id: str, member: dict[str, Any]
    ) -> None:
        key = f"{team_id}:{channel_id}"
        self.channel_members.setdefault(key, []).append(member)

    def add_message_attachment(
        self, user_id: str, message_id: str, attachment: dict[str, Any]
    ) -> None:
        key = f"{user_id}:{message_id}"
        self.message_attachments.setdefault(key, []).append(attachment)

    def set_share_drive_item(
        self, share_token: str, drive_item: dict[str, Any]
    ) -> None:
        self.share_drive_items[share_token] = drive_item

    def create_app(self, base_url: str) -> Starlette:
        mock = self

        async def organization(request: Request) -> JSONResponse:
            return JSONResponse(
                {"value": [{"id": "org-001", "displayName": "Test Org"}]}
            )

        async def list_users(request: Request) -> JSONResponse:
            return JSONResponse({"value": mock.users})

        async def user_drive_delta(request: Request) -> JSONResponse:
            uid = request.path_params["uid"]
            items = mock.drive_items.get(uid, [])
            delta_link = f"{base_url}/users/{uid}/drive/root/delta?deltatoken=latest"
            return JSONResponse({"value": items, "@odata.deltaLink": delta_link})

        async def drive_item_content(request: Request) -> Response:
            did = request.path_params["did"]
            iid = request.path_params["iid"]
            key = f"{did}:{iid}"
            content = mock.file_contents.get(key, b"file content placeholder")
            return Response(content=content, media_type="application/octet-stream")

        async def mail_delta(request: Request) -> JSONResponse:
            uid = request.path_params["uid"]
            messages = mock.mail_messages.get(uid, [])
            # Respect $filter on receivedDateTime for max-age testing
            filter_param = request.query_params.get("$filter", "")
            if "receivedDateTime ge " in filter_param:
                cutoff_str = filter_param.split("receivedDateTime ge ")[1].strip()
                messages = [
                    m for m in messages if m.get("receivedDateTime", "") >= cutoff_str
                ]
            delta_link = (
                f"{base_url}/users/{uid}/mailFolders/inbox/messages/delta"
                f"?deltatoken=latest"
            )
            return JSONResponse({"value": messages, "@odata.deltaLink": delta_link})

        async def calendar_delta(request: Request) -> JSONResponse:
            uid = request.path_params["uid"]
            events = mock.calendar_events.get(uid, [])
            delta_link = f"{base_url}/users/{uid}/calendarView/delta?deltatoken=latest"
            return JSONResponse({"value": events, "@odata.deltaLink": delta_link})

        async def item_permissions(request: Request) -> JSONResponse:
            did = request.path_params["did"]
            iid = request.path_params["iid"]
            key = f"{did}:{iid}"
            perms = mock.item_permissions.get(key, [])
            return JSONResponse({"value": perms})

        async def list_groups(request: Request) -> JSONResponse:
            filter_param = request.query_params.get("$filter", "")
            if "MCO" in filter_param:
                return JSONResponse({"value": mock.teams})
            return JSONResponse({"value": mock.groups})

        async def group_members(request: Request) -> JSONResponse:
            gid = request.path_params["gid"]
            members = mock.group_members.get(gid, [])
            return JSONResponse({"value": members})

        async def list_sites(request: Request) -> JSONResponse:
            return JSONResponse({"value": mock.sites})

        async def site_drive_delta(request: Request) -> JSONResponse:
            sid = request.path_params["sid"]
            items = mock.site_drive_items.get(sid, [])
            delta_link = f"{base_url}/sites/{sid}/drive/root/delta?deltatoken=latest"
            return JSONResponse({"value": items, "@odata.deltaLink": delta_link})

        async def site_drives_list(request: Request) -> JSONResponse:
            sid = request.path_params["sid"]
            err = mock.site_drives_errors.get(sid)
            if err is not None:
                return JSONResponse(err["body"], status_code=err["status"])
            return JSONResponse({"value": mock.site_drives.get(sid, [])})

        async def site_get(request: Request) -> JSONResponse:
            sid = request.path_params["sid"]
            payload = mock.site_diagnostics.get(sid)
            if payload is None:
                return JSONResponse(
                    {"error": {"code": "itemNotFound"}}, status_code=404
                )
            return JSONResponse(payload)

        async def drive_root_delta(request: Request) -> JSONResponse:
            did = request.path_params["did"]
            queued = mock.drive_delta_errors.get(did)
            if queued:
                err = queued.pop(0)
                return JSONResponse(err["body"], status_code=err["status"])
            items = mock.drive_items_by_drive.get(did, [])
            delta_link = f"{base_url}/v1.0/drives/{did}/root/delta?deltatoken=latest"
            return JSONResponse({"value": items, "@odata.deltaLink": delta_link})

        async def team_channels(request: Request) -> JSONResponse:
            tid = request.path_params["tid"]
            channels = mock.team_channels.get(tid, [])
            return JSONResponse({"value": channels})

        async def channel_messages_delta(request: Request) -> JSONResponse:
            tid = request.path_params["tid"]
            cid = request.path_params["cid"]
            key = f"{tid}:{cid}"
            messages = mock.channel_messages.get(key, [])
            delta_link = (
                f"{base_url}/teams/{tid}/channels/{cid}/messages/delta"
                f"?deltatoken=latest"
            )
            return JSONResponse({"value": messages, "@odata.deltaLink": delta_link})

        async def message_replies(request: Request) -> JSONResponse:
            tid = request.path_params["tid"]
            cid = request.path_params["cid"]
            mid = request.path_params["mid"]
            key = f"{tid}:{cid}:{mid}"
            replies = mock.message_replies.get(key, [])
            return JSONResponse({"value": replies})

        async def channel_members(request: Request) -> JSONResponse:
            tid = request.path_params["tid"]
            cid = request.path_params["cid"]
            key = f"{tid}:{cid}"
            members = mock.channel_members.get(key, [])
            return JSONResponse({"value": members})

        async def mail_attachments(request: Request) -> JSONResponse:
            uid = request.path_params["uid"]
            mid = request.path_params["mid"]
            key = f"{uid}:{mid}"
            attachments = mock.message_attachments.get(key, [])
            return JSONResponse({"value": attachments})

        async def resolve_share(request: Request) -> JSONResponse:
            token = request.path_params["token"]
            drive_item = mock.share_drive_items.get(token)
            if drive_item is None:
                return JSONResponse(
                    {"error": {"code": "itemNotFound"}}, status_code=404
                )
            return JSONResponse(drive_item)

        routes = [
            Route("/v1.0/organization", organization),
            Route("/v1.0/users", list_users),
            Route("/v1.0/users/{uid}/drive/root/delta", user_drive_delta),
            Route("/v1.0/drives/{did}/items/{iid}/content", drive_item_content),
            Route("/v1.0/drives/{did}/items/{iid}/permissions", item_permissions),
            Route(
                "/v1.0/users/{uid}/mailFolders/inbox/messages/delta",
                mail_delta,
            ),
            Route(
                "/v1.0/users/{uid}/messages/{mid}/attachments",
                mail_attachments,
            ),
            Route("/v1.0/users/{uid}/calendarView/delta", calendar_delta),
            Route("/v1.0/groups", list_groups),
            Route("/v1.0/groups/{gid}/members", group_members),
            Route("/v1.0/sites", list_sites),
            Route("/v1.0/sites/{sid}", site_get),
            Route("/v1.0/sites/{sid}/drives", site_drives_list),
            Route("/v1.0/sites/{sid}/drive/root/delta", site_drive_delta),
            Route("/v1.0/drives/{did}/root/delta", drive_root_delta),
            Route("/v1.0/teams/{tid}/channels", team_channels),
            Route(
                "/v1.0/teams/{tid}/channels/{cid}/messages/delta",
                channel_messages_delta,
            ),
            Route(
                "/v1.0/teams/{tid}/channels/{cid}/messages/{mid}/replies",
                message_replies,
            ),
            Route("/v1.0/teams/{tid}/channels/{cid}/members", channel_members),
            Route("/v1.0/shares/{token}/driveItem", resolve_share),
        ]
        return Starlette(routes=routes)


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("", 0))
        return s.getsockname()[1]


def _wait_for_port(port: int, host: str = "localhost", timeout: float = 10) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            with socket.create_connection((host, port), timeout=1):
                return
        except OSError:
            time.sleep(0.1)
    raise TimeoutError(f"Port {port} not open after {timeout}s")


def _wait_for_registration(cm_url: str, source_type: str, timeout: float = 15) -> None:
    """Poll CM until the connector has registered itself.

    Registration runs asynchronously inside the connector's lifespan loop, so
    binding the port doesn't guarantee the manifest is in CM's Redis yet.
    """
    deadline = time.monotonic() + timeout
    last_err: Exception | None = None
    last_payload: object = None
    while time.monotonic() < deadline:
        try:
            resp = httpx.get(f"{cm_url}/connectors", timeout=2)
            if resp.status_code == 200:
                manifests = resp.json()
                last_payload = manifests
                for m in manifests:
                    if m.get("source_type") == source_type and m.get("healthy"):
                        return
        except Exception as e:
            last_err = e
        time.sleep(0.2)
    raise TimeoutError(
        f"Connector did not register source_type={source_type} within "
        f"{timeout}s: last_err={last_err}"
    )


async def _create_ms_source(
    seed: SeedHelper,
    mock_graph_server: str,
    mock_graph_api: MockGraphAPI,
    source_type: str,
    user_filter_mode: str = "all",
    user_whitelist: list[str] | None = None,
    user_blacklist: list[str] | None = None,
) -> str:
    mock_graph_api.reset()
    sid = await seed.create_source(
        source_type=source_type,
        config={"graph_base_url": f"{mock_graph_server}/v1.0"},
        user_filter_mode=user_filter_mode,
        user_whitelist=user_whitelist,
        user_blacklist=user_blacklist,
    )
    await seed.create_credentials(sid, {"token": "test-token"}, provider="microsoft")
    return sid


# ---------------------------------------------------------------------------
# Session-scoped fixtures
# ---------------------------------------------------------------------------


@pytest.fixture(scope="session")
def mock_graph_api() -> MockGraphAPI:
    return MockGraphAPI()


@pytest.fixture(scope="session")
def mock_graph_server(mock_graph_api: MockGraphAPI) -> str:
    """Start mock Graph API server in a daemon thread. Returns base URL."""
    port = _free_port()
    base_url = f"http://localhost:{port}"
    app = mock_graph_api.create_app(base_url)
    config = uvicorn.Config(app, host="0.0.0.0", port=port, log_level="warning")
    server = uvicorn.Server(config)

    thread = threading.Thread(target=server.run, daemon=True)
    thread.start()

    _wait_for_port(port)
    return base_url


@pytest.fixture(scope="session")
def connector_port() -> int:
    return _free_port()


@pytest_asyncio.fixture(scope="session")
async def harness() -> OmniTestHarness:
    """Session-scoped OmniTestHarness with infra + connector-manager started.

    Does not depend on `connector_server` — CM learns of connectors via the
    `/register` call the connector itself makes after startup, not via any
    pre-known URL. Starting the harness first means `connector_server` can
    set CONNECTOR_MANAGER_URL to the real CM URL before SdkClient caches it.
    """
    h = OmniTestHarness()
    await h.start_infra()
    await h.start_connector_manager()

    yield h
    await h.teardown()


@pytest.fixture(scope="session")
def connector_server(connector_port: int, harness: OmniTestHarness) -> str:
    """Start the Microsoft connector as a uvicorn server in a daemon thread.

    Depends on `harness` so we can point CONNECTOR_MANAGER_URL at the real CM
    before `create_app` runs (SdkClient reads the env var once at construction
    and caches it).
    """
    import os

    os.environ["CONNECTOR_MANAGER_URL"] = harness.connector_manager_url
    # Connector advertises this hostname in its registration manifest. CM
    # health-checks the URL from inside its container, so localhost would
    # resolve to the container itself — use host.docker.internal instead.
    os.environ["CONNECTOR_HOST_NAME"] = "host.docker.internal"
    os.environ.setdefault("PORT", str(connector_port))

    from ms_connector import MicrosoftConnector
    from omni_connector.server import create_app

    app = create_app(MicrosoftConnector())
    config = uvicorn.Config(
        app, host="0.0.0.0", port=connector_port, log_level="warning"
    )
    server = uvicorn.Server(config)

    thread = threading.Thread(target=server.run, daemon=True)
    thread.start()

    _wait_for_port(connector_port)
    _wait_for_registration(harness.connector_manager_url, "one_drive")
    return f"http://localhost:{connector_port}"


# ---------------------------------------------------------------------------
# Function-scoped fixtures
# ---------------------------------------------------------------------------


@pytest_asyncio.fixture
async def seed(harness: OmniTestHarness) -> SeedHelper:
    return harness.seed()


@pytest_asyncio.fixture
async def onedrive_source_id(
    connector_server: str,
    seed: SeedHelper,
    mock_graph_server: str,
    mock_graph_api: MockGraphAPI,
) -> str:
    return await _create_ms_source(seed, mock_graph_server, mock_graph_api, "one_drive")


@pytest_asyncio.fixture
async def sharepoint_source_id(
    connector_server: str,
    seed: SeedHelper,
    mock_graph_server: str,
    mock_graph_api: MockGraphAPI,
) -> str:
    return await _create_ms_source(
        seed, mock_graph_server, mock_graph_api, "share_point"
    )


@pytest_asyncio.fixture
async def outlook_source_id(
    connector_server: str,
    seed: SeedHelper,
    mock_graph_server: str,
    mock_graph_api: MockGraphAPI,
) -> str:
    return await _create_ms_source(seed, mock_graph_server, mock_graph_api, "outlook")


@pytest_asyncio.fixture
async def outlook_calendar_source_id(
    connector_server: str,
    seed: SeedHelper,
    mock_graph_server: str,
    mock_graph_api: MockGraphAPI,
) -> str:
    return await _create_ms_source(
        seed, mock_graph_server, mock_graph_api, "outlook_calendar"
    )


@pytest_asyncio.fixture
async def ms_teams_source_id(
    connector_server: str,
    seed: SeedHelper,
    mock_graph_server: str,
    mock_graph_api: MockGraphAPI,
) -> str:
    return await _create_ms_source(seed, mock_graph_server, mock_graph_api, "ms_teams")


@pytest_asyncio.fixture
async def cm_client(harness: OmniTestHarness) -> httpx.AsyncClient:
    """Async httpx client pointed at the connector-manager."""
    async with httpx.AsyncClient(
        base_url=harness.connector_manager_url, timeout=30
    ) as client:
        yield client
