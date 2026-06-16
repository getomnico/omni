"""SkillHandler: provides a load_skill tool for on-demand instruction loading."""

from __future__ import annotations

import logging
from pathlib import Path

from anthropic.types import ToolParam

from tools.registry import ToolContext, ToolResult

logger = logging.getLogger(__name__)

_TOOL_NAMES = {"load_skill"}
_SKILL_FILENAME = "SKILL.md"


class SkillHandler:
    """Serves skill files from a directory so the LLM can load instructions on demand.

    Skills are discovered from the preferred directory layout:

        skills/<skill_name>/SKILL.md

    For backwards compatibility, legacy flat files are also discovered:

        skills/<skill_name>.md

    If both exist for the same skill name, the directory layout wins.
    """

    def __init__(self, skills_dir: Path) -> None:
        self._skills_dir = skills_dir
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
        content = path.read_text(encoding="utf-8")
        return ToolResult(content=[{"type": "text", "text": content}])
