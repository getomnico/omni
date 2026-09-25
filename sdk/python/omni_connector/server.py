import asyncio
import logging
import os
from collections.abc import AsyncIterator
from contextlib import asynccontextmanager
from typing import TYPE_CHECKING, Any

from fastapi import FastAPI, status
from fastapi.responses import JSONResponse
from starlette.responses import Response

from .client import SdkClient
from .config import SdkConfig
from .context import SyncContext
from .exceptions import SdkClientError
from .mcp_adapter import MCP_AUTH_REQUIRED_MESSAGE, MCP_AUTH_STATUS_FILE_ENV
from .models import (
    ActionRequest,
    ActionResponse,
    CancelRequest,
    CancelResponse,
    OAuthCredentialReadyRequest,
    OAuthCredentialValidationRequest,
    PromptRequest,
    ResourceRequest,
    SkillRequest,
    SkillResponse,
    SyncMode,
    SyncRequest,
    SyncResponse,
)

if TYPE_CHECKING:
    from .connector import Connector

logger = logging.getLogger(__name__)

REGISTRATION_INTERVAL_SECONDS = 30
SyncSlot = tuple[str, str]


def _sync_slot(source_id: str, sync_mode: SyncMode) -> SyncSlot:
    slot = "realtime" if sync_mode == SyncMode.REALTIME else "scheduled"
    return source_id, slot


class ConnectorServer:
    """HTTP server wrapper for a connector."""

    def __init__(self, connector: "Connector", config: SdkConfig):
        self.connector = connector
        self.config = config
        # Connector-manager permits one realtime and one scheduled sync per source.
        self.active_syncs: dict[SyncSlot, SyncContext] = {}
        self._sdk_client: SdkClient | None = None

    @property
    def sdk_client(self) -> SdkClient:
        if self._sdk_client is None:
            self._sdk_client = SdkClient(config=self.config)
        return self._sdk_client


def create_app(
    connector: "Connector", config: SdkConfig | None = None
) -> FastAPI:
    """Create FastAPI app for a connector."""

    def mcp_auth_required_response(
        credentials: dict[str, Any],
        message: str,
        *,
        source_id: str | None = None,
        source_type: str | None = None,
        auth: dict[str, Any] | None = None,
    ) -> JSONResponse | None:
        status_file = None
        if auth:
            env = auth.get("env")
            if isinstance(env, dict):
                status_file = env.get(MCP_AUTH_STATUS_FILE_ENV)
        marker_auth = False
        if isinstance(status_file, str) and status_file:
            try:
                with open(status_file, encoding="utf-8") as marker:
                    marker_auth = marker.read().strip() == "needs_user_auth"
                # The marker is a short-lived per-attempt temp file; consume
                # it after reading so failed authentications do not accumulate
                # in the connector container.
                os.unlink(status_file)
            except OSError:
                pass
        if message == MCP_AUTH_REQUIRED_MESSAGE:
            marker_auth = True
        if not connector.mcp_authentication_error(message) and not marker_auth:
            return None
        resolved_source_id = source_id or credentials.get("source_id")
        resolved_source_type = source_type or (
            connector.source_types[0] if connector.source_types else None
        )
        if not isinstance(resolved_source_id, str) or not resolved_source_id:
            return None
        if not isinstance(resolved_source_type, str) or not resolved_source_type:
            return None
        oauth_config = connector.oauth_config()
        provider = oauth_config.provider if oauth_config is not None else None
        return JSONResponse(
            status_code=status.HTTP_412_PRECONDITION_FAILED,
            content={
                "error": "needs_user_auth",
                "source_id": resolved_source_id,
                "source_type": resolved_source_type,
                "provider": provider,
                "oauth_start_url": f"/api/oauth/start?source_id={resolved_source_id}",
            },
        )

    config_from_env = config is None
    config = config or SdkConfig.from_env()
    connector_url = config.connector_url
    server = ConnectorServer(connector, config)

    @asynccontextmanager
    async def lifespan(app: FastAPI) -> AsyncIterator[None]:  # noqa: ARG001
        nonlocal connector_url
        if config_from_env:
            # Resolve environment-backed configuration when the ASGI app is
            # started, not only when it is imported. Test harnesses and
            # embedders may create the app before bringing up the manager.
            try:
                runtime_config = SdkConfig.from_env()
                server.config = runtime_config
                server._sdk_client = None
                connector_url = runtime_config.connector_url
            except ValueError:
                logger.warning("Using connector SDK configuration captured at app creation")

        async def registration_loop() -> None:
            nonlocal connector_url
            registration_succeeded = False
            while True:
                if config_from_env:
                    try:
                        runtime_config = SdkConfig.from_env()
                        if runtime_config != server.config:
                            server.config = runtime_config
                            server._sdk_client = None
                            connector_url = runtime_config.connector_url
                    except ValueError:
                        pass
                try:
                    manifest = await connector.get_manifest(connector_url=connector_url)
                    await server.sdk_client.register(manifest.model_dump())
                    logger.info("Registered with connector manager")
                    registration_succeeded = True
                except Exception as e:
                    logger.warning("Registration failed: %s", e)
                await asyncio.sleep(
                    REGISTRATION_INTERVAL_SECONDS if registration_succeeded else 1
                )

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

    @app.post("/oauth/validate")
    async def validate_oauth_credential(
        request: OAuthCredentialValidationRequest,
    ) -> JSONResponse:
        if request.source is None:
            return JSONResponse(
                status_code=status.HTTP_400_BAD_REQUEST,
                content={"error": "source is required for OAuth credential validation"},
            )
        try:
            binding = await connector.validate_oauth_credential(
                request.source, request.credentials, request.flow, request.metadata
            )
            if binding is not None:
                if not isinstance(binding, dict) or not all(
                    isinstance(key, str) and isinstance(value, str)
                    for key, value in binding.items()
                ):
                    raise ValueError(
                        "OAuth credential validation returned an invalid source binding"
                    )
            return JSONResponse(
                content={
                    "source_binding": (
                        dict(binding) if binding else None
                    )
                }
            )
        except Exception as exc:
            logger.warning("OAuth credential validation failed", exc_info=True)
            return JSONResponse(
                status_code=status.HTTP_400_BAD_REQUEST,
                content={"error": str(exc)},
            )

    @app.post("/oauth/credential-ready")
    async def oauth_credential_ready(
        request: OAuthCredentialReadyRequest,
    ) -> Response:
        """
        Generic notification when a new OAuth credential has been stored.
        The connector may use the credential to refresh its MCP catalog or
        perform other provider-specific setup.
        """
        # Include resolved credentials from the request so the connector hook
        # can use them without re-resolving.
        changed = await connector.oauth_credential_ready(request)
        if changed:
            m = await connector.get_manifest(connector_url=connector_url)
            try:
                await server.sdk_client.register(m.model_dump())
            except Exception as e:
                logger.warning(
                    "OAuth credential-ready manifest registration failed: %s: %r",
                    type(e).__name__,
                    e,
                )
            return JSONResponse(
                status_code=status.HTTP_200_OK,
                content=m.model_dump(),
            )
        return Response(status_code=status.HTTP_204_NO_CONTENT)

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

        try:
            sync_mode = SyncMode(request.sync_mode)
        except ValueError:
            logger.warning(
                "Unknown sync_mode %r; defaulting to Incremental batching",
                request.sync_mode,
            )
            sync_mode = SyncMode.INCREMENTAL

        sync_slot = _sync_slot(source_id, sync_mode)
        if sync_slot in server.active_syncs:
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
            checkpoint = request.checkpoint
            connector_state = sync_data.connector_state
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

        # Bootstrap MCP subprocess with credentials (populates tool cache for
        # manifest). Include the source id as an internal, non-secret hint so
        # stdio connectors can isolate CLI state per source.
        mcp_credentials = dict(credentials)
        mcp_credentials["_omni_source_id"] = source_id
        await connector.bootstrap_mcp(mcp_credentials)

        ctx = SyncContext(
            sdk_client=server.sdk_client,
            sync_run_id=sync_run_id,
            source_id=source_id,
            source_type=source_type,
            checkpoint=checkpoint,
            connector_state=connector_state,
            user_filter_mode=sync_data.user_filter_mode,
            user_whitelist=sync_data.user_whitelist,
            user_blacklist=sync_data.user_blacklist,
            sync_mode=sync_mode,
            is_resume=request.is_resume,
            documents_scanned=request.documents_scanned,
            documents_updated=request.documents_updated,
        )
        server.active_syncs[sync_slot] = ctx

        async def run_sync() -> None:
            try:
                await connector.sync(source_config, credentials, checkpoint, ctx)
            except Exception as e:
                logger.error("Sync %s failed: %s", sync_run_id, e)
                if not ctx.is_cancelled():
                    try:
                        await ctx.fail(str(e))
                    except Exception as fail_error:
                        logger.error("Failed to report sync failure: %s", fail_error)
            finally:
                if server.active_syncs.get(sync_slot) is ctx:
                    server.active_syncs.pop(sync_slot, None)

        asyncio.create_task(run_sync())

        return JSONResponse(
            status_code=status.HTTP_200_OK,
            content=SyncResponse.started().model_dump(),
        )

    @app.post("/cancel")
    async def cancel_sync(request: CancelRequest) -> JSONResponse:
        sync_run_id = request.sync_run_id
        logger.info("Cancel requested for sync %s", sync_run_id)

        matching_slot: SyncSlot | None = None
        matching_ctx = None
        for sync_slot, ctx in server.active_syncs.items():
            if ctx.sync_run_id == sync_run_id:
                matching_slot = sync_slot
                matching_ctx = ctx
                break

        if matching_slot is None or matching_ctx is None:
            return JSONResponse(
                status_code=status.HTTP_404_NOT_FOUND,
                content=CancelResponse(status="not_found").model_dump(),
            )

        matching_ctx._set_cancelled()
        server.active_syncs.pop(matching_slot, None)
        connector.cancel(sync_run_id)
        return JSONResponse(
            status_code=status.HTTP_200_OK,
            content=CancelResponse(status="cancelled").model_dump(),
        )

    @app.post("/action")
    async def execute_action(request: ActionRequest) -> Response:
        logger.info("Action requested: %s", request.action)

        # Native action names are reserved. MCP actions are validated here so
        # connector-specific policies run before a tool reaches the server;
        # native actions fall through to connector overrides.
        adapter = connector.mcp_adapter
        native_action_names = {action.name for action in connector.actions}
        if adapter is not None and request.action not in native_action_names:
            try:
                mcp_action_names = {
                    action.name for action in await adapter.get_action_definitions()
                }
            except Exception:
                logger.warning("Failed to identify MCP action", exc_info=True)
                return JSONResponse(
                    status_code=status.HTTP_503_SERVICE_UNAVAILABLE,
                    content={"error": "MCP catalog is unavailable"},
                )
            if request.action not in mcp_action_names:
                return JSONResponse(
                    status_code=status.HTTP_404_NOT_FOUND,
                    content={"error": "Action is not in the connector catalog"},
                )

            auth: dict[str, Any] = {}
            try:
                connector.validate_mcp_action(
                    request.action, dict(request.params), request.source
                )
                auth = connector._prepare_mcp_auth(request.credentials)
                arguments = connector.prepare_mcp_tool_arguments(
                    request.action, request.params
                )
                response = await adapter.execute_tool(
                    request.action, arguments, **auth
                )
                if response.status != "success" and response.error:
                    auth_response = mcp_auth_required_response(
                        request.credentials,
                        response.error,
                        source_id=(request.source.id if request.source else None),
                        source_type=(
                            request.source.source_type if request.source else None
                        ),
                        auth=auth,
                    )
                    if auth_response is not None:
                        return auth_response
                status_code = (
                    status.HTTP_200_OK
                    if response.status == "success"
                    else status.HTTP_400_BAD_REQUEST
                )
                return JSONResponse(
                    content=response.model_dump(), status_code=status_code
                )
            except ValueError as e:
                logger.info("MCP action rejected by connector validation: %s", e)
                return JSONResponse(
                    content=ActionResponse.failure(str(e)).model_dump(),
                    status_code=status.HTTP_400_BAD_REQUEST,
                )
            except Exception as e:
                auth_response = mcp_auth_required_response(
                    request.credentials,
                    str(e),
                    source_id=(request.source.id if request.source else None),
                    source_type=(
                        request.source.source_type if request.source else None
                    ),
                    auth=auth,
                )
                if auth_response is not None:
                    return auth_response
                logger.warning("MCP action execution failed", exc_info=True)
                return JSONResponse(
                    status_code=status.HTTP_502_BAD_GATEWAY,
                    content={"error": "MCP action execution failed"},
                )

        return await connector.execute_action(
            request.action,
            dict(request.params),
            request.credentials,
            source=request.source,
            actor_email=request.actor_email,
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
        auth: dict[str, Any] = {}
        try:
            auth = connector._prepare_mcp_auth(request.credentials)
            result = await adapter.read_resource(request.uri, **auth)
            return JSONResponse(status_code=status.HTTP_200_OK, content=result)
        except Exception as e:
            auth_response = mcp_auth_required_response(
                request.credentials, str(e), auth=auth
            )
            if auth_response is not None:
                return auth_response
            logger.error("Resource read failed for %s: %s", request.uri, e)
            return JSONResponse(
                status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                content={"error": "MCP resource read failed"},
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
        auth: dict[str, Any] = {}
        try:
            auth = connector._prepare_mcp_auth(request.credentials)
            result = await adapter.get_prompt(request.name, request.arguments, **auth)
            return JSONResponse(status_code=status.HTTP_200_OK, content=result)
        except Exception as e:
            auth_response = mcp_auth_required_response(
                request.credentials, str(e), auth=auth
            )
            if auth_response is not None:
                return auth_response
            logger.error("Prompt get failed for %s: %s", request.name, e)
            return JSONResponse(
                status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                content={"error": "MCP prompt request failed"},
            )

    @app.post("/skill")
    async def get_skill(request: SkillRequest) -> JSONResponse:
        logger.info("Skill requested: %s", request.skill_id)
        for skill in connector.skills:
            if skill.id != request.skill_id:
                continue
            if skill.content is not None:
                return JSONResponse(
                    status_code=status.HTTP_200_OK,
                    content=SkillResponse(
                        skill_id=skill.id,
                        title=skill.title,
                        content=skill.content,
                    ).model_dump(),
                )
            if skill.mcp_prompt:
                return await _mcp_skill_response(
                    skill.id, skill.title, skill.mcp_prompt, request
                )

        prompt_name = (
            request.skill_id.removeprefix("mcp:")
            if request.skill_id.startswith("mcp:")
            else None
        )
        if prompt_name:
            return await _mcp_skill_response(
                request.skill_id, "MCP Prompt", prompt_name, request
            )

        return JSONResponse(
            status_code=status.HTTP_404_NOT_FOUND,
            content={"error": f"Unknown skill: {request.skill_id}"},
        )

    async def _mcp_skill_response(
        skill_id: str,
        title: str,
        prompt_name: str,
        request: SkillRequest,
    ) -> JSONResponse:
        adapter = connector.mcp_adapter
        if adapter is None:
            return JSONResponse(
                status_code=status.HTTP_404_NOT_FOUND,
                content={"error": "MCP not enabled for this connector"},
            )
        auth: dict[str, Any] = {}
        try:
            auth = connector._prepare_mcp_auth(request.credentials)
            result = await adapter.get_prompt(prompt_name, request.arguments, **auth)
            content = _mcp_prompt_to_text(result)
            return JSONResponse(
                status_code=status.HTTP_200_OK,
                content=SkillResponse(
                    skill_id=skill_id,
                    title=title,
                    content=content,
                ).model_dump(),
            )
        except Exception as e:
            auth_response = mcp_auth_required_response(
                request.credentials, str(e), auth=auth
            )
            if auth_response is not None:
                return auth_response
            logger.error("Skill get failed for %s: %s", skill_id, e)
            return JSONResponse(
                status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                content={"error": "MCP skill request failed"},
            )

    def _mcp_prompt_to_text(value: dict[str, Any]) -> str:
        parts: list[str] = []
        description = value.get("description")
        if isinstance(description, str) and description:
            parts.append(description)
        messages = value.get("messages")
        if isinstance(messages, list):
            for message in messages:
                if not isinstance(message, dict):
                    continue
                content = message.get("content")
                if not isinstance(content, dict):
                    continue
                text = content.get("text")
                if isinstance(text, str):
                    parts.append(text)
        return "\n\n".join(parts)

    return app
