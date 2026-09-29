"""ConnectorToolHandler: discovers and dispatches connector actions."""

from __future__ import annotations

import json
import logging
import re
from collections.abc import Mapping
from dataclasses import dataclass
from typing import Literal, TypedDict

import httpx
from anthropic.types import ToolParam

from db.documents import DocumentsRepository
from db.models import Source, filter_sources_for_user, parse_allowed_action_origins
from tools.omni_tool_result import OAuthRequiredPayload, encode_oauth_required
from tools.registry import ToolContext, ToolResult
from tools.sandbox import (
    is_textual_content_type,
    text_result_or_sandbox,
    write_binary_to_sandbox,
)

logger = logging.getLogger(__name__)

_TOOL_NAME_SAFE_RE = re.compile(r"[^a-zA-Z0-9_]")

SourceMode = Literal["read", "write"]
# Maps source_id -> list of modes allowed for that source.
SourceFilter = dict[str, list[SourceMode]]
ConnectorCatalog = list[dict[str, object]]
EffectiveActionsBySource = dict[str, list[Mapping[str, object]]]


def connector_catalog_from_payload(payload: object) -> ConnectorCatalog:
    if not isinstance(payload, list):
        raise TypeError("connector-manager /connectors response must be a list")

    catalog: ConnectorCatalog = []
    for item in payload:
        if not isinstance(item, Mapping) or any(not isinstance(key, str) for key in item):
            raise TypeError("connector-manager /connectors response contains a non-object item")
        catalog.append(dict(item))
    return catalog


def effective_actions_by_source_from_payload(payload: object) -> EffectiveActionsBySource:
    if not isinstance(payload, Mapping) or not isinstance(payload.get("actions"), list):
        raise TypeError("connector-manager /actions response must contain an actions list")

    actions_by_source: EffectiveActionsBySource = {}
    for action in payload["actions"]:
        if not isinstance(action, Mapping):
            raise TypeError("connector-manager /actions contains a non-object action")
        source_id = action.get("source_id")
        if not isinstance(source_id, str) or not source_id:
            raise TypeError("connector-manager action source_id is invalid")
        actions_by_source.setdefault(source_id, []).append(action)
    return actions_by_source


def action_is_available_for_source(source: Source, action_origin: str) -> bool:
    allowed_origins = parse_allowed_action_origins(source.config)
    return allowed_origins is None or action_origin in allowed_origins


def sources_from_sync_overview_response(payload: object) -> list[Source]:
    if not isinstance(payload, list):
        raise TypeError("connector-manager /sources response must be a list")

    sources: list[Source] = []
    for item in payload:
        if not isinstance(item, Mapping):
            raise TypeError("connector-manager /sources response contains a non-object item")
        source_payload = item["source"]
        if not isinstance(source_payload, Mapping):
            raise TypeError("connector-manager source overview missing source object")
        sources.append(Source.from_row(source_payload))
    return sources


async def fetch_active_sources_from_connector_manager(
    connector_manager_url: str,
    timeout: float = 10.0,
) -> list[Source]:
    """Fetch active source rows, including remote MCP rows.

    Connector-manager's legacy /sources endpoint is sync-overview oriented and
    excludes remote MCP sources because they never sync. Tool prefetch paths must
    use /sources/active so action/resource manifests can join active rows by
    (integration_type, source_type) without re-enabling sync.
    """
    async with httpx.AsyncClient(timeout=timeout) as client:
        resp = await client.get(f"{connector_manager_url.rstrip('/')}/sources/active")
        resp.raise_for_status()
        return sources_from_sync_overview_response(resp.json())


class ToolsetSummary(TypedDict):
    source_id: str
    source_type: str
    source_name: str
    tool_count: int
    sample_tool_names: list[str]


@dataclass
class SearchOperator:
    """A search operator declared by a connector."""

    operator: str
    attribute_key: str
    value_type: Literal["text", "person", "datetime"]
    source_type: str
    display_name: str


@dataclass
class ConnectorAction:
    """Internal mapping from LLM tool name to connector action details."""

    source_id: str
    source_type: str
    source_name: str
    action_name: str
    description: str
    input_schema: dict
    mode: SourceMode
    required_scopes: list[str] | None = None
    admin_only: bool = False
    hidden: bool = False
    integration_type: str = "connector"
    origin: Literal["native", "mcp"] = "native"
    # True when the connector declares a per-user OAuth flow (manifest.oauth).
    # Org-only connectors (e.g. Darwinbox) run actions against the org
    # credential and must never surface an OAuth prompt.
    supports_user_oauth: bool = False
    # The connector enforces that this action only affects records within
    # the caller's own authority (e.g. Darwinbox self-service writes), so it
    # may run on the org credential for regular users. Without it, write
    # actions on the org-credential path are admin-only.
    actor_scoped: bool = False


class ConnectorActionUnavailable(RuntimeError):
    """An action cannot run because Connector Manager reports missing org credentials."""


class ConnectorToolHandler:
    """Fetches connector actions and dispatches tool calls to connector-manager."""

    def __init__(
        self,
        connector_manager_url: str,
        user_id: str,
        prefetched_sources: list[Source] | None = None,
        source_filter: SourceFilter | None = None,
        action_whitelist: list[str] | None = None,
        documents_repo: DocumentsRepository | None = None,
        sandbox_url: str | None = None,
        is_admin: bool = False,
    ) -> None:
        self._connector_manager_url = connector_manager_url.rstrip("/")
        self._sandbox_url = sandbox_url.rstrip("/") if sandbox_url else None
        self._user_id = user_id
        self._prefetched_sources = prefetched_sources
        self._connector_catalog: ConnectorCatalog | None = None
        self._source_filter = source_filter
        self._action_whitelist = action_whitelist  # ["gmail__send_email"]
        self._documents_repo = documents_repo
        self._is_admin = is_admin
        self._actions: dict[str, ConnectorAction] = {}
        self._tools: list[ToolParam] = []
        self._search_operators: list[SearchOperator] = []
        self._initialized = False

    @property
    def connector_catalog(self) -> ConnectorCatalog | None:
        return self._connector_catalog

    async def _ensure_initialized(self) -> None:
        """Lazily fetch the current connector catalog and actions once per handler."""
        if self._initialized:
            return

        actions = await self._fetch_actions()
        self._build_tools(actions)
        self._initialized = True

    async def _fetch_actions(self) -> list[ConnectorAction]:
        """Fetch available actions from connector-manager.

        The connector-manager exposes GET /connectors which returns connector info
        including manifests with action definitions. We also need active sources
        to map source_id.
        """
        try:
            async with httpx.AsyncClient(timeout=10.0) as client:
                # Fetch connector info (includes manifests)
                connectors_resp = await client.get(f"{self._connector_manager_url}/connectors")
                connectors_resp.raise_for_status()
                connectors = connector_catalog_from_payload(connectors_resp.json())
                self._connector_catalog = connectors

                # Use pre-fetched sources if available, otherwise fetch from connector-manager
                if self._prefetched_sources is not None:
                    sources = self._prefetched_sources
                else:
                    sources = await fetch_active_sources_from_connector_manager(
                        self._connector_manager_url
                    )
                sources = filter_sources_for_user(sources, self._user_id)
                effective_actions_resp = await client.get(f"{self._connector_manager_url}/actions")
                effective_actions_resp.raise_for_status()
                effective_actions_by_source = effective_actions_by_source_from_payload(
                    effective_actions_resp.json()
                )

        except Exception as e:
            logger.error(f"Failed to fetch connector info: {e}")
            return []

        # Build a mapping from (integration_type, source_type) to active sources.
        source_by_identity: dict[tuple[str, str], list[Source]] = {}
        for source in sources:
            if source.is_active and not source.is_deleted:
                source_by_identity.setdefault(
                    (source.integration_type, source.source_type), []
                ).append(source)

        # Extract search operators from connector manifests
        search_operators: list[SearchOperator] = []
        for connector in connectors:
            source_type = connector.get("source_type", "")
            manifest = connector.get("manifest")
            integration_type = (
                manifest.get("integration_type", "connector") if manifest else "connector"
            )
            if not manifest or connector.get("healthy") is False:
                continue

            if (integration_type, source_type) not in source_by_identity:
                continue

            display_name = manifest.get("display_name", source_type)
            for op in manifest.get("search_operators", []):
                operator = op.get("operator")
                attribute_key = op.get("attribute_key")
                if not operator or not attribute_key:
                    continue
                search_operators.append(
                    SearchOperator(
                        operator=operator,
                        attribute_key=attribute_key,
                        value_type=op.get("value_type", "text"),
                        source_type=source_type,
                        display_name=display_name,
                    )
                )

        self._search_operators = search_operators

        # Build action list from connector manifests
        actions: list[ConnectorAction] = []
        for connector in connectors:
            source_type = connector.get("source_type", "")
            manifest = connector.get("manifest")
            integration_type = (
                manifest.get("integration_type", "connector") if manifest else "connector"
            )
            if not manifest or connector.get("healthy") is False:
                continue

            for source in source_by_identity.get((integration_type, source_type), []):
                source_actions = effective_actions_by_source.get(source.id, [])
                for action_def in source_actions:
                    if not isinstance(action_def, Mapping):
                        raise TypeError("connector-manager /actions contains a non-object action")
                    if (
                        action_def.get("source_id") != source.id
                        or action_def.get("source_type") != source_type
                    ):
                        raise ValueError(
                            "connector-manager returned an action for a different source"
                        )
                    action_origin = action_def.get("origin")
                    if action_origin not in {"native", "mcp"}:
                        raise TypeError("connector-manager action origin is invalid")
                    action_name = action_def.get("name")
                    description = action_def.get("description")
                    input_schema = action_def.get("input_schema")
                    mode = action_def.get("mode")
                    required_scopes = action_def.get("required_scopes")
                    admin_only = action_def.get("admin_only")
                    hidden = action_def.get("hidden")
                    actor_scoped = action_def.get("actor_scoped")
                    supports_user_oauth = action_def.get("supports_user_oauth")
                    if not isinstance(action_name, str) or not action_name:
                        raise TypeError("connector-manager action name is invalid")
                    if not isinstance(description, str) or not isinstance(input_schema, dict):
                        raise TypeError("connector-manager action metadata is malformed")
                    if mode not in {"read", "write"}:
                        raise TypeError("connector-manager action mode is invalid")
                    if required_scopes is not None and (
                        not isinstance(required_scopes, list)
                        or not all(isinstance(scope, str) for scope in required_scopes)
                    ):
                        raise TypeError("connector-manager action scopes are malformed")
                    if not all(
                        isinstance(value, bool)
                        for value in (admin_only, hidden, actor_scoped, supports_user_oauth)
                    ):
                        raise TypeError("connector-manager action policy is malformed")
                    actions.append(
                        ConnectorAction(
                            source_id=source.id,
                            source_type=source_type,
                            source_name=source.name or source_type,
                            action_name=action_name,
                            description=description,
                            input_schema=input_schema,
                            mode=mode,
                            required_scopes=required_scopes,
                            admin_only=admin_only,
                            hidden=hidden,
                            actor_scoped=actor_scoped,
                            integration_type=integration_type,
                            origin=action_origin,
                            supports_user_oauth=supports_user_oauth,
                        )
                    )

        logger.info(f"Discovered {len(actions)} connector actions for user {self._user_id}")
        return actions

    def _build_tools(self, actions: list[ConnectorAction]) -> None:
        """Convert connector actions to LLM tool format."""
        self._actions.clear()
        self._tools.clear()

        base_name_counts: dict[str, int] = {}
        for action in actions:
            # Hidden actions (e.g. internal setup actions) never appear in
            # chat/agent tool lists, admins included.
            if action.hidden:
                continue

            # Hide admin-only actions (e.g. admin-directory ops) from non-admins.
            if action.admin_only and not self._is_admin:
                continue

            # Writes that would execute with the org-level credential are
            # admin-role-only unless the connector declares the write is
            # server-side actor-scoped (e.g. Darwinbox self-service).
            if (
                action.mode == "write"
                and not action.supports_user_oauth
                and not action.actor_scoped
                and not self._is_admin
            ):
                continue

            # Apply source_filter: skip actions not in allowed sources or modes
            if self._source_filter is not None:
                if action.source_id not in self._source_filter:
                    continue
                if action.mode not in self._source_filter[action.source_id]:
                    continue

            # Namespace: {source_type}__{action_name}
            base_tool_name = f"{action.source_type}__{action.action_name}"

            # Apply action_whitelist: skip actions not in whitelist
            if self._action_whitelist is not None and base_tool_name not in self._action_whitelist:
                continue

            occurrence = base_name_counts.get(base_tool_name, 0)
            base_name_counts[base_tool_name] = occurrence + 1
            if occurrence == 0:
                tool_name = base_tool_name
            else:
                source_suffix = _TOOL_NAME_SAFE_RE.sub("_", action.source_id)
                tool_name = f"{base_tool_name}__source_{source_suffix}"
                if tool_name in self._actions:
                    tool_name = f"{tool_name}_{occurrence + 1}"

            self._actions[tool_name] = action

            source_display = action.source_name or action.source_type
            self._tools.append(
                ToolParam(
                    name=tool_name,
                    description=f"[{source_display}] {action.description}",
                    input_schema=action.input_schema,
                )
            )

    @property
    def search_operators(self) -> list[SearchOperator]:
        return self._search_operators

    def get_tools(self) -> list[ToolParam]:
        # Note: caller must await _ensure_initialized() before calling this
        return self._tools

    @property
    def actions(self) -> dict[str, ConnectorAction]:
        """All resolved actions, keyed by namespaced tool name."""
        return self._actions

    def filtered_tools(self, allowed_tool_names: set[str]) -> list[ToolParam]:
        """Subset of tools explicitly loaded into the current session."""
        if not allowed_tool_names:
            return []
        return [tool for tool in self._tools if tool["name"] in allowed_tool_names]

    def list_toolsets(self) -> list[ToolsetSummary]:
        """One entry per source for prompt rendering and tool_search.

        Returns dicts with: source_id, source_type, source_name, tool_count,
        sample_tool_names (up to 3 for the LLM to skim).
        """
        by_source: dict[str, list[ConnectorAction]] = {}
        for _tool_name, action in self._actions.items():
            by_source.setdefault(action.source_id, []).append(action)

        toolsets: list[ToolsetSummary] = []
        for source_id, actions in by_source.items():
            sample = sorted({a.action_name for a in actions})[:3]
            first = actions[0]
            toolsets.append(
                {
                    "source_id": source_id,
                    "source_type": first.source_type,
                    "source_name": first.source_name,
                    "tool_count": len(actions),
                    "sample_tool_names": sample,
                }
            )
        toolsets.sort(key=lambda t: (t["source_type"], t["source_name"]))
        return toolsets

    def can_handle(self, tool_name: str) -> bool:
        return tool_name in self._actions

    def requires_approval(self, tool_name: str) -> bool:
        action = self._actions.get(tool_name)
        if not action:
            return True
        # Every write action requires interactive approval, regardless of
        # source filters or action whitelists.
        if self._source_filter is not None or self._action_whitelist is not None:
            return False if action.mode == "read" else True
        return action.mode == "write"

    async def check_oauth_required(
        self, tool_name: str, _tool_input: dict, context: ToolContext
    ) -> OAuthRequiredPayload | None:
        action = self._actions.get(tool_name)
        if (
            action is None
            or action.admin_only
            # Connector-manager owns credential scope, provider, and OAuth
            # availability decisions for every source-bound action.
            or context.user_id is None
            or context.skip_permission_check
        ):
            return None

        try:
            async with httpx.AsyncClient(timeout=10.0) as client:
                response = await client.post(
                    f"{self._connector_manager_url}/actions/preflight",
                    json={
                        "source_id": action.source_id,
                        "user_id": context.user_id,
                        "action": action.action_name,
                    },
                )
                response.raise_for_status()
                payload = response.json()
        except Exception as exc:
            logger.error("Connector-manager action preflight failed: %s", exc)
            raise RuntimeError("Connector-manager action preflight failed") from exc

        if not isinstance(payload, Mapping):
            raise TypeError("connector-manager action preflight response must be an object")
        if (
            payload.get("source_id") != action.source_id
            or payload.get("source_type") != action.source_type
        ):
            raise ValueError(
                "connector-manager action preflight response does not match the requested source"
            )
        state = payload.get("state")
        if state == "ready":
            return None
        if state == "missing_org_credentials":
            raise ConnectorActionUnavailable(
                f"Action '{action.action_name}' is unavailable because this source has no organization credentials"
            )
        if state == "unavailable":
            missing = payload.get("missing_scopes")
            if not isinstance(missing, list) or not all(
                isinstance(scope, str) for scope in missing
            ):
                raise TypeError(
                    "connector-manager unavailable preflight response has invalid scopes"
                )
            scope_detail = f" Missing required scopes: {', '.join(missing)}." if missing else ""
            raise ConnectorActionUnavailable(
                f"Action '{action.action_name}' is unavailable with the configured organization credential.{scope_detail}"
            )
        if state not in {"needs_user_auth", "needs_additional_scopes"}:
            raise ValueError("connector-manager returned an unknown action preflight state")
        provider = payload.get("provider")
        oauth_start_url = payload.get("oauth_start_url")
        if (
            not isinstance(provider, str)
            or not provider
            or not isinstance(oauth_start_url, str)
            or not oauth_start_url
        ):
            raise ValueError("connector-manager action preflight response is missing OAuth details")
        return OAuthRequiredPayload(
            source_id=action.source_id,
            source_type=action.source_type,
            provider=provider,
            oauth_start_url=oauth_start_url,
        )

    async def execute(self, tool_name: str, tool_input: dict, context: ToolContext) -> ToolResult:
        action = self._actions.get(tool_name)
        if not action:
            return ToolResult(
                content=[{"type": "text", "text": f"Unknown connector tool: {tool_name}"}],
                is_error=True,
            )

        logger.info(
            f"Executing connector action: {action.action_name} on source {action.source_id}"
        )

        try:
            oauth_required = await self.check_oauth_required(tool_name, tool_input, context)
        except ConnectorActionUnavailable as exc:
            return ToolResult(content=[{"type": "text", "text": str(exc)}], is_error=True)
        except Exception as exc:
            logger.error("Connector action preflight unavailable: %s", exc)
            return ToolResult(
                content=[
                    {
                        "type": "text",
                        "text": "Action availability could not be verified; try again.",
                    }
                ],
                is_error=True,
            )
        if oauth_required is not None:
            return ToolResult(
                content=[encode_oauth_required(oauth_required)],
                is_error=False,
                oauth_required=oauth_required,
            )

        # If this action references a document, check user permissions
        document_id = tool_input.get("document_id")
        if document_id and self._documents_repo and not context.skip_permission_check:
            user_email = context.user_email
            if user_email is None:
                return ToolResult(
                    content=[
                        {
                            "type": "text",
                            "text": f"Document not found: {document_id}",
                        }
                    ],
                    is_error=True,
                )
            doc = await self._documents_repo.get_by_id(
                document_id,
                user_email=user_email,
                user_groups=context.user_groups,
            )
            if doc is None:
                return ToolResult(
                    content=[
                        {
                            "type": "text",
                            "text": f"Document not found: {document_id}",
                        }
                    ],
                    is_error=True,
                )

        try:
            async with httpx.AsyncClient(timeout=120.0) as client:
                response = await client.post(
                    f"{self._connector_manager_url}/action",
                    json={
                        "source_id": action.source_id,
                        "user_id": self._user_id,
                        "action": action.action_name,
                        "params": tool_input,
                    },
                )

                # 412 = needs_user_auth: the user has no per-user credential for an
                # org-wide source. Surface a structured envelope so chat.py can
                # pause the agent loop and the chat UI can render a "Connect
                # <provider>" card instead of a raw error.
                if response.status_code == 412:
                    body = response.json()
                    error_kind = body.get("error") if isinstance(body, Mapping) else None
                    provider = body.get("provider") if isinstance(body, Mapping) else None
                    oauth_start_url = (
                        body.get("oauth_start_url") if isinstance(body, Mapping) else None
                    )
                    if (
                        error_kind not in {"needs_user_auth", "needs_additional_scopes"}
                        or body.get("source_id") != action.source_id
                        or body.get("source_type") != action.source_type
                        or not isinstance(provider, str)
                        or not provider
                        or not isinstance(oauth_start_url, str)
                        or not oauth_start_url
                    ):
                        logger.error(
                            f"connector-manager 412 missing provider/oauth_start_url; body={body}"
                        )
                        return ToolResult(
                            content=[
                                {
                                    "type": "text",
                                    "text": (
                                        "This action requires authorization, but the OAuth "
                                        "start URL was not provided by connector-manager."
                                    ),
                                }
                            ],
                            is_error=True,
                        )
                    payload = OAuthRequiredPayload(
                        source_id=action.source_id,
                        source_type=action.source_type,
                        provider=provider,
                        oauth_start_url=oauth_start_url,
                    )
                    return ToolResult(
                        content=[encode_oauth_required(payload)],
                        is_error=False,
                        oauth_required=payload,
                    )

                response.raise_for_status()

                content_type = response.headers.get("content-type", "")

                if "application/json" not in content_type:
                    content_disposition = response.headers.get("content-disposition", "")
                    if is_textual_content_type(content_type) and not content_disposition:
                        return await text_result_or_sandbox(
                            text=response.text,
                            sandbox_url=self._sandbox_url,
                            chat_id=context.chat_id,
                            file_name=_action_result_file_name(action.action_name, extension="txt"),
                            description="Action returned text",
                        )
                    if not self._sandbox_url:
                        return ToolResult(
                            content=[
                                {
                                    "type": "text",
                                    "text": "Received file but no sandbox is available to save it.",
                                }
                            ],
                            is_error=True,
                        )
                    file_name = response.headers.get("x-file-name", "download")
                    return await write_binary_to_sandbox(
                        self._sandbox_url,
                        response.content,
                        file_name,
                        context.chat_id,
                    )

                result = response.json()
        except httpx.HTTPStatusError as e:
            logger.error(f"Connector action HTTP {e.response.status_code}: {e.response.text}")
            return ToolResult(
                content=[{"type": "text", "text": f"Action failed: {e.response.text}"}],
                is_error=True,
            )
        except Exception as e:
            # Transport-level errors (ReadError, ConnectError, TimeoutException, etc.)
            # don't carry a response. Don't try to access e.response.
            logger.error(f"Connector action failed: {e}", exc_info=True)
            return ToolResult(
                content=[{"type": "text", "text": f"Action failed: {e}"}],
                is_error=True,
            )

        if result.get("status") == "error":
            return ToolResult(
                content=[
                    {
                        "type": "text",
                        "text": f"Action error: {result.get('error', 'Unknown error')}",
                    }
                ],
                is_error=True,
            )

        result_data = result.get("result", {})
        if not result_data:
            return ToolResult(content=[{"type": "text", "text": "Action completed successfully."}])

        return await text_result_or_sandbox(
            text=json.dumps(result_data, indent=2),
            sandbox_url=self._sandbox_url,
            chat_id=context.chat_id,
            file_name=_action_result_file_name(action.action_name, extension="json"),
            description="Action returned JSON",
        )


def _action_result_file_name(action_name: str, *, extension: str) -> str:
    safe_name = "".join(
        char if char.isalnum() or char in ("-", "_") else "_" for char in action_name
    ).strip("_")
    if not safe_name:
        safe_name = "connector_action_result"
    return f"{safe_name}_result.{extension}"
