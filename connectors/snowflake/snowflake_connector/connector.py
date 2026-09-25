from __future__ import annotations

import asyncio
import json
import logging
from collections.abc import Callable
from typing import Any

from omni_connector import (
    ActionDefinition,
    Connector,
    ConnectorManifest,
    ConnectorSkillDefinition,
    ConnectorSourceCapabilities,
    HttpMcpServer,
    ManifestSourceContext,
    McpPromptDefinition,
    McpResourceDefinition,
    OAuthCredentialFlow,
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
        self._source_cache_keys: dict[str, str] = {}
        self._source_skills: dict[str, list[ConnectorSkillDefinition]] = {}
        self._source_catalogs: dict[
            str,
            tuple[
                list[ActionDefinition],
                list[McpResourceDefinition],
                list[McpPromptDefinition],
            ],
        ] = {}
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
            identity_scopes=["session:role:all", "refresh_token"],
            scopes={
                "snowflake": OAuthScopeSet(
                    read=["session:role:all", "refresh_token"],
                    write=["session:role:all", "refresh_token"],
                )
            },
            pkce_required=True,
            token_endpoint_auth_method="client_secret_basic",
            issuer_source_config_key="oauth_issuer_url",
            client_config_provider_template="snowflake:{source_id}",
            validate_endpoint_urls=True,
            supports_org_oauth=False,
        )

    @property
    def mcp_server(self) -> None:
        return None

    def mcp_server_for_source(self, source: Source | None) -> HttpMcpServer | None:
        if source is None:
            return None
        config = SnowflakeConfig.model_validate(source.config)
        if not config.mcp_enabled or config.mcp_endpoint_url is None:
            return None
        return HttpMcpServer(validate_mcp_endpoint(config.mcp_endpoint_url, config.account_url))

    def mcp_adapter_for_source(self, source: Source | None) -> McpAdapter | None:
        server = self.mcp_server_for_source(source)
        return McpAdapter(server) if server is not None else None

    def mcp_adapter_for_credentials(
        self, credentials: dict[str, Any], source: Source | None = None
    ) -> McpAdapter | None:
        return self.mcp_adapter_for_source(source)

    def mcp_skill_for_source(
        self, skill_id: str, source: Source | None
    ) -> ConnectorSkillDefinition | None:
        if source is None:
            return None
        return next(
            (skill for skill in self._source_skills.get(source.id, []) if skill.id == skill_id),
            None,
        )

    async def mcp_action_names_for_source(self, source: Source | None) -> set[str]:
        if source is None:
            return set()
        catalog = self._source_catalogs.get(source.id)
        return {action.name for action in catalog[0]} if catalog is not None else set()

    def _prepare_mcp_auth(self, credentials: dict[str, Any]) -> dict[str, Any]:
        return {"headers": self.prepare_mcp_headers(credentials)}

    def prepare_mcp_headers(self, credentials: dict[str, Any]) -> dict[str, str]:
        token = _oauth_token(credentials)
        if token is None:
            raise ValueError("Snowflake MCP requires the invoking user's OAuth access token")
        return {"Authorization": f"Bearer {token}"}

    def mcp_action_allowed(self, action: str, source: Source | None) -> bool:
        if source is None:
            return False
        config = SnowflakeConfig.model_validate(source.config)
        catalog = self._source_catalogs.get(source.id)
        if catalog is None:
            return False
        definition = next((item for item in catalog[0] if item.name == action), None)
        if definition is None:
            return False
        return definition.mode == "read" or (
            config.write_tools_enabled and not config.read_only
        )

    async def get_manifest(
        self,
        connector_url: str,
        *,
        source_context: ManifestSourceContext | None = None,
        credentials: dict[str, Any] | None = None,
        force_refresh: bool = False,
    ) -> ConnectorManifest:
        manifest = ConnectorManifest(
            name=self.name,
            display_name=self.display_name,
            version=self.version,
            sync_modes=self.sync_modes,
            connector_id=self.name,
            connector_url=connector_url,
            source_types=self.source_types,
            description=self.description,
            mcp_enabled=True,
            oauth=self.oauth_config(),
        )
        if source_context is None:
            return manifest

        config = SnowflakeConfig.model_validate(source_context.config)
        if not config.mcp_enabled or config.mcp_endpoint_url is None:
            return manifest.model_copy(update={"mcp_enabled": False})

        cache_key = json.dumps(
            {
                "id": source_context.id,
                "source_type": source_context.source_type,
                "config": {
                    key: source_context.config.get(key)
                    for key in (
                        "account_url",
                        "mcp_endpoint_url",
                        "mcp_enabled",
                        "read_only",
                        "write_tools_enabled",
                    )
                },
            },
            sort_keys=True,
            separators=(",", ":"),
        )
        if self._source_cache_keys.get(source_context.id) != cache_key:
            self._source_cache_keys[source_context.id] = cache_key
            self._source_catalogs.pop(source_context.id, None)
            self._source_skills.pop(source_context.id, None)
            self._source_endpoints.pop(source_context.id, None)

        if credentials is None:
            return manifest
        cached_catalog = self._source_catalogs.get(source_context.id)
        cached_skills = self._source_skills.get(source_context.id)
        if not force_refresh and cached_catalog is not None and cached_skills is not None:
            return manifest.model_copy(
                update={
                    "source_capabilities": [
                        ConnectorSourceCapabilities(
                            source_id=source_context.id,
                            actions=cached_catalog[0],
                            resources=cached_catalog[1],
                            prompts=cached_catalog[2],
                            skills=cached_skills,
                        )
                    ],
                    "mcp_catalog_loaded": True,
                }
            )

        endpoint = validate_mcp_endpoint(config.mcp_endpoint_url, config.account_url)
        adapter = McpAdapter(HttpMcpServer(endpoint))
        await adapter.discover(headers=self.prepare_mcp_headers(credentials))
        actions = await adapter.get_action_definitions()
        for action in actions:
            action.origin = "mcp"
            if not action.source_types:
                action.source_types = list(self.source_types)
        if not config.write_tools_enabled or config.read_only:
            actions = [action for action in actions if action.mode == "read"]
        resources = await adapter.get_resource_definitions()
        prompts = await adapter.get_prompt_definitions()
        skills = []
        for prompt in prompts:
            skill = self._mcp_prompt_skill(prompt.name, prompt.description)
            skill.id = f"mcp:{source_context.id}:{prompt.name}"
            skill.source_types = self.source_types
            skills.append(skill)
        self._source_endpoints[source_context.id] = endpoint
        self._source_catalogs[source_context.id] = (actions, resources, prompts)
        self._source_skills[source_context.id] = skills
        return manifest.model_copy(
            update={
                "source_capabilities": [
                    ConnectorSourceCapabilities(
                        source_id=source_context.id,
                        actions=actions,
                        resources=resources,
                        prompts=prompts,
                        skills=skills,
                    )
                ],
                "mcp_catalog_loaded": True,
            }
        )

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
        expected_account = config.source_binding.account if config.source_binding else None
        if flow == OAuthCredentialFlow.ORG_SOURCE:
            metadata_credentials = SnowflakeCredentials.model_validate(
                _credential_payload(credentials)
            )
            session = await asyncio.to_thread(self._session_factory, config, metadata_credentials)
        else:
            if config.mcp_endpoint_url is None:
                raise ValueError("Snowflake MCP endpoint is required for OAuth validation")
            validate_mcp_endpoint(config.mcp_endpoint_url, config.account_url)
            if token is None:
                raise ValueError("Snowflake OAuth access token is required")
            session = await asyncio.to_thread(self._oauth_session_factory, config, token)
        client = SnowflakeClient(session)
        try:
            account, current_user, _region = client.verify_identity(
                expected_account=expected_account
            )
            provider_email = client.user_email(current_user)
        finally:
            await asyncio.to_thread(session.close)
        expected_email = (metadata or {}).get("omni_user_email")
        if flow in (
            OAuthCredentialFlow.USER_READ,
            OAuthCredentialFlow.USER_WRITE,
        ):
            if not isinstance(expected_email, str) or not expected_email:
                raise ValueError(
                    "trusted Omni user email is required for Snowflake OAuth validation"
                )
            if (
                provider_email is None
                or provider_email.strip().casefold() != expected_email.strip().casefold()
            ):
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
