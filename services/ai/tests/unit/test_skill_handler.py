from __future__ import annotations

import pytest

from tools.registry import ToolContext
from tools.searcher_client import CapabilitySearchResponse, CapabilitySearchResult
from tools.skill_handler import SkillHandler


class _FakeSearcherClient:
    def __init__(self) -> None:
        self.upserts = []
        self.searches = []

    async def upsert_capabilities(self, request):
        self.upserts.append(request)
        return type("Resp", (), {"upserted": len(request.capabilities)})()

    async def search_capabilities(self, request):
        self.searches.append(request)
        return CapabilitySearchResponse(
            results=[
                CapabilitySearchResult(
                    capability_id="skill:excel",
                    capability_type="skill",
                    item_id="excel",
                    title="Excel Skill",
                    description="Spreadsheet guidance",
                    body="Inspect spreadsheet headers and merged cells.",
                    source_id=None,
                    source_type=None,
                    metadata={},
                    score=4.2,
                )
            ]
        )


def _ctx() -> ToolContext:
    return ToolContext(chat_id="c1", user_id="u1")


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

    excel_result = await handler.execute("load_skill", {"skill": "excel"}, _ctx())
    legacy_result = await handler.execute(
        "load_skill", {"skill": "legacy_only"}, _ctx()
    )
    google_ads_result = await handler.execute(
        "load_skill", {"skill": "google_ads"}, _ctx()
    )

    assert not excel_result.is_error
    assert excel_result.content[0]["text"] == "directory excel"
    assert legacy_result.content[0]["text"] == "legacy skill"
    assert google_ads_result.content[0]["text"] == "google ads skill"


@pytest.mark.asyncio
async def test_skill_search_uses_searcher_and_publishes_skills(tmp_path):
    (tmp_path / "excel.md").write_text("# Excel Skill\n\nInspect spreadsheets.")
    skill_dir = tmp_path / "slack"
    skill_dir.mkdir()
    (skill_dir / "SKILL.md").write_text("# Slack Skill\n\nThreads.")
    searcher = _FakeSearcherClient()
    handler = SkillHandler(tmp_path, searcher_client=searcher)

    result = await handler.execute("skill_search", {"query": "spreadsheet"}, _ctx())

    assert not result.is_error
    assert "excel" in result.content[0]["text"]
    assert searcher.upserts
    assert {c.item_id for c in searcher.upserts[0].capabilities} == {
        "excel",
        "slack",
    }
    assert searcher.searches[0].capability_type == "skill"
    assert set(searcher.searches[0].allowed_item_ids) == {"excel", "slack"}


@pytest.mark.asyncio
async def test_skill_search_falls_back_to_local_search(tmp_path):
    (tmp_path / "excel.md").write_text("# Excel Skill\n\nSpreadsheet formulas.")
    handler = SkillHandler(tmp_path)

    result = await handler.execute("skill_search", {"query": "formulas"}, _ctx())

    assert not result.is_error
    assert "excel" in result.content[0]["text"]
    assert "load_skill" in result.content[0]["text"]
