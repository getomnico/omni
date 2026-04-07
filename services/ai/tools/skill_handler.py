"""SkillHandler: provides a load_skill tool for on-demand instruction loading."""

from __future__ import annotations

import logging
from pathlib import Path

from tools.registry import ToolContext, ToolResult

logger = logging.getLogger(__name__)

_TOOL_NAMES = {"load_skill"}


class SkillHandler:
    """Serves skill files from a directory so the LLM can load instructions on demand."""

    def __init__(self, skills_dir: Path) -> None:
        self._skills_dir = skills_dir
        self._available: dict[str, Path] = {}
        if skills_dir.exists():
            for f in skills_dir.glob("*.md"):
                self._available[f.stem] = f

    def get_tools(self) -> list[dict]:
        skill_names = ", ".join(sorted(self._available.keys()))
        return [
            {
                "name": "load_skill",
                "description": (
                    f"Load specialized instructions for a domain. Available skills: {skill_names}. "
                    "Call this when you need detailed guidance for working with a specific file type or task."
                ),
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "skill": {
                            "type": "string",
                            "description": f"Skill to load. One of: {skill_names}",
                        }
                    },
                    "required": ["skill"],
                },
            }
        ]

    def can_handle(self, tool_name: str) -> bool:
        return tool_name in _TOOL_NAMES

    def requires_approval(self, tool_name: str) -> bool:
        return False

    async def execute(
        self, tool_name: str, tool_input: dict, context: ToolContext
    ) -> ToolResult:
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
        content = path.read_text()
        return ToolResult(content=[{"type": "text", "text": content}])
