"""ConnectorToolHandler: discovers and dispatches connector actions."""

from __future__ import annotations

import json
import logging
import re
from collections.abc import Mapping
from dataclasses import asdict, dataclass
from typing import Literal, TypedDict
from urllib.parse import urlencode

import httpx
import redis.asyncio as aioredis
from anthropic.types import ToolParam

from db.connection import get_db_pool
from db.documents import DocumentsRepository
from db.models import Source
from tools.omni_tool_result import OAuthRequiredPayload, encode_oauth_required
from tools.registry import ToolContext, ToolResult
from tools.sandbox import (
    is_textual_content_type,
    text_result_or_sandbox,
    write_binary_to_sandbox,
)

logger = logging.getLogger(__name__)

ACTIONS_CACHE_TTL = 60  # seconds
_TOOL_NAME_SAFE_RE = re.compile(r"[^a-zA-Z0-9_]")

SourceMode = Literal["read", "write"]
# Maps source_id -> list of modes allowed for that source.
SourceFilter = dict[str, list[SourceMode]]


def sources_from_sync_overview_response(payload: object) -> list[Source]:
    if not isinstance(payload, list):
        raise TypeError("connector-manager /sources response must be a list")

    sources: list[Source] = []
    for item in payload:
        if not isinstance(item, Mapping):
            raise TypeError(
                "connector-manager /sources response contains a non-object item"
            )
        source_payload = item["source"]
        if not isinstance(source_payload, Mapping):
            raise TypeError("connector-manager source overview missing source object")
        sources.append(Source.from_row(source_payload))
    return sources


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


class ConnectorToolHandler:
    """Fetches connector actions and dispatches tool calls to connector-manager."""

    def __init__(
        self,
        connector_manager_url: str,
        user_id: str,
        redis_client: aioredis.Redis | None = None,
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
        self._redis = redis_client
        self._prefetched_sources = prefetched_sources
        self._source_filter = source_filter
        self._action_whitelist = action_whitelist  # ["gmail__send_email"]
        self._documents_repo = documents_repo
        self._is_admin = is_admin
        self._actions: dict[str, ConnectorAction] = {}
        self._tools: list[ToolParam] = []
        self._search_operators: list[SearchOperator] = []
        self._initialized = False

    async def _ensure_initialized(self) -> None:
        """Lazily fetch actions from connector-manager, using Redis cache."""
        if self._initialized:
            return

        actions = await self._load_cached_actions()
        if actions is None:
            actions = await self._fetch_actions()
            await self._cache_actions(actions)

        self._build_tools(actions)
        self._initialized = True

    async def _load_cached_actions(self) -> list[ConnectorAction] | None:
        if not self._redis:
            return None
        try:
            cached = await self._redis.get(f"actions:{self._user_id}")
            if cached:
                return [ConnectorAction(**d) for d in json.loads(cached)]
        except Exception as e:
            logger.warning(f"Failed to load cached actions: {e}")
        return None

    async def _cache_actions(self, actions: list[ConnectorAction]) -> None:
        if not self._redis:
            return
        try:
            await self._redis.set(
                f"actions:{self._user_id}",
                json.dumps([asdict(a) for a in actions]),
                ex=ACTIONS_CACHE_TTL,
            )
        except Exception as e:
            logger.warning(f"Failed to cache actions: {e}")

    async def _fetch_actions(self) -> list[ConnectorAction]:
        """Fetch available actions from connector-manager.

        The connector-manager exposes GET /connectors which returns connector info
        including manifests with action definitions. We also need active sources
        to map source_id.
        """
        try:
            async with httpx.AsyncClient(timeout=10.0) as client:
                # Fetch connector info (includes manifests)
                connectors_resp = await client.get(
                    f"{self._connector_manager_url}/connectors"
                )
                connectors_resp.raise_for_status()
                connectors = connectors_resp.json()

                # Use pre-fetched sources if available, otherwise fetch from connector-manager
                if self._prefetched_sources is not None:
                    sources = self._prefetched_sources
                else:
                    sources_resp = await client.get(
                        f"{self._connector_manager_url}/sources"
                    )
                    sources_resp.raise_for_status()
                    sources = sources_from_sync_overview_response(sources_resp.json())

        except Exception as e:
            logger.error(f"Failed to fetch connector info: {e}")
            return []

        # Build a mapping from source_type to list of active sources
        source_by_type: dict[str, list[Source]] = {}
        for source in sources:
            if source.is_active and not source.is_deleted:
                source_by_type.setdefault(source.source_type, []).append(source)

        # Extract search operators from connector manifests
        search_operators: list[SearchOperator] = []
        for connector in connectors:
            source_type = connector.get("source_type", "")
            manifest = connector.get("manifest")
            if not manifest or not connector.get("healthy"):
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
            if not manifest or not connector.get("healthy"):
                continue

            for action_def in manifest.get("actions", []):
                action_source_types = action_def.get("source_types") or []
                if action_source_types and source_type not in action_source_types:
                    continue
                # Find matching active sources for this connector type
                for source in source_by_type.get(source_type, []):
                    actions.append(
                        ConnectorAction(
                            source_id=source.id,
                            source_type=source_type,
                            source_name=source.name or source_type,
                            action_name=action_def["name"],
                            description=action_def.get("description", ""),
                            input_schema=action_def.get(
                                "input_schema", {"type": "object", "properties": {}}
                            ),
                            mode=action_def.get("mode", "write"),
                            required_scopes=action_def.get("required_scopes", []),
                            admin_only=action_def.get("admin_only", False),
                            hidden=action_def.get("hidden", False),
                        )
                    )

        logger.info(
            f"Discovered {len(actions)} connector actions for user {self._user_id}"
        )
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

            # Apply source_filter: skip actions not in allowed sources or modes
            if self._source_filter is not None:
                if action.source_id not in self._source_filter:
                    continue
                if action.mode not in self._source_filter[action.source_id]:
                    continue

            # Namespace: {source_type}__{action_name}
            base_tool_name = f"{action.source_type}__{action.action_name}"

            # Apply action_whitelist: skip actions not in whitelist
            if (
                self._action_whitelist is not None
                and base_tool_name not in self._action_whitelist
            ):
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
        # Pre-authorized when filters are active (background agent context)
        if self._source_filter is not None or self._action_whitelist is not None:
            return False
        action = self._actions.get(tool_name)
        if not action:
            return True
        return action.mode == "write"

    async def check_oauth_required(
        self, tool_name: str, _tool_input: dict, context: ToolContext
    ) -> OAuthRequiredPayload | None:
        action = self._actions.get(tool_name)
        if (
            action is None
            or action.admin_only
            or context.user_id is None
            or context.skip_permission_check
        ):
            return None

        pool = await get_db_pool()
        async with pool.acquire() as conn:
            user_credential = await conn.fetchrow(
                """
                SELECT id, provider, config
                FROM service_credentials
                WHERE source_id = $1 AND user_id = $2
                LIMIT 1
                """,
                action.source_id,
                context.user_id,
            )
            if user_credential is not None:
                required_scopes = set(action.required_scopes or [])
                config = user_credential["config"] or {}
                if isinstance(config, str):
                    try:
                        config = json.loads(config)
                    except json.JSONDecodeError:
                        config = {}
                if not isinstance(config, Mapping):
                    config = {}
                granted_scopes = set(config.get("granted_scopes") or [])
                missing_scopes = sorted(required_scopes - granted_scopes)
                if not missing_scopes:
                    return None

                provider = user_credential["provider"]
                if not provider:
                    return None
                query = urlencode(
                    {
                        "source_id": action.source_id,
                        "flow": "user_write",
                        "required_scopes": ",".join(missing_scopes),
                    }
                )
                return OAuthRequiredPayload(
                    source_id=action.source_id,
                    source_type=action.source_type,
                    provider=provider,
                    oauth_start_url=f"/api/oauth/start?{query}",
                )

            org_credential = await conn.fetchrow(
                """
                SELECT provider
                FROM service_credentials
                WHERE source_id = $1 AND user_id IS NULL
                LIMIT 1
                """,
                action.source_id,
            )

        if org_credential is None:
            return None

        provider = org_credential["provider"]
        if not provider:
            return None

        return OAuthRequiredPayload(
            source_id=action.source_id,
            source_type=action.source_type,
            provider=provider,
            oauth_start_url=f"/api/oauth/start?source_id={action.source_id}",
        )

    async def execute(
        self, tool_name: str, tool_input: dict, context: ToolContext
    ) -> ToolResult:
        action = self._actions.get(tool_name)
        if not action:
            return ToolResult(
                content=[
                    {"type": "text", "text": f"Unknown connector tool: {tool_name}"}
                ],
                is_error=True,
            )

        logger.info(
            f"Executing connector action: {action.action_name} on source {action.source_id}"
        )

        # If this action references a document, check user permissions
        document_id = tool_input.get("document_id")
        if document_id and self._documents_repo and not context.skip_permission_check:
            user_email = context.user_email
            doc = await self._documents_repo.get_by_id(
                document_id, user_email=user_email
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
                    provider = body.get("provider")
                    oauth_start_url = body.get("oauth_start_url")
                    if not provider or not oauth_start_url:
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
                    content_disposition = response.headers.get(
                        "content-disposition", ""
                    )
                    if (
                        is_textual_content_type(content_type)
                        and not content_disposition
                    ):
                        return await text_result_or_sandbox(
                            text=response.text,
                            sandbox_url=self._sandbox_url,
                            chat_id=context.chat_id,
                            file_name=_action_result_file_name(
                                action.action_name, extension="txt"
                            ),
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
            logger.error(
                f"Connector action HTTP {e.response.status_code}: {e.response.text}"
            )
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
            return ToolResult(
                content=[{"type": "text", "text": "Action completed successfully."}]
            )

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
