"""SkillHandler: provides a load_skill tool for on-demand instruction loading."""

from __future__ import annotations

import asyncio
import hashlib
import json
import logging
import re
from dataclasses import dataclass
from pathlib import Path

import httpx
from anthropic.types import ToolParam

from db.skills import Skill, SkillsRepository
from tools.registry import ToolContext, ToolResult
from tools.searcher_client import (
    CapabilitiesSyncRequest,
    CapabilitySearchRequest,
    CapabilityUpsert,
    SearcherClient,
)

logger = logging.getLogger(__name__)

_TOOL_NAMES = {"skill_search", "load_skill"}
_SKILL_FILENAME = "SKILL.md"
_DEFAULT_LIMIT = 10
_MAX_LIMIT = 25
_BUILTIN_SKILLS_PUBLISHER_ID = "omni:skills"
_TOKEN_RE = re.compile(r"[a-z0-9]+")


@dataclass(frozen=True)
class ConnectorSkill:
    skill_id: str
    title: str
    description: str
    source_type: str | None = None
    source_id: str | None = None


class SkillHandler:
    """Serves skill files from a directory so the LLM can load instructions on demand.

    Skills are discovered from the preferred directory layout:

        skills/<skill_name>/SKILL.md

    For backwards compatibility, legacy flat files are also discovered:

        skills/<skill_name>.md

    If both exist for the same skill name, the directory layout wins.
    """

    _publish_lock = asyncio.Lock()
    _published_capability_keys: set[tuple[int, str]] = set()

    def __init__(
        self,
        skills_dir: Path,
        searcher_client: SearcherClient | None = None,
        connector_manager_url: str | None = None,
        skills_repository: SkillsRepository | None = None,
        skill_user_id: str | None = None,
    ) -> None:
        self._skills_dir = skills_dir
        self._searcher_client = searcher_client
        self._connector_manager_url = (
            connector_manager_url.rstrip("/") if connector_manager_url else None
        )
        self._available: dict[str, Path] = {}
        self._connector_skills: dict[str, ConnectorSkill] = {}
        self._connector_skills_loaded = False
        self._library_skills: dict[str, Skill] = {}
        self._skills_repository = skills_repository
        self._skill_user_id = skill_user_id
        self._discover_skills()

    def _discover_skills(self) -> None:
        """Populate available skills from legacy files and directory skills."""
        if not self._skills_dir.exists():
            return

        # Legacy flat-file layout: skills/excel.md
        for skill_file in sorted(self._skills_dir.glob("*.md")):
            if skill_file.is_file():
                self._available[skill_file.stem] = skill_file

        # Preferred directory layout: skills/excel/SKILL.md
        # Directory skills intentionally override legacy flat files with the same name.
        for skill_dir in sorted(self._skills_dir.iterdir()):
            if not skill_dir.is_dir():
                continue
            skill_file = skill_dir / _SKILL_FILENAME
            if skill_file.is_file():
                self._available[skill_dir.name] = skill_file

    async def refresh_connector_skills(self) -> None:
        if self._connector_skills_loaded or not self._connector_manager_url:
            return
        try:
            async with httpx.AsyncClient(timeout=10.0) as client:
                response = await client.get(f"{self._connector_manager_url}/skills")
                response.raise_for_status()
                payload = response.json()
        except Exception as e:
            logger.warning(f"Failed to load connector skills: {e}")
            self._connector_skills_loaded = True
            return

        if not isinstance(payload, dict):
            self._connector_skills_loaded = True
            return

        skills: dict[str, ConnectorSkill] = {}
        for item in payload.get("skills", []):
            if not isinstance(item, dict):
                continue
            skill_id = item.get("id")
            title = item.get("title")
            if not isinstance(skill_id, str) or not isinstance(title, str):
                continue
            description = item.get("description")
            source_type = item.get("source_type")
            source_id = item.get("source_id")
            skills[skill_id] = ConnectorSkill(
                skill_id=skill_id,
                title=title,
                description=description if isinstance(description, str) else "",
                source_type=source_type if isinstance(source_type, str) else None,
                source_id=source_id if isinstance(source_id, str) else None,
            )
        self._connector_skills = skills
        self._connector_skills_loaded = True

    async def refresh_library_skills(self, user_id: str | None = None) -> None:
        """Fetch library skills visible to the configured skill-library user."""
        if self._skills_repository is None:
            self._library_skills = {}
            return
        if user_id and self._skill_user_id is None:
            self._skill_user_id = user_id
        visibility_user_id = self._skill_user_id
        if not visibility_user_id:
            self._library_skills = {}
            return
        try:
            skills = await self._skills_repository.list_visible(visibility_user_id)
            self._library_skills = {f"library:{skill.id}": skill for skill in skills}
        except Exception as e:
            logger.warning(f"Failed to refresh library skills: {e}")
            self._library_skills = {}

    def has_skills(self) -> bool:
        return bool(self._available or self._connector_skills or self._library_skills)

    def get_tools(self) -> list[ToolParam]:
        return [
            {
                "name": "skill_search",
                "description": (
                    "Search available skills by keyword. Use this when you need "
                    "specialized instructions for a file type, connector, or task. "
                    "Call load_skill with a returned skill id to load full instructions."
                ),
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Keywords matched against skill id, title, and content.",
                        },
                        "limit": {
                            "type": "integer",
                            "description": f"Max skills to return (default {_DEFAULT_LIMIT}, max {_MAX_LIMIT}).",
                            "default": _DEFAULT_LIMIT,
                            "minimum": 1,
                            "maximum": _MAX_LIMIT,
                        },
                    },
                    "required": ["query"],
                },
            },
            {
                "name": "load_skill",
                "description": (
                    "Load full specialized instructions for an exact skill id returned "
                    "by skill_search. Call this before applying domain-specific guidance."
                ),
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "skill": {
                            "type": "string",
                            "description": "Exact skill id returned by skill_search.",
                        }
                    },
                    "required": ["skill"],
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
        await self.refresh_library_skills(context.user_id)

        if tool_name == "skill_search":
            return await self._skill_search(tool_input)
        if tool_name != "load_skill":
            return ToolResult(
                content=[{"type": "text", "text": f"Unknown skill tool: {tool_name}"}],
                is_error=True,
            )

        skill = tool_input.get("skill")
        if not isinstance(skill, str) or not skill:
            return ToolResult(
                content=[
                    {
                        "type": "text",
                        "text": "Missing required parameter: skill",
                    }
                ],
                is_error=True,
            )
        await self.refresh_connector_skills()
        path = self._available.get(skill)
        if path:
            content = path.read_text(encoding="utf-8")
            return ToolResult(content=[{"type": "text", "text": content}])

        if skill in self._connector_skills:
            return await self._load_connector_skill(skill)

        if skill in self._library_skills:
            lib_skill = self._library_skills[skill]
            return ToolResult(
                content=[{"type": "text", "text": lib_skill.instructions}]
            )

        available = ", ".join(sorted(self._all_skill_ids()))
        return ToolResult(
            content=[
                {
                    "type": "text",
                    "text": f"Unknown skill: '{skill}'. Available: {available}",
                }
            ],
            is_error=True,
        )

    async def _load_connector_skill(self, skill_id: str) -> ToolResult:
        if not self._connector_manager_url:
            return ToolResult(
                content=[
                    {"type": "text", "text": "Connector manager is not configured."}
                ],
                is_error=True,
            )
        try:
            async with httpx.AsyncClient(timeout=10.0) as client:
                response = await client.post(
                    f"{self._connector_manager_url}/skill",
                    json=self._connector_skill_request(skill_id),
                )
                response.raise_for_status()
                payload = response.json()
        except Exception as e:
            logger.warning(f"Failed to load connector skill {skill_id}: {e}")
            return ToolResult(
                content=[
                    {
                        "type": "text",
                        "text": f"Failed to load connector skill: {skill_id}",
                    }
                ],
                is_error=True,
            )
        content = payload.get("content") if isinstance(payload, dict) else None
        if not isinstance(content, str):
            return ToolResult(
                content=[
                    {
                        "type": "text",
                        "text": f"Connector skill returned no content: {skill_id}",
                    }
                ],
                is_error=True,
            )
        return ToolResult(content=[{"type": "text", "text": content}])

    async def _connector_skill_content(self, skill_id: str) -> str | None:
        if not self._connector_manager_url:
            return None
        try:
            async with httpx.AsyncClient(timeout=10.0) as client:
                response = await client.post(
                    f"{self._connector_manager_url}/skill",
                    json=self._connector_skill_request(skill_id),
                )
                response.raise_for_status()
                payload = response.json()
        except Exception as e:
            logger.warning(f"Failed to fetch connector skill content {skill_id}: {e}")
            return None
        content = payload.get("content") if isinstance(payload, dict) else None
        return content if isinstance(content, str) else None

    def _connector_skill_request(self, skill_id: str) -> dict[str, str]:
        request = {"skill_id": skill_id}
        skill = self._connector_skills.get(skill_id)
        if skill and skill.source_id:
            request["source_id"] = skill.source_id
        return request

    def _all_skill_ids(self) -> set[str]:
        return (
            set(self._available)
            | set(self._connector_skills)
            | set(self._library_skills)
        )

    async def _skill_search(self, tool_input: dict) -> ToolResult:
        await self.refresh_connector_skills()
        query = (tool_input.get("query") or "").strip()
        if not query:
            return ToolResult(
                content=[{"type": "text", "text": "Missing required parameter: query"}],
                is_error=True,
            )

        raw_limit = tool_input.get("limit", _DEFAULT_LIMIT)
        try:
            limit = max(1, min(int(raw_limit), _MAX_LIMIT))
        except (TypeError, ValueError):
            limit = _DEFAULT_LIMIT

        query_tokens = set(_TOKEN_RE.findall(query.lower()))
        if not query_tokens:
            return ToolResult(
                content=[
                    {
                        "type": "text",
                        "text": f"No searchable tokens in query: {query!r}",
                    }
                ],
                is_error=True,
            )

        matches = await self._search_skill_capabilities(query, limit)
        if not matches:
            return ToolResult(
                content=[{"type": "text", "text": f"No skills matched {query!r}."}]
            )

        lines = [f"Found {len(matches)} skill(s) matching {query!r}:"]
        for skill_id, title, snippet in matches:
            summary = f" — {title}" if title and title != skill_id else ""
            lines.append(f"- {skill_id}{summary}")
            if snippet:
                lines.append(f"  {snippet}")
        lines.append(
            "Call load_skill with the exact skill id to load full instructions."
        )
        return ToolResult(content=[{"type": "text", "text": "\n".join(lines)}])

    async def _search_skill_capabilities(
        self, query: str, limit: int
    ) -> list[tuple[str, str, str]]:
        if self._searcher_client is None:
            raise RuntimeError("skill_search requires a searcher client")

        allowed_ids = [f"skill:{skill_id}" for skill_id in self._all_skill_ids()]

        response = await self._searcher_client.search_capabilities(
            CapabilitySearchRequest(
                capability_type="skill",
                query=query,
                limit=limit,
                allowed_ids=allowed_ids,
            )
        )
        all_ids = self._all_skill_ids() | set(self._library_skills)
        matches: list[tuple[str, str, str]] = []
        for result in response.results:
            skill_id = result.data["skill_id"]
            if skill_id not in all_ids:
                continue
            title = result.data.get("title") or skill_id
            snippet_text = (
                result.data.get("description")
                or result.data.get("body")
                or ""
            )
            matches.append((skill_id, title, self._snippet(snippet_text)))
        return matches

    async def publish_skill_capabilities(self) -> None:
        await self.refresh_connector_skills()
        await self.refresh_library_skills()
        if self._searcher_client is None:
            return
        if not self._all_skill_ids() and not self._library_skills:
            return

        capabilities = await self._skill_capabilities()
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
                    publisher_id = (
                        capability.publisher_id or _BUILTIN_SKILLS_PUBLISHER_ID
                    )
                    grouped.setdefault(
                        (publisher_id, capability.capability_type), []
                    ).append(capability)
                for (publisher_id, capability_type), group in grouped.items():
                    await self._searcher_client.sync_capabilities(
                        CapabilitiesSyncRequest(
                            publisher_id=publisher_id,
                            capability_type=capability_type,
                            capabilities=group,
                        )
                    )
            except Exception as e:
                logger.warning(f"Failed to publish skill capabilities: {e}")
                return
            self._published_capability_keys.add(publish_key)

    async def _skill_capabilities(self) -> list[CapabilityUpsert]:
        capabilities: list[CapabilityUpsert] = []
        for skill_id, path in self._available.items():
            content = path.read_text()
            title = self._title(skill_id, content)
            capabilities.append(
                CapabilityUpsert(
                    id=f"skill:{skill_id}",
                    capability_type="skill",
                    name=skill_id,
                    description=self._snippet(content, max_chars=240),
                    publisher_id=_BUILTIN_SKILLS_PUBLISHER_ID,
                    search_text=f"{skill_id} {title}\n{content}",
                    data={
                        "skill_id": skill_id,
                        "title": title,
                        "description": self._snippet(content, max_chars=240),
                        "body": content,
                        "path": str(path.relative_to(self._skills_dir)),
                    },
                )
            )
        for skill_id, skill in self._connector_skills.items():
            content = await self._connector_skill_content(skill_id)
            body = content or skill.description
            capabilities.append(
                CapabilityUpsert(
                    id=f"skill:{skill_id}",
                    capability_type="skill",
                    name=skill_id,
                    description=skill.description or self._snippet(body, max_chars=240),
                    publisher_id=skill.source_id
                    or f"connector:{skill.source_type or skill_id}",
                    search_text=f"{skill_id} {skill.title}\n{body}",
                    data={
                        "skill_id": skill_id,
                        "title": skill.title,
                        "description": skill.description,
                        "body": body,
                        "source_type": skill.source_type,
                        "source_id": skill.source_id,
                    },
                )
            )
        for lib_id, lib_skill in self._library_skills.items():
            body = lib_skill.instructions
            description = lib_skill.description
            capabilities.append(
                CapabilityUpsert(
                    id=f"skill:{lib_id}",
                    capability_type="skill",
                    name=lib_id,
                    description=description,
                    publisher_id=f"omni:skill-library:{lib_skill.id}",
                    search_text=f"{lib_id} {lib_skill.name} {description}\n{body}",
                    user_id=lib_skill.owner_id,
                    data={
                        "skill_id": lib_id,
                        "title": lib_skill.name,
                        "description": description,
                        "body": body,
                        "user_id": lib_skill.owner_id,
                    },
                )
            )
        return capabilities

    def _capability_fingerprint(self, capabilities: list[CapabilityUpsert]) -> str:
        payload = [capability.model_dump() for capability in capabilities]
        payload.sort(key=lambda capability: capability["id"])
        raw = json.dumps(payload, sort_keys=True, separators=(",", ":"))
        return hashlib.sha256(raw.encode("utf-8")).hexdigest()

    @staticmethod
    def _title(skill_id: str, content: str) -> str:
        for line in content.splitlines():
            stripped = line.strip()
            if stripped.startswith("#"):
                return stripped.lstrip("#").strip() or skill_id
        return skill_id

    @staticmethod
    def _snippet(content: str, max_chars: int = 160) -> str:
        text = " ".join(line.strip() for line in content.splitlines() if line.strip())
        if len(text) <= max_chars:
            return text
        return text[: max_chars - 3].rstrip() + "..."
