"""ConnectorToolHandler: discovers and dispatches connector actions."""

from __future__ import annotations

import json
import logging
from dataclasses import asdict, dataclass
from typing import Literal

import httpx
import redis.asyncio as aioredis

from db.documents import DocumentsRepository
from db.models import Source
from tools.registry import ToolContext, ToolResult
from tools.sandbox import write_binary_to_sandbox

logger = logging.getLogger(__name__)

ACTIONS_CACHE_TTL = 60  # seconds


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
    mode: str  # "read" | "write"


class ConnectorToolHandler:
    """Fetches connector actions and dispatches tool calls to connector-manager."""

    def __init__(
        self,
        connector_manager_url: str,
        user_id: str,
        redis_client: aioredis.Redis | None = None,
        prefetched_sources: list[Source] | None = None,
        source_filter: dict[str, list[str]] | None = None,
        action_whitelist: list[str] | None = None,
        documents_repo: DocumentsRepository | None = None,
        sandbox_url: str | None = None,
    ) -> None:
        self._connector_manager_url = connector_manager_url.rstrip("/")
        self._sandbox_url = sandbox_url.rstrip("/") if sandbox_url else None
        self._user_id = user_id
        self._redis = redis_client
        self._prefetched_sources = prefetched_sources
        self._source_filter = source_filter  # {source_id: ["read","write"]}
        self._action_whitelist = action_whitelist  # ["gmail__send_email"]
        self._documents_repo = documents_repo
        self._actions: dict[str, ConnectorAction] = {}
        self._tools: list[dict] = []
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

    async def _load_cached_actions(self) -> list[dict] | None:
        if not self._redis:
            return None
        try:
            cached = await self._redis.get(f"actions:{self._user_id}")
            if cached:
                return json.loads(cached)
        except Exception as e:
            logger.warning(f"Failed to load cached actions: {e}")
        return None

    async def _cache_actions(self, actions: list[dict]) -> None:
        if not self._redis:
            return
        try:
            await self._redis.set(
                f"actions:{self._user_id}",
                json.dumps(actions),
                ex=ACTIONS_CACHE_TTL,
            )
        except Exception as e:
            logger.warning(f"Failed to cache actions: {e}")

    async def _fetch_actions(self) -> list[dict]:
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
                    sources = [asdict(s) for s in self._prefetched_sources]
                else:
                    sources_resp = await client.get(
                        f"{self._connector_manager_url}/sources"
                    )
                    sources_resp.raise_for_status()
                    sources = sources_resp.json()

        except Exception as e:
            logger.error(f"Failed to fetch connector info: {e}")
            return []

        # Build a mapping from source_type to list of active sources
        source_by_type: dict[str, list[dict]] = {}
        for source in sources:
            if source.get("is_active") and not source.get("is_deleted"):
                st = source.get("source_type", "")
                source_by_type.setdefault(st, []).append(source)

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
        actions: list[dict] = []
        for connector in connectors:
            source_type = connector.get("source_type", "")
            manifest = connector.get("manifest")
            if not manifest or not connector.get("healthy"):
                continue

            for action_def in manifest.get("actions", []):
                # Find matching active sources for this connector type
                matching_sources = source_by_type.get(source_type, [])
                for source in matching_sources:
                    actions.append(
                        {
                            "source_id": source["id"],
                            "source_type": source_type,
                            "source_name": source.get("name", source_type),
                            "action_name": action_def["name"],
                            "description": action_def.get("description", ""),
                            "input_schema": action_def.get(
                                "input_schema", {"type": "object", "properties": {}}
                            ),
                            "mode": action_def.get("mode", "write"),
                        }
                    )

        logger.info(
            f"Discovered {len(actions)} connector actions for user {self._user_id}"
        )
        return actions

    def _build_tools(self, actions: list[dict]) -> None:
        """Convert connector actions to LLM tool format."""
        self._actions.clear()
        self._tools.clear()

        seen_tools: set[str] = set()
        for action in actions:
            source_id = action["source_id"]

            # Apply source_filter: skip actions not in allowed sources or modes
            if self._source_filter is not None:
                if source_id not in self._source_filter:
                    continue
                allowed_modes = self._source_filter[source_id]
                if action.get("mode", "write") not in allowed_modes:
                    continue

            # Namespace: {source_type}__{action_name}
            tool_name = f"{action['source_type']}__{action['action_name']}"

            # Apply action_whitelist: skip actions not in whitelist
            if self._action_whitelist is not None:
                if tool_name not in self._action_whitelist:
                    continue

            if tool_name in seen_tools:
                continue
            seen_tools.add(tool_name)

            self._actions[tool_name] = ConnectorAction(
                source_id=action["source_id"],
                source_type=action["source_type"],
                source_name=action["source_name"],
                action_name=action["action_name"],
                description=action["description"],
                input_schema=action["input_schema"],
                mode=action["mode"],
            )

            source_display = action["source_name"] or action["source_type"]
            self._tools.append(
                {
                    "name": tool_name,
                    "description": f"[{source_display}] {action['description']}",
                    "input_schema": action["input_schema"],
                }
            )

    @property
    def search_operators(self) -> list[SearchOperator]:
        return self._search_operators

    def get_tools(self) -> list[dict]:
        # Note: caller must await _ensure_initialized() before calling this
        return self._tools

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
                        "action": action.action_name,
                        "params": tool_input,
                    },
                )
                response.raise_for_status()

                # Binary file response — connectors set x-file-name on file downloads
                if response.headers.get("x-file-name"):
                    if not self._sandbox_url:
                        return ToolResult(
                            content=[
                                {
                                    "type": "text",
                                    "text": "Received binary file but no sandbox is available to save it.",
                                }
                            ],
                            is_error=True,
                        )
                    file_name = response.headers["x-file-name"]
                    return await write_binary_to_sandbox(
                        self._sandbox_url,
                        response.content,
                        file_name,
                        context.chat_id,
                    )

                result = response.json()
        except Exception as e:
            logger.error(f"Connector action failed: {e}")
            return ToolResult(
                content=[{"type": "text", "text": f"Action failed: {str(e)}"}],
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

        # Return the result as text content
        result_data = result.get("result", {})
        return ToolResult(
            content=[
                {
                    "type": "text",
                    "text": (
                        json.dumps(result_data, indent=2)
                        if result_data
                        else "Action completed successfully."
                    ),
                }
            ],
        )
