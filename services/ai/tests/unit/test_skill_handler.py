from __future__ import annotations

import pytest

from tools.registry import ToolContext
from tools.skill_handler import SkillHandler


@pytest.mark.asyncio
async def test_skill_handler_discovers_directory_skills_and_legacy_files(tmp_path):
    skills_dir = tmp_path / "skills"
    skills_dir.mkdir()

    (skills_dir / "legacy_only.md").write_text("legacy skill", encoding="utf-8")
    (skills_dir / "excel.md").write_text("legacy excel", encoding="utf-8")

    excel_dir = skills_dir / "excel"
    excel_dir.mkdir()
    (excel_dir / "SKILL.md").write_text("directory excel", encoding="utf-8")

    google_ads_dir = skills_dir / "google_ads"
    google_ads_dir.mkdir()
    (google_ads_dir / "SKILL.md").write_text("google ads skill", encoding="utf-8")

    handler = SkillHandler(skills_dir)

    assert sorted(handler._available) == ["excel", "google_ads", "legacy_only"]
    assert handler._available["excel"] == excel_dir / "SKILL.md"

    context = ToolContext(chat_id="chat", user_id="user")
    excel_result = await handler.execute("load_skill", {"skill": "excel"}, context)
    legacy_result = await handler.execute("load_skill", {"skill": "legacy_only"}, context)
    google_ads_result = await handler.execute("load_skill", {"skill": "google_ads"}, context)

    assert not excel_result.is_error
    assert excel_result.content[0]["text"] == "directory excel"
    assert legacy_result.content[0]["text"] == "legacy skill"
    assert google_ads_result.content[0]["text"] == "google ads skill"


def test_skill_handler_tool_description_lists_directory_skills(tmp_path):
    skills_dir = tmp_path / "skills"
    (skills_dir / "google_ads").mkdir(parents=True)
    (skills_dir / "google_ads" / "SKILL.md").write_text(
        "google ads skill", encoding="utf-8"
    )

    handler = SkillHandler(skills_dir)
    tool = handler.get_tools()[0]

    assert tool["name"] == "load_skill"
    assert "google_ads" in tool["description"]
    assert "google_ads" in tool["input_schema"]["properties"]["skill"]["description"]
