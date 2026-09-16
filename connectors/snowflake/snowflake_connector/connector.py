from __future__ import annotations

import logging
from collections.abc import Callable
from typing import Any

from omni_connector import (
    ActionDefinition,
    Connector,
    ConnectorManifest,
    ConnectorManifestSource,
    HttpMcpServer,
    OAuthCredentialFlow,
    OAuthCredentialReadyRequest,
    OAuthManifestConfig,
    OAuthScopeSet,
    Source,
)
from omni_connector.mcp_adapter import McpAdapter

from .client import (
    SnowflakeClient,
    default_oauth_session_factory,
    default_session_factory,
    validate_mcp_endpoint,
)
from .config import SnowflakeConfig, SnowflakeCredentials
from .sync import SnowflakeSync

logger = logging.getLogger(__name__)


class SnowflakeConnector(Connector):
    def __init__(
        self,
        session_factory: Callable[..., Any] | None = None,
        oauth_session_factory: Callable[..., Any] | None = None,
    ) -> None:
        super().__init__()
        self._session_factory = session_factory or default_session_factory
        self._oauth_session_factory = oauth_session_factory or default_oauth_session_factory
        self._source_endpoints: dict[str, str] = {}
        self._source_catalogs: dict[str, tuple[list[Any], list[Any], list[Any]]] = {}
        self._cached_manifest_action_names: set[str] = set()
        self._source_contexts: list[ConnectorManifestSource] = []
        self._sync = SnowflakeSync(self._session_factory)

    @property
    def name(self) -> str:
        return "snowflake"

    @property
    def display_name(self) -> str:
        return "Snowflake"

    @property
    def version(self) -> str:
        return "1.0.0"

    @property
    def source_types(self) -> list[str]:
        return ["snowflake"]

    @property
    def description(self) -> str:
        return "Snowflake metadata search and governed managed MCP tools"

    @property
    def sync_modes(self) -> list[str]:
        return ["full", "incremental"]

    @property
    def actions(self) -> list[ActionDefinition]:
        return []

    def oauth_config(self) -> OAuthManifestConfig:
        return OAuthManifestConfig(
            provider="snowflake",
            auth_endpoint="https://login.snowflake.com/oauth/authorize",
            token_endpoint="https://login.snowflake.com/oauth/token-request",
            identity_scopes=["session:role-any"],
            scopes={
                "snowflake": OAuthScopeSet(read=["session:role-any"], write=["session:role-any"])
            },
            pkce_required=True,
            token_endpoint_auth_method="client_secret_basic",
            issuer_source_config_key="oauth_issuer_url",
            client_config_provider_template="snowflake:{source_id}",
            validate_endpoint_urls=True,
            supports_org_oauth=False,
        )

    @property
    def mcp_server(self) -> HttpMcpServer:
        # The source-aware adapter below is authoritative. This placeholder
        # only keeps the generic SDK manifest contract MCP-enabled for legacy
        # GET /manifest callers; it is never used for a selected source.
        return HttpMcpServer("https://invalid.snowflake.invalid/mcp")

    def mcp_server_for_source(self, source: Source | None) -> HttpMcpServer | None:
        if source is None:
            return self.mcp_server
        config = SnowflakeConfig.model_validate(source.config)
        if not config.mcp_enabled or config.mcp_endpoint_url is None:
            return None
        return HttpMcpServer(validate_mcp_endpoint(config.mcp_endpoint_url, config.account_url))

    def mcp_adapter_for_source(self, source: Source | None) -> McpAdapter | None:
        server = self.mcp_server_for_source(source)
        if server is None:
            return None
        return McpAdapter(server)

    def mcp_adapter_for_credentials(
        self, credentials: dict[str, Any], source: Source | None = None
    ) -> McpAdapter | None:
        source_id = credentials.get("source_id")
        endpoint = self._source_endpoints.get(source_id) if isinstance(source_id, str) else None
        if endpoint is None:
            return super().mcp_adapter_for_credentials(credentials, source)
        return McpAdapter(HttpMcpServer(endpoint))

    async def mcp_action_names_for_source(self, source: Source | None) -> set[str]:
        if source is not None:
            catalog = self._source_catalogs.get(source.id)
            if catalog is not None:
                return {action.name for action in catalog[0]}
        return set(self._cached_manifest_action_names)

    def prepare_mcp_headers(self, credentials: dict[str, Any]) -> dict[str, str]:
        token = _oauth_token(credentials)
        if token is None:
            raise ValueError("Snowflake MCP requires the invoking user's OAuth access token")
        return {"Authorization": f"Bearer {token}"}

    async def build_manifest_for_sources(
        self,
        sources: list[ConnectorManifestSource],
        current_manifest: ConnectorManifest | None,
        connector_url: str,
    ) -> ConnectorManifest:
        self._source_contexts = list(sources)
        self._source_endpoints = {}
        active_source_ids: set[str] = set()
        self._cached_manifest_action_names = {
            action.name
            for action in (current_manifest.actions if current_manifest is not None else [])
            if action.origin == "mcp"
        }
        catalogs: list[tuple[tuple[list[Any], list[Any], list[Any]], bool]] = []
        for source in sources:
            config = SnowflakeConfig.model_validate(source.config)
            if not config.mcp_enabled or config.mcp_endpoint_url is None:
                continue
            endpoint = validate_mcp_endpoint(config.mcp_endpoint_url, config.account_url)
            self._source_endpoints[source.id] = endpoint
            active_source_ids.add(source.id)
            catalog = self._source_catalogs.get(source.id)
            if catalog is not None:
                catalogs.append((catalog, config.write_tools_enabled))

        self._source_catalogs = {
            source_id: catalog
            for source_id, catalog in self._source_catalogs.items()
            if source_id in active_source_ids
        }
        manifest = ConnectorManifest(
            name=self.name,
            display_name=self.display_name,
            version=self.version,
            sync_modes=self.sync_modes,
            connector_id=self.name,
            connector_url=connector_url,
            source_types=self.source_types,
            description=self.description,
            actions=[],
            mcp_enabled=bool(active_source_ids),
            mcp_catalog_loaded=False,
            oauth=self.oauth_config(),
        )
        if current_manifest is not None and active_source_ids and not catalogs:
            # The manager's compatible cache is authoritative until a user
            # credential permits a fresh source-specific tools/list request.
            return current_manifest.model_copy(update={"connector_url": connector_url})

        actions: dict[str, Any] = {}
        conflicts: set[str] = set()
        resources: dict[str, Any] = {}
        prompts: dict[str, Any] = {}
        for (source_actions, source_resources, source_prompts), write_tools_enabled in catalogs:
            for action in source_actions:
                if action.mode == "write" and not write_tools_enabled:
                    continue
                existing = actions.get(action.name)
                if existing is None and action.name not in conflicts:
                    actions[action.name] = action
                elif existing is not None and _same_action(existing, action):
                    continue
                else:
                    actions.pop(action.name, None)
                    conflicts.add(action.name)
                    logger.warning("Omitting conflicting Snowflake MCP tool %s", action.name)
            for resource in source_resources:
                resources.setdefault(resource.uri_template, resource)
            for prompt in source_prompts:
                prompts.setdefault(prompt.name, prompt)
        manifest.actions = [
            action for name, action in sorted(actions.items()) if name not in conflicts
        ]
        manifest.resources = list(resources.values())
        manifest.prompts = list(prompts.values())
        manifest.mcp_catalog_loaded = bool(catalogs)
        return manifest

    async def get_manifest(self, connector_url: str) -> ConnectorManifest:
        if self._source_contexts:
            return await self.build_manifest_for_sources(self._source_contexts, None, connector_url)
        return await super().get_manifest(connector_url)

    async def oauth_credential_ready(self, request: OAuthCredentialReadyRequest) -> bool:
        endpoint = self._source_endpoints.get(request.source_id)
        if endpoint is None:
            logger.warning(
                "Ignoring OAuth catalog refresh for unknown Snowflake source %s", request.source_id
            )
            return False
        adapter = McpAdapter(HttpMcpServer(endpoint))
        await adapter.discover(headers=self.prepare_mcp_headers(request.credentials))
        self._source_catalogs[request.source_id] = (
            await adapter.get_action_definitions(),
            await adapter.get_resource_definitions(),
            await adapter.get_prompt_definitions(),
        )
        return True

    async def validate_oauth_credential(
        self,
        source: Source,
        credentials: dict[str, Any],
        flow: OAuthCredentialFlow,
        metadata: dict[str, Any] | None = None,
    ) -> dict[str, str]:
        token = _oauth_token(credentials)
        if flow != OAuthCredentialFlow.ORG_SOURCE and token is None:
            raise ValueError("Snowflake OAuth credential has no access token")
        config = SnowflakeConfig.model_validate(source.config)
        source_binding = source.config.get("source_binding")
        expected_account = (
            source_binding.get("account")
            if isinstance(source_binding, dict) and isinstance(source_binding.get("account"), str)
            else None
        )
        if flow == OAuthCredentialFlow.ORG_SOURCE:
            metadata_credentials = SnowflakeCredentials.model_validate(
                _credential_payload(credentials)
            )
            session = self._session_factory(config, metadata_credentials)
        else:
            if config.mcp_endpoint_url is None:
                raise ValueError("Snowflake MCP endpoint is required for OAuth validation")
            validate_mcp_endpoint(config.mcp_endpoint_url, config.account_url)
            if token is None:
                raise ValueError("Snowflake OAuth access token is required")
            session = self._oauth_session_factory(config, token)
        client = SnowflakeClient(session)
        try:
            account, current_user, _region = client.verify_identity(
                expected_account=expected_account
            )
            provider_email = client.user_email(current_user)
        finally:
            session.close()
        expected_email = (metadata or {}).get("omni_user_email")
        if flow in (
            OAuthCredentialFlow.USER_READ,
            OAuthCredentialFlow.USER_WRITE,
        ):
            if not isinstance(expected_email, str) or not expected_email:
                raise ValueError(
                    "trusted Omni user email is required for Snowflake OAuth validation"
                )
            if provider_email is None or provider_email.casefold() != expected_email.casefold():
                raise ValueError(
                    "Snowflake OAuth principal does not match the authenticated Omni user"
                )
        binding = {"account": account, "user": current_user}
        if provider_email is not None:
            binding["email"] = provider_email
        return binding

    async def sync(
        self,
        source_config: dict[str, Any],
        credentials: dict[str, Any],
        checkpoint: dict[str, Any] | None,
        ctx: Any,
    ) -> None:
        await self._sync.run(source_config, credentials, checkpoint, ctx)


def _credential_payload(credentials: dict[str, Any]) -> dict[str, Any]:
    nested = credentials.get("credentials")
    return nested if isinstance(nested, dict) else credentials


def _oauth_token(credentials: dict[str, Any]) -> str | None:
    nested = credentials.get("credentials")
    payload: dict[str, Any] = nested if isinstance(nested, dict) else credentials
    token = payload.get("access_token")
    return token if isinstance(token, str) and token else None


def _same_action(left: Any, right: Any) -> bool:
    return bool(
        left.description == right.description
        and left.input_schema == right.input_schema
        and left.mode == right.mode
    )
