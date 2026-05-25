"""SkillHandler: provides a load_skill tool for on-demand instruction loading."""

from __future__ import annotations

import logging
import re
from pathlib import Path

from anthropic.types import ToolParam

from tools.registry import ToolContext, ToolResult
from tools.searcher_client import (
    CapabilitiesUpsertRequest,
    CapabilitySearchRequest,
    CapabilityUpsert,
    SearcherClient,
)

logger = logging.getLogger(__name__)

_TOOL_NAMES = {"skill_search", "load_skill"}
_SKILL_FILENAME = "SKILL.md"
_DEFAULT_LIMIT = 10
_MAX_LIMIT = 25
_TOKEN_RE = re.compile(r"[a-z0-9]+")


class SkillHandler:
    """Serves skill files from a directory so the LLM can load instructions on demand.

    Skills are discovered from the preferred directory layout:

        skills/<skill_name>/SKILL.md

    For backwards compatibility, legacy flat files are also discovered:

        skills/<skill_name>.md

    If both exist for the same skill name, the directory layout wins.
    """

    def __init__(
        self, skills_dir: Path, searcher_client: SearcherClient | None = None
    ) -> None:
        self._skills_dir = skills_dir
        self._searcher_client = searcher_client
        self._available: dict[str, Path] = {}
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
        if tool_name == "skill_search":
            return await self._skill_search(tool_input)
        if tool_name != "load_skill":
            return ToolResult(
                content=[{"type": "text", "text": f"Unknown skill tool: {tool_name}"}],
                is_error=True,
            )

        skill = tool_input.get("skill")
        if not skill:
            return ToolResult(
                content=[
                    {
                        "type": "text",
                        "text": "Missing required parameter: skill",
                    }
                ],
                is_error=True,
            )
        path = self._available.get(skill)
        if not path:
            available = ", ".join(sorted(self._available.keys()))
            return ToolResult(
                content=[
                    {
                        "type": "text",
                        "text": f"Unknown skill: '{skill}'. Available: {available}",
                    }
                ],
                is_error=True,
            )
        content = path.read_text(encoding="utf-8")
        return ToolResult(content=[{"type": "text", "text": content}])

    async def _skill_search(self, tool_input: dict) -> ToolResult:
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

        matches = await self._search_skill_capabilities(query, limit, query_tokens)
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
        self, query: str, limit: int, query_tokens: set[str]
    ) -> list[tuple[str, str, str]]:
        if self._searcher_client is not None:
            try:
                await self._publish_skill_capabilities()
                response = await self._searcher_client.search_capabilities(
                    CapabilitySearchRequest(
                        capability_type="skill",
                        query=query,
                        limit=limit,
                        allowed_ids=[
                            f"skill:{skill_id}" for skill_id in self._available
                        ],
                    )
                )
                matches: list[tuple[str, str, str]] = []
                for result in response.results:
                    skill_id = result.data["skill_id"]
                    if skill_id not in self._available:
                        continue
                    title = result.data.get("title") or skill_id
                    body = (
                        result.data.get("body") or result.data.get("description") or ""
                    )
                    matches.append((skill_id, title, self._snippet(body)))
                if matches:
                    return matches
            except Exception as e:
                logger.warning(
                    f"Capability skill search failed; using local fallback: {e}"
                )

        return self._local_skill_search(query_tokens, limit)

    async def _publish_skill_capabilities(self) -> None:
        if self._searcher_client is None or not self._available:
            return

        capabilities: list[CapabilityUpsert] = []
        for skill_id, path in self._available.items():
            content = path.read_text()
            title = self._title(skill_id, content)
            capabilities.append(
                CapabilityUpsert(
                    id=f"skill:{skill_id}",
                    capability_type="skill",
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
        await self._searcher_client.upsert_capabilities(
            CapabilitiesUpsertRequest(capabilities=capabilities)
        )

    def _local_skill_search(
        self, query_tokens: set[str], limit: int
    ) -> list[tuple[str, str, str]]:
        scored: list[tuple[int, str, str, str]] = []
        for skill_id, path in self._available.items():
            content = path.read_text()
            title = self._title(skill_id, content)
            id_tokens = set(_TOKEN_RE.findall(skill_id.lower()))
            title_tokens = set(_TOKEN_RE.findall(title.lower()))
            body_tokens = set(_TOKEN_RE.findall(content.lower()))
            score = (
                4 * len(query_tokens & id_tokens)
                + 3 * len(query_tokens & title_tokens)
                + len(query_tokens & body_tokens)
            )
            if score > 0:
                scored.append((score, skill_id, title, self._snippet(content)))
        scored.sort(key=lambda x: (-x[0], x[1]))
        return [
            (skill_id, title, snippet) for _, skill_id, title, snippet in scored[:limit]
        ]

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
