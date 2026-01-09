import asyncio
import json
import logging
import os
from typing import TYPE_CHECKING, Any

from fastapi import FastAPI, HTTPException, status
from fastapi.responses import JSONResponse

from .client import SdkClient
from .context import SyncContext
from .models import (
    ActionRequest,
    ActionResponse,
    CancelRequest,
    CancelResponse,
    SyncRequest,
    SyncResponse,
)

if TYPE_CHECKING:
    from .connector import Connector

logger = logging.getLogger(__name__)


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


async def _fetch_source_data(
    source_id: str,
) -> tuple[dict[str, Any], dict[str, Any], dict[str, Any] | None]:
    """
    Fetch source config, credentials, and state from database.
    """
    import asyncpg

    database_url = os.environ.get("DATABASE_URL")
    if not database_url:
        host = os.environ.get("DATABASE_HOST", "localhost")
        username = os.environ.get("DATABASE_USERNAME", "postgres")
        password = os.environ.get("DATABASE_PASSWORD", "postgres")
        dbname = os.environ.get("DATABASE_NAME", "omni")
        port = os.environ.get("DATABASE_PORT", "5432")
        database_url = f"postgresql://{username}:{password}@{host}:{port}/{dbname}"

    conn = await asyncpg.connect(database_url)
    try:
        source = await conn.fetchrow(
            "SELECT config, connector_state FROM sources WHERE id = $1",
            source_id,
        )
        if not source:
            raise HTTPException(
                status_code=status.HTTP_404_NOT_FOUND,
                detail=f"Source not found: {source_id}",
            )

        creds = await conn.fetchrow(
            "SELECT credentials FROM service_credentials WHERE source_id = $1",
            source_id,
        )

        source_config = json.loads(source["config"]) if source["config"] else {}
        state = (
            json.loads(source["connector_state"]) if source["connector_state"] else None
        )
        credentials = (
            json.loads(creds["credentials"]) if creds and creds["credentials"] else {}
        )

        return source_config, credentials, state
    finally:
        await conn.close()


def create_app(connector: "Connector") -> FastAPI:
    """Create FastAPI app for a connector."""

    app = FastAPI(
        title=f"Omni {connector.name} Connector",
        version=connector.version,
    )
    server = ConnectorServer(connector)

    @app.get("/health")
    async def health() -> dict[str, str]:
        return {"status": "healthy", "service": connector.name}

    @app.get("/manifest")
    async def manifest() -> dict[str, Any]:
        return connector.get_manifest().model_dump()

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
            source_config, credentials, state = await _fetch_source_data(source_id)
        except HTTPException as e:
            return JSONResponse(
                status_code=e.status_code,
                content=SyncResponse.error(e.detail).model_dump(),
            )
        except Exception as e:
            logger.error("Failed to fetch source data: %s", e)
            return JSONResponse(
                status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                content=SyncResponse.error(
                    f"Failed to fetch source data: {e}"
                ).model_dump(),
            )

        ctx = SyncContext(
            sdk_client=server.sdk_client,
            sync_run_id=sync_run_id,
            source_id=source_id,
            state=state,
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
    async def execute_action(request: ActionRequest) -> dict[str, Any]:
        logger.info("Action requested: %s", request.action)

        try:
            response = await connector.execute_action(
                request.action,
                request.params,
                request.credentials,
            )
            return response.model_dump()
        except Exception as e:
            logger.error("Action %s failed: %s", request.action, e)
            return ActionResponse.failure(str(e)).model_dump()

    return app
