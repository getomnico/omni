"""ConnectorToolHandler: discovers and dispatches connector actions."""

from __future__ import annotations

import json
import logging
from dataclasses import dataclass

import httpx
import redis.asyncio as aioredis

from tools.registry import ToolContext, ToolResult

logger = logging.getLogger(__name__)

ACTIONS_CACHE_TTL = 60  # seconds


@dataclass
class ConnectorAction:
    """Internal mapping from LLM tool name to connector action details."""

    source_id: str
    source_type: str
    source_name: str
    action_name: str
    description: str
    parameters: dict
    mode: str  # "read" | "write"


class ConnectorToolHandler:
    """Fetches connector actions and dispatches tool calls to connector-manager."""

    def __init__(
        self,
        connector_manager_url: str,
        user_id: str,
        redis_client: aioredis.Redis | None = None,
    ) -> None:
        self._connector_manager_url = connector_manager_url.rstrip("/")
        self._user_id = user_id
        self._redis = redis_client
        self._actions: dict[str, ConnectorAction] = {}
        self._tools: list[dict] = []
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

                # Fetch active sources to map source_type -> source_id
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
                            "parameters": action_def.get("parameters", {}),
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

        for action in actions:
            # Namespace: {source_type}__{action_name}
            tool_name = f"{action['source_type']}__{action['action_name']}"

            self._actions[tool_name] = ConnectorAction(
                source_id=action["source_id"],
                source_type=action["source_type"],
                source_name=action["source_name"],
                action_name=action["action_name"],
                description=action["description"],
                parameters=action["parameters"],
                mode=action["mode"],
            )

            # Convert connector parameter definitions to JSON Schema
            properties = {}
            required = []
            for param_name, param_def in action["parameters"].items():
                prop: dict = {
                    "type": param_def.get("type", "string"),
                }
                if param_def.get("description"):
                    prop["description"] = param_def["description"]
                properties[param_name] = prop
                if param_def.get("required"):
                    required.append(param_name)

            source_display = action["source_name"] or action["source_type"]
            self._tools.append(
                {
                    "name": tool_name,
                    "description": f"[{source_display}] {action['description']}",
                    "input_schema": {
                        "type": "object",
                        "properties": properties,
                        "required": required,
                    },
                }
            )

    def get_tools(self) -> list[dict]:
        # Note: caller must await _ensure_initialized() before calling this
        return self._tools

    def can_handle(self, tool_name: str) -> bool:
        return tool_name in self._actions

    def requires_approval(self, tool_name: str) -> bool:
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

        try:
            async with httpx.AsyncClient(timeout=30.0) as client:
                response = await client.post(
                    f"{self._connector_manager_url}/action",
                    json={
                        "source_id": action.source_id,
                        "action": action.action_name,
                        "params": tool_input,
                    },
                )
                response.raise_for_status()
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
