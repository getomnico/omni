import asyncio
import logging
import os
from contextlib import asynccontextmanager
from typing import TYPE_CHECKING, Any

from fastapi import FastAPI, status
from fastapi.responses import JSONResponse
from starlette.responses import Response

from .client import SdkClient
from .context import SyncContext
from .exceptions import SdkClientError
from .models import (
    ActionRequest,
    CancelRequest,
    CancelResponse,
    PromptRequest,
    ResourceRequest,
    SyncMode,
    SyncRequest,
    SyncResponse,
)

if TYPE_CHECKING:
    from .connector import Connector

logger = logging.getLogger(__name__)

REGISTRATION_INTERVAL_SECONDS = 30


class ConnectorServer:
    """HTTP server wrapper for a connector."""

    def __init__(self, connector: "Connector"):
        self.connector = connector
        self.active_syncs: dict[str, SyncContext] = {}
        self._sdk_client: SdkClient | None = None

    @property
    def sdk_client(self) -> SdkClient:
        if self._sdk_client is None:
            self._sdk_client = SdkClient.from_env()
        return self._sdk_client


def _build_connector_url() -> str:
    hostname = os.environ.get("CONNECTOR_HOST_NAME")
    if not hostname:
        raise RuntimeError(
            "CONNECTOR_HOST_NAME environment variable is required. "
            "Set it to this connector's hostname (e.g. the Docker service name)."
        )
    port = os.environ.get("PORT")
    if not port:
        raise RuntimeError("PORT environment variable is required.")
    return f"http://{hostname}:{port}"


def create_app(connector: "Connector") -> FastAPI:
    """Create FastAPI app for a connector."""

    server = ConnectorServer(connector)
    connector_url = _build_connector_url()

    if not os.environ.get("CONNECTOR_MANAGER_URL"):
        raise RuntimeError(
            "CONNECTOR_MANAGER_URL environment variable is required for connector registration."
        )

    @asynccontextmanager
    async def lifespan(app: FastAPI):  # noqa: ARG001
        async def registration_loop() -> None:
            while True:
                try:
                    manifest = await connector.get_manifest(connector_url=connector_url)
                    await server.sdk_client.register(manifest.model_dump())
                    logger.info("Registered with connector manager")
                except Exception as e:
                    logger.warning("Registration failed: %s", e)
                await asyncio.sleep(REGISTRATION_INTERVAL_SECONDS)

        registration_task = asyncio.create_task(registration_loop())

        yield

        registration_task.cancel()

    app = FastAPI(
        title=f"Omni {connector.name} Connector",
        version=connector.version,
        lifespan=lifespan,
    )

    @app.get("/health")
    async def health() -> dict[str, str]:
        return {"status": "healthy", "service": connector.name}

    @app.get("/manifest")
    async def manifest() -> dict[str, Any]:
        m = await connector.get_manifest(connector_url=connector_url)
        return m.model_dump()

    @app.get("/sync/{sync_run_id}")
    async def sync_status(sync_run_id: str) -> dict[str, bool]:
        running = any(
            ctx.sync_run_id == sync_run_id for ctx in server.active_syncs.values()
        )
        return {"running": running}

    @app.post("/sync")
    async def trigger_sync(request: SyncRequest) -> JSONResponse:
        sync_run_id = request.sync_run_id
        source_id = request.source_id

        logger.info(
            "Sync triggered for source %s (sync_run_id: %s)",
            source_id,
            sync_run_id,
        )

        if source_id in server.active_syncs:
            return JSONResponse(
                status_code=status.HTTP_409_CONFLICT,
                content=SyncResponse.error(
                    "Sync already in progress for this source"
                ).model_dump(),
            )

        try:
            sync_data = await server.sdk_client.fetch_source_sync_data(source_id)
            source_config = sync_data.config
            credentials = sync_data.credentials
            state = sync_data.connector_state
            source_type = sync_data.source_type
        except SdkClientError as e:
            error_msg = str(e)
            if "404" in error_msg:
                return JSONResponse(
                    status_code=status.HTTP_404_NOT_FOUND,
                    content=SyncResponse.error(
                        f"Source not found: {source_id}"
                    ).model_dump(),
                )
            logger.error("Failed to fetch source data: %s", e)
            return JSONResponse(
                status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                content=SyncResponse.error(
                    f"Failed to fetch source data: {e}"
                ).model_dump(),
            )
        except Exception as e:
            logger.error("Failed to fetch source data: %s", e)
            return JSONResponse(
                status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                content=SyncResponse.error(
                    f"Failed to fetch source data: {e}"
                ).model_dump(),
            )

        # Bootstrap MCP subprocess with credentials (populates tool cache for manifest)
        await connector.bootstrap_mcp(credentials)

        try:
            sync_mode = SyncMode(request.sync_mode)
        except ValueError:
            logger.warning(
                "Unknown sync_mode %r; defaulting to Incremental batching",
                request.sync_mode,
            )
            sync_mode = SyncMode.INCREMENTAL

        ctx = SyncContext(
            sdk_client=server.sdk_client,
            sync_run_id=sync_run_id,
            source_id=source_id,
            source_type=source_type,
            state=state,
            user_filter_mode=sync_data.user_filter_mode,
            user_whitelist=sync_data.user_whitelist,
            user_blacklist=sync_data.user_blacklist,
            sync_mode=sync_mode,
            documents_scanned=request.documents_scanned,
            documents_updated=request.documents_updated,
        )
        server.active_syncs[source_id] = ctx

        async def run_sync() -> None:
            try:
                await connector.sync(source_config, credentials, state, ctx)
            except Exception as e:
                logger.error("Sync %s failed: %s", sync_run_id, e)
                try:
                    await ctx.fail(str(e))
                except Exception as fail_error:
                    logger.error("Failed to report sync failure: %s", fail_error)
            finally:
                server.active_syncs.pop(source_id, None)

        asyncio.create_task(run_sync())

        return JSONResponse(
            status_code=status.HTTP_200_OK,
            content=SyncResponse.started().model_dump(),
        )

    @app.post("/cancel")
    async def cancel_sync(request: CancelRequest) -> dict[str, str]:
        sync_run_id = request.sync_run_id
        logger.info("Cancel requested for sync %s", sync_run_id)

        for source_id, ctx in server.active_syncs.items():
            if ctx.sync_run_id == sync_run_id:
                ctx._set_cancelled()
                connector.cancel(sync_run_id)
                return CancelResponse(status="cancelled").model_dump()

        return CancelResponse(status="not_found").model_dump()

    @app.post("/action")
    async def execute_action(request: ActionRequest) -> Response:
        logger.info("Action requested: %s", request.action)
        return await connector.execute_action(
            request.action,
            request.params,
            request.credentials,
        )

    @app.post("/resource")
    async def read_resource(request: ResourceRequest) -> JSONResponse:
        adapter = connector.mcp_adapter
        if adapter is None:
            return JSONResponse(
                status_code=status.HTTP_404_NOT_FOUND,
                content={"error": "MCP not enabled for this connector"},
            )
        logger.info("Resource requested: %s", request.uri)
        try:
            env = connector.prepare_mcp_env(request.credentials)
            result = await adapter.read_resource(request.uri, env=env)
            return JSONResponse(status_code=status.HTTP_200_OK, content=result)
        except Exception as e:
            logger.error("Resource read failed for %s: %s", request.uri, e)
            return JSONResponse(
                status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                content={"error": str(e)},
            )

    @app.post("/prompt")
    async def get_prompt(request: PromptRequest) -> JSONResponse:
        adapter = connector.mcp_adapter
        if adapter is None:
            return JSONResponse(
                status_code=status.HTTP_404_NOT_FOUND,
                content={"error": "MCP not enabled for this connector"},
            )
        logger.info("Prompt requested: %s", request.name)
        try:
            env = connector.prepare_mcp_env(request.credentials)
            result = await adapter.get_prompt(request.name, request.arguments, env=env)
            return JSONResponse(status_code=status.HTTP_200_OK, content=result)
        except Exception as e:
            logger.error("Prompt get failed for %s: %s", request.name, e)
            return JSONResponse(
                status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                content={"error": str(e)},
            )

    return app
