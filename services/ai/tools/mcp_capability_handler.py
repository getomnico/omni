"""MCP capability handler: search/load connector MCP resources and prompts."""

from __future__ import annotations

import asyncio
import hashlib
import json
import logging
import re
from dataclasses import dataclass
from typing import Any

import httpx
from anthropic.types import ToolParam
from pydantic import BaseModel, ConfigDict, Field, TypeAdapter

from db.models import Source
from tools.connector_handler import SourceFilter, sources_from_sync_overview_response
from tools.registry import ToolContext, ToolResult
from tools.searcher_client import (
    CapabilitiesSyncRequest,
    CapabilitySearchRequest,
    CapabilityUpsert,
    SearcherClient,
)

logger = logging.getLogger(__name__)

_TOOL_NAMES = {"resource_search", "load_resource", "prompt_search", "load_prompt"}
_DEFAULT_LIMIT = 10
_MAX_LIMIT = 25
_TOKEN_RE = re.compile(r"[a-z0-9]+")
_RESOURCE_INLINE_MAX_BYTES = 24_000
_RESOURCE_PREVIEW_MAX_LINES = 80
_RESOURCE_PREVIEW_MAX_BYTES = 12_000

# Connector-manager returns Omni connector manifests, not raw MCP SDK objects.
# These DTOs mirror shared::models and the connector SDKs; the adapters populate
# them from MCP list_resources/list_resource_templates/list_prompts responses.


class McpResourceDefinition(BaseModel):
    model_config = ConfigDict(extra="ignore")

    uri_template: str
    name: str
    description: str | None = None
    mime_type: str | None = None


class McpPromptArgument(BaseModel):
    model_config = ConfigDict(extra="ignore")

    name: str
    description: str | None = None
    required: bool = False

    def to_data(self) -> dict[str, object]:
        data: dict[str, object] = {"name": self.name, "required": self.required}
        if self.description is not None:
            data["description"] = self.description
        return data


class McpPromptDefinition(BaseModel):
    model_config = ConfigDict(extra="ignore")

    name: str
    description: str | None = None
    arguments: list[McpPromptArgument] = Field(default_factory=list)


class McpConnectorManifest(BaseModel):
    model_config = ConfigDict(extra="ignore")

    mcp_enabled: bool = False
    resources: list[McpResourceDefinition] = Field(default_factory=list)
    prompts: list[McpPromptDefinition] = Field(default_factory=list)


class McpConnectorInfo(BaseModel):
    model_config = ConfigDict(extra="ignore")

    source_type: str
    healthy: bool
    manifest: McpConnectorManifest | None = None


_CONNECTORS_RESPONSE_ADAPTER = TypeAdapter(list[McpConnectorInfo])


@dataclass(frozen=True)
class McpResourceRecord:
    id: str
    source_id: str
    source_type: str
    source_name: str
    uri_template: str
    name: str
    description: str
    mime_type: str | None = None

    @property
    def requires_uri(self) -> bool:
        return "{" in self.uri_template and "}" in self.uri_template


@dataclass(frozen=True)
class McpPromptRecord:
    id: str
    source_id: str
    source_type: str
    source_name: str
    name: str
    description: str
    arguments: list[McpPromptArgument]


class McpCapabilityHandler:
    """Searches and loads MCP resources/prompts exposed by connector manifests."""

    _publish_lock = asyncio.Lock()
    _published_capability_keys: set[tuple[int, str]] = set()

    def __init__(
        self,
        connector_manager_url: str,
        searcher_client: SearcherClient | None = None,
        prefetched_sources: list[Source] | None = None,
        source_filter: SourceFilter | None = None,
    ) -> None:
        self._connector_manager_url = connector_manager_url.rstrip("/")
        self._searcher_client = searcher_client
        self._prefetched_sources = prefetched_sources
        self._source_filter = source_filter
        self._resources: dict[str, McpResourceRecord] = {}
        self._prompts: dict[str, McpPromptRecord] = {}
        self._initialized = False

    async def refresh(self) -> None:
        if self._initialized:
            return

        try:
            async with httpx.AsyncClient(timeout=10.0) as client:
                connectors_resp = await client.get(
                    f"{self._connector_manager_url}/connectors"
                )
                connectors_resp.raise_for_status()
                connectors = _CONNECTORS_RESPONSE_ADAPTER.validate_python(
                    connectors_resp.json()
                )

                if self._prefetched_sources is not None:
                    sources = self._prefetched_sources
                else:
                    sources_resp = await client.get(
                        f"{self._connector_manager_url}/sources"
                    )
                    sources_resp.raise_for_status()
                    sources = sources_from_sync_overview_response(sources_resp.json())
        except Exception as e:
            logger.warning(f"Failed to fetch MCP connector capabilities: {e}")
            self._initialized = True
            return

        active_sources_by_type: dict[str, list[Source]] = {}
        for source in sources:
            if not source.is_active or source.is_deleted:
                continue
            if not self._source_allows_read(source.id):
                continue
            active_sources_by_type.setdefault(source.source_type, []).append(source)

        resources: dict[str, McpResourceRecord] = {}
        prompts: dict[str, McpPromptRecord] = {}

        for connector in connectors:
            if not connector.healthy or connector.manifest is None:
                continue
            source_type = connector.source_type
            manifest = connector.manifest
            if not manifest.mcp_enabled:
                continue

            matching_sources = active_sources_by_type.get(source_type, [])
            if not matching_sources:
                continue

            for source in matching_sources:
                source_name = source.name or source_type
                for resource_def in manifest.resources:
                    record = self._resource_record(
                        source, source_type, source_name, resource_def
                    )
                    resources[record.id] = record

                for prompt_def in manifest.prompts:
                    record = self._prompt_record(
                        source, source_type, source_name, prompt_def
                    )
                    prompts[record.id] = record

        self._resources = resources
        self._prompts = prompts
        self._initialized = True

    def has_capabilities(self) -> bool:
        return bool(self._resources or self._prompts)

    def get_tools(self) -> list[ToolParam]:
        return [
            {
                "name": "resource_search",
                "description": (
                    "Search MCP resources exposed by connected sources. Use this for "
                    "source-provided reference data or connector-managed resource content. "
                    "Call load_resource with a returned resource_id to read it."
                ),
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Keywords matched against resource name, description, source, and URI template.",
                        },
                        "limit": {
                            "type": "integer",
                            "description": f"Max resources to return (default {_DEFAULT_LIMIT}, max {_MAX_LIMIT}).",
                            "default": _DEFAULT_LIMIT,
                            "minimum": 1,
                            "maximum": _MAX_LIMIT,
                        },
                    },
                    "required": ["query"],
                },
            },
            {
                "name": "load_resource",
                "description": (
                    "Load an exact MCP resource returned by resource_search. For URI "
                    "templates, provide the concrete uri. For large text resources, use "
                    "start_line/end_line to read focused chunks."
                ),
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "resource_id": {
                            "type": "string",
                            "description": "Exact resource_id returned by resource_search.",
                        },
                        "uri": {
                            "type": "string",
                            "description": "Concrete URI to read; required when the resource has a URI template.",
                        },
                        "start_line": {
                            "type": "integer",
                            "description": "Optional 1-based start line for text resources, inclusive.",
                            "minimum": 1,
                        },
                        "end_line": {
                            "type": "integer",
                            "description": "Optional 1-based end line for text resources, inclusive.",
                            "minimum": 1,
                        },
                    },
                    "required": ["resource_id"],
                },
            },
            {
                "name": "prompt_search",
                "description": (
                    "Search MCP prompt templates exposed by connected sources. Use this "
                    "for connector-provided workflows, templates, or few-shot interaction patterns. "
                    "Call load_prompt with a returned prompt_id to inspect/apply it."
                ),
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Keywords matched against prompt name, description, source, and arguments.",
                        },
                        "limit": {
                            "type": "integer",
                            "description": f"Max prompts to return (default {_DEFAULT_LIMIT}, max {_MAX_LIMIT}).",
                            "default": _DEFAULT_LIMIT,
                            "minimum": 1,
                            "maximum": _MAX_LIMIT,
                        },
                    },
                    "required": ["query"],
                },
            },
            {
                "name": "load_prompt",
                "description": (
                    "Load an exact MCP prompt template returned by prompt_search. The "
                    "result is prompt-template content, not actual chat history."
                ),
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "prompt_id": {
                            "type": "string",
                            "description": "Exact prompt_id returned by prompt_search.",
                        },
                        "arguments": {
                            "type": "object",
                            "description": "Arguments required by the prompt definition.",
                        },
                    },
                    "required": ["prompt_id"],
                },
            },
        ]

    def can_handle(self, tool_name: str) -> bool:
        return tool_name in _TOOL_NAMES

    def requires_approval(self, tool_name: str) -> bool:
        return False

    async def execute(
        self, tool_name: str, tool_input: dict, context: ToolContext
    ) -> ToolResult:
        await self.refresh()
        if tool_name == "resource_search":
            return await self._resource_search(tool_input)
        if tool_name == "load_resource":
            return await self._load_resource(tool_input)
        if tool_name == "prompt_search":
            return await self._prompt_search(tool_input)
        if tool_name == "load_prompt":
            return await self._load_prompt(tool_input)
        return ToolResult(
            content=[{"type": "text", "text": f"Unknown MCP capability tool: {tool_name}"}],
            is_error=True,
        )

    async def publish_capabilities(self) -> None:
        await self.refresh()
        if self._searcher_client is None:
            return

        capabilities = self._capabilities()
        if not capabilities:
            return

        publish_key = (
            id(self._searcher_client),
            self._capability_fingerprint(capabilities),
        )
        if publish_key in self._published_capability_keys:
            return

        async with self._publish_lock:
            if publish_key in self._published_capability_keys:
                return
            try:
                grouped: dict[tuple[str, str], list[CapabilityUpsert]] = {}
                for capability in capabilities:
                    if not capability.source_id:
                        continue
                    grouped.setdefault((capability.source_id, capability.capability_type), []).append(
                        capability
                    )
                for (publisher_id, capability_type), group in grouped.items():
                    await self._searcher_client.sync_capabilities(
                        CapabilitiesSyncRequest(
                            publisher_id=publisher_id,
                            capability_type=capability_type,
                            capabilities=group,
                        )
                    )
            except Exception as e:
                logger.warning(f"Failed to publish MCP capabilities: {e}")
                return
            self._published_capability_keys.add(publish_key)

    async def _resource_search(self, tool_input: dict) -> ToolResult:
        query = (tool_input.get("query") or "").strip()
        if not query:
            return ToolResult(
                content=[{"type": "text", "text": "Missing required parameter: query"}],
                is_error=True,
            )
        if not _TOKEN_RE.findall(query.lower()):
            return ToolResult(
                content=[{"type": "text", "text": f"No searchable tokens in query: {query!r}"}],
                is_error=True,
            )
        limit = self._parse_limit(tool_input.get("limit"))
        matches = await self._search_capabilities("resource", query, limit)
        if not matches:
            return ToolResult(content=[{"type": "text", "text": f"No MCP resources matched {query!r}."}])

        lines = [f"Found {len(matches)} MCP resource(s) matching {query!r}:"]
        for record in matches:
            assert isinstance(record, McpResourceRecord)
            desc = f" — {record.description}" if record.description else ""
            uri_note = " (URI template; pass concrete uri to load_resource)" if record.requires_uri else ""
            mime = f" · {record.mime_type}" if record.mime_type else ""
            lines.append(
                f"- {record.id}: {record.name}{desc} [{record.source_name}/{record.source_type}] uri_template={record.uri_template!r}{mime}{uri_note}"
            )
        lines.append("Call load_resource with the exact resource_id to read a resource.")
        return ToolResult(content=[{"type": "text", "text": "\n".join(lines)}])

    async def _prompt_search(self, tool_input: dict) -> ToolResult:
        query = (tool_input.get("query") or "").strip()
        if not query:
            return ToolResult(
                content=[{"type": "text", "text": "Missing required parameter: query"}],
                is_error=True,
            )
        if not _TOKEN_RE.findall(query.lower()):
            return ToolResult(
                content=[{"type": "text", "text": f"No searchable tokens in query: {query!r}"}],
                is_error=True,
            )
        limit = self._parse_limit(tool_input.get("limit"))
        matches = await self._search_capabilities("prompt", query, limit)
        if not matches:
            return ToolResult(content=[{"type": "text", "text": f"No MCP prompts matched {query!r}."}])

        lines = [f"Found {len(matches)} MCP prompt(s) matching {query!r}:"]
        for record in matches:
            assert isinstance(record, McpPromptRecord)
            desc = f" — {record.description}" if record.description else ""
            args = self._format_argument_summary(record.arguments)
            arg_note = f" args: {args}" if args else ""
            lines.append(
                f"- {record.id}: {record.name}{desc} [{record.source_name}/{record.source_type}]{arg_note}"
            )
        lines.append("Call load_prompt with the exact prompt_id and any required arguments.")
        return ToolResult(content=[{"type": "text", "text": "\n".join(lines)}])

    async def _search_capabilities(
        self, capability_type: str, query: str, limit: int
    ) -> list[McpResourceRecord | McpPromptRecord]:
        if self._searcher_client is None:
            raise RuntimeError(f"{capability_type}_search requires a searcher client")
        records: dict[str, McpResourceRecord | McpPromptRecord]
        records = self._resources if capability_type == "resource" else self._prompts
        if not records:
            return []
        allowed_source_ids = sorted({record.source_id for record in records.values()})
        response = await self._searcher_client.search_capabilities(
            CapabilitySearchRequest(
                capability_type=capability_type,
                query=query,
                limit=limit,
                allowed_ids=sorted(records),
                allowed_source_ids=allowed_source_ids,
            )
        )
        matches: list[McpResourceRecord | McpPromptRecord] = []
        seen: set[str] = set()
        for result in response.results:
            record = records.get(result.id)
            if record is None or result.id in seen:
                continue
            seen.add(result.id)
            matches.append(record)
        return matches

    async def _load_resource(self, tool_input: dict) -> ToolResult:
        resource_id = (tool_input.get("resource_id") or "").strip()
        if not resource_id:
            return ToolResult(
                content=[{"type": "text", "text": "Missing required parameter: resource_id"}],
                is_error=True,
            )
        record = self._resources.get(resource_id)
        if record is None:
            return ToolResult(
                content=[{"type": "text", "text": f"Unknown or inaccessible MCP resource: {resource_id}"}],
                is_error=True,
            )

        uri = (tool_input.get("uri") or "").strip()
        if record.requires_uri and not uri:
            return ToolResult(
                content=[
                    {
                        "type": "text",
                        "text": (
                            f"Resource {resource_id} uses URI template {record.uri_template!r}; "
                            "provide a concrete uri."
                        ),
                    }
                ],
                is_error=True,
            )
        read_uri = uri or record.uri_template

        line_error, start_line, end_line = self._parse_line_range(
            tool_input.get("start_line"), tool_input.get("end_line")
        )
        if line_error:
            return ToolResult(content=[{"type": "text", "text": line_error}], is_error=True)

        try:
            async with httpx.AsyncClient(timeout=60.0) as client:
                response = await client.post(
                    f"{self._connector_manager_url}/resource",
                    json={"source_id": record.source_id, "uri": read_uri},
                )
                response.raise_for_status()
                payload = response.json()
        except Exception as e:
            logger.warning(f"Failed to load MCP resource {resource_id}: {e}")
            return ToolResult(
                content=[{"type": "text", "text": f"Failed to load MCP resource: {resource_id}"}],
                is_error=True,
            )

        return ToolResult(
            content=[
                {
                    "type": "text",
                    "text": self._format_resource_result(
                        record, read_uri, payload, start_line, end_line
                    ),
                }
            ]
        )

    async def _load_prompt(self, tool_input: dict) -> ToolResult:
        prompt_id = (tool_input.get("prompt_id") or "").strip()
        if not prompt_id:
            return ToolResult(
                content=[{"type": "text", "text": "Missing required parameter: prompt_id"}],
                is_error=True,
            )
        record = self._prompts.get(prompt_id)
        if record is None:
            return ToolResult(
                content=[{"type": "text", "text": f"Unknown or inaccessible MCP prompt: {prompt_id}"}],
                is_error=True,
            )

        raw_arguments = tool_input.get("arguments")
        arguments = raw_arguments if isinstance(raw_arguments, dict) else {}
        if raw_arguments is not None and not isinstance(raw_arguments, dict):
            return ToolResult(
                content=[{"type": "text", "text": "Prompt arguments must be an object."}],
                is_error=True,
            )
        missing = self._missing_required_arguments(record.arguments, arguments)
        if missing:
            return ToolResult(
                content=[{"type": "text", "text": f"Missing required prompt argument(s): {', '.join(missing)}"}],
                is_error=True,
            )

        try:
            async with httpx.AsyncClient(timeout=60.0) as client:
                response = await client.post(
                    f"{self._connector_manager_url}/prompt",
                    json={
                        "source_id": record.source_id,
                        "name": record.name,
                        "arguments": arguments or None,
                    },
                )
                response.raise_for_status()
                payload = response.json()
        except Exception as e:
            logger.warning(f"Failed to load MCP prompt {prompt_id}: {e}")
            return ToolResult(
                content=[{"type": "text", "text": f"Failed to load MCP prompt: {prompt_id}"}],
                is_error=True,
            )

        return ToolResult(
            content=[
                {
                    "type": "text",
                    "text": self._format_prompt_result(record, payload),
                }
            ]
        )

    def _capabilities(self) -> list[CapabilityUpsert]:
        capabilities: list[CapabilityUpsert] = []
        for record in self._resources.values():
            capabilities.append(
                CapabilityUpsert(
                    id=record.id,
                    capability_type="resource",
                    name=record.name,
                    description=record.description,
                    publisher_id=record.source_id,
                    source_id=record.source_id,
                    source_type=record.source_type,
                    search_text=(
                        f"{record.name} {record.description} {record.source_type} "
                        f"{record.source_name} {record.uri_template} {record.mime_type or ''}"
                    ),
                    data={
                        "source_id": record.source_id,
                        "source_type": record.source_type,
                        "source_name": record.source_name,
                        "uri_template": record.uri_template,
                        "name": record.name,
                        "description": record.description,
                        "mime_type": record.mime_type,
                    },
                )
            )
        for record in self._prompts.values():
            arg_text = self._format_argument_search_text(record.arguments)
            capabilities.append(
                CapabilityUpsert(
                    id=record.id,
                    capability_type="prompt",
                    name=record.name,
                    description=record.description,
                    publisher_id=record.source_id,
                    source_id=record.source_id,
                    source_type=record.source_type,
                    search_text=(
                        f"{record.name} {record.description} {record.source_type} "
                        f"{record.source_name} {arg_text}"
                    ),
                    data={
                        "source_id": record.source_id,
                        "source_type": record.source_type,
                        "source_name": record.source_name,
                        "name": record.name,
                        "description": record.description,
                        "arguments": [arg.to_data() for arg in record.arguments],
                    },
                )
            )
        return capabilities

    def _resource_record(
        self,
        source: Source,
        source_type: str,
        source_name: str,
        resource_def: McpResourceDefinition,
    ) -> McpResourceRecord:
        return McpResourceRecord(
            id=f"resource:{source.id}:{self._short_hash(resource_def.uri_template)}",
            source_id=source.id,
            source_type=source_type,
            source_name=source_name,
            uri_template=resource_def.uri_template,
            name=resource_def.name,
            description=resource_def.description or "",
            mime_type=resource_def.mime_type,
        )

    def _prompt_record(
        self,
        source: Source,
        source_type: str,
        source_name: str,
        prompt_def: McpPromptDefinition,
    ) -> McpPromptRecord:
        return McpPromptRecord(
            id=f"prompt:{source.id}:{prompt_def.name}",
            source_id=source.id,
            source_type=source_type,
            source_name=source_name,
            name=prompt_def.name,
            description=prompt_def.description or "",
            arguments=prompt_def.arguments,
        )

    def _source_allows_read(self, source_id: str) -> bool:
        if self._source_filter is None:
            return True
        return source_id in self._source_filter and "read" in self._source_filter[source_id]

    @staticmethod
    def _short_hash(value: str) -> str:
        return hashlib.sha256(value.encode("utf-8")).hexdigest()[:16]

    @staticmethod
    def _capability_fingerprint(capabilities: list[CapabilityUpsert]) -> str:
        payload = [capability.model_dump() for capability in capabilities]
        payload.sort(key=lambda capability: capability["id"])
        raw = json.dumps(payload, sort_keys=True, separators=(",", ":"))
        return hashlib.sha256(raw.encode("utf-8")).hexdigest()

    @staticmethod
    def _parse_limit(raw_limit: object) -> int:
        try:
            value = _DEFAULT_LIMIT if raw_limit is None else int(raw_limit)
            return max(1, min(value, _MAX_LIMIT))
        except (TypeError, ValueError):
            return _DEFAULT_LIMIT

    @staticmethod
    def _parse_line_range(
        raw_start: object, raw_end: object
    ) -> tuple[str | None, int | None, int | None]:
        try:
            start = int(raw_start) if raw_start is not None else None
            end = int(raw_end) if raw_end is not None else None
        except (TypeError, ValueError):
            return "start_line and end_line must be integers.", None, None
        if start is not None and start < 1:
            return "start_line must be >= 1.", None, None
        if end is not None and end < 1:
            return "end_line must be >= 1.", None, None
        if start is not None and end is not None and start > end:
            return "start_line must be <= end_line.", None, None
        return None, start, end

    @staticmethod
    def _missing_required_arguments(
        argument_defs: list[McpPromptArgument], arguments: dict[str, Any]
    ) -> list[str]:
        return [
            arg.name
            for arg in argument_defs
            if arg.required and arg.name not in arguments
        ]

    @staticmethod
    def _format_argument_search_text(arguments: list[McpPromptArgument]) -> str:
        parts: list[str] = []
        for arg in arguments:
            parts.append(arg.name)
            if arg.description is not None:
                parts.append(arg.description)
            parts.append("required" if arg.required else "optional")
        return " ".join(parts)

    @staticmethod
    def _format_argument_summary(arguments: list[McpPromptArgument]) -> str:
        return ", ".join(
            f"{arg.name}{'*' if arg.required else ''}" for arg in arguments
        )

    def _format_resource_result(
        self,
        record: McpResourceRecord,
        read_uri: str,
        payload: Any,
        start_line: int | None,
        end_line: int | None,
    ) -> str:
        contents = payload.get("contents") if isinstance(payload, dict) else None
        if not isinstance(contents, list):
            return (
                f"Loaded MCP resource {record.id} from {record.source_name} ({record.source_type}), "
                "but the connector returned an unexpected response shape."
            )

        header = [
            f"MCP resource: {record.name}",
            f"resource_id: {record.id}",
            f"source: {record.source_name} ({record.source_type}, source_id={record.source_id})",
            f"uri: {read_uri}",
        ]
        if record.description:
            header.append(f"description: {record.description}")

        sections: list[str] = ["\n".join(header)]
        text_items = [item for item in contents if isinstance(item, dict) and isinstance(item.get("text"), str)]
        blob_items = [item for item in contents if isinstance(item, dict) and "blob" in item]

        if not text_items:
            if blob_items:
                sections.append(
                    f"The resource returned {len(blob_items)} binary/blob content item(s). Binary MCP resources are not included inline."
                )
            else:
                sections.append("The resource returned no text content.")
            return "\n\n".join(sections)

        has_range = start_line is not None or end_line is not None
        for index, item in enumerate(text_items, start=1):
            text = item["text"]
            item_uri = item.get("uri") if isinstance(item.get("uri"), str) else read_uri
            mime_type = item.get("mime_type") if isinstance(item.get("mime_type"), str) else record.mime_type
            total_bytes = len(text.encode("utf-8"))
            lines = text.split("\n")
            total_lines = len(lines)

            if has_range:
                start = start_line or 1
                if start > total_lines:
                    sections.append(
                        f"Content item {index}: uri={item_uri} mime_type={mime_type or 'unknown'}\n"
                        f"Requested start_line {start} is past the end of the text ({total_lines} lines)."
                    )
                    continue
                end = min(end_line or total_lines, total_lines)
                selected = lines[start - 1 : end]
                rendered = "\n".join(selected)
                sections.append(
                    f"Content item {index}: uri={item_uri} mime_type={mime_type or 'unknown'}\n"
                    f"Returned lines {start}-{end} of {total_lines} ({len(rendered.encode('utf-8'))} bytes):\n"
                    f"```\n{rendered}\n```"
                )
                continue

            if total_bytes <= _RESOURCE_INLINE_MAX_BYTES:
                sections.append(
                    f"Content item {index}: uri={item_uri} mime_type={mime_type or 'unknown'}\n"
                    f"Returned full text: lines 1-{total_lines} of {total_lines} ({total_bytes} bytes):\n"
                    f"```\n{text}\n```"
                )
                continue

            preview_lines: list[str] = []
            preview_bytes = 0
            for line in lines[:_RESOURCE_PREVIEW_MAX_LINES]:
                line_bytes = len((line + "\n").encode("utf-8"))
                if preview_bytes + line_bytes > _RESOURCE_PREVIEW_MAX_BYTES:
                    break
                preview_lines.append(line)
                preview_bytes += line_bytes
            preview_end = len(preview_lines)
            preview = "\n".join(preview_lines)
            sections.append(
                f"Content item {index}: uri={item_uri} mime_type={mime_type or 'unknown'}\n"
                f"Resource is too large to include inline ({total_bytes} bytes, {total_lines} lines).\n"
                f"Preview lines 1-{preview_end} of {total_lines}:\n```\n{preview}\n```\n"
                "Reload a focused chunk with load_resource using "
                f"resource_id={record.id!r}, uri={read_uri!r}, start_line=<line>, end_line=<line>."
            )

        if blob_items:
            sections.append(
                f"Omitted {len(blob_items)} binary/blob content item(s); binary MCP resource bodies are not included inline."
            )
        return "\n\n".join(sections)

    def _format_prompt_result(self, record: McpPromptRecord, payload: Any) -> str:
        description = ""
        messages: list[Any] = []
        if isinstance(payload, dict):
            raw_description = payload.get("description")
            description = raw_description if isinstance(raw_description, str) else record.description
            raw_messages = payload.get("messages")
            messages = raw_messages if isinstance(raw_messages, list) else []

        normalized_messages: list[dict[str, Any]] = []
        for message in messages:
            if not isinstance(message, dict):
                continue
            role = message.get("role")
            content = message.get("content")
            normalized_messages.append(
                {
                    "role": role if isinstance(role, str) else "unknown",
                    "content": self._normalize_prompt_content(content),
                }
            )

        structured = {
            "prompt_id": record.id,
            "source_id": record.source_id,
            "source_type": record.source_type,
            "source_name": record.source_name,
            "name": record.name,
            "description": description,
            "messages": normalized_messages,
        }
        return (
            "Loaded MCP prompt template. These messages are prompt-template content "
            "returned by the connector, not actual user/assistant chat history. Use them "
            "as guidance or a reusable workflow while preserving their intended roles/order.\n\n"
            f"```json\n{json.dumps(structured, indent=2, ensure_ascii=False)}\n```"
        )

    @staticmethod
    def _normalize_prompt_content(content: Any) -> dict[str, Any]:
        if not isinstance(content, dict):
            return {"type": "unknown"}
        content_type = content.get("type")
        if content_type == "text":
            text = content.get("text")
            return {"type": "text", "text": text if isinstance(text, str) else ""}
        if content_type in {"image", "audio"}:
            out: dict[str, Any] = {"type": content_type}
            mime_type = content.get("mime_type") or content.get("mimeType")
            if isinstance(mime_type, str):
                out["mime_type"] = mime_type
            if isinstance(content.get("data"), str):
                out["data_omitted"] = True
            return out
        if content_type == "resource":
            resource = content.get("resource")
            if isinstance(resource, dict):
                normalized_resource = {
                    key: value
                    for key, value in resource.items()
                    if key in {"uri", "mimeType", "mime_type", "text"}
                }
                if "blob" in resource:
                    normalized_resource["blob_omitted"] = True
                return {"type": "resource", "resource": normalized_resource}
            return {"type": "resource"}
        return {"type": content_type if isinstance(content_type, str) else "unknown"}
