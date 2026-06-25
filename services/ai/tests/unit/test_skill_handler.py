from __future__ import annotations

from pathlib import Path

import pytest
import respx
from httpx import Response

from tools.registry import ToolContext
from tools.searcher_client import CapabilitySearchResponse, CapabilitySearchResult
from tools.skill_handler import SkillHandler

SKILLS_DIR = Path(__file__).resolve().parents[2] / "skills"


class _FakeSearcherClient:
    def __init__(self, include_excel: bool = True) -> None:
        self.upserts = []
        self.searches = []
        self.include_excel = include_excel

    async def upsert_capabilities(self, request):
        self.upserts.append(request)
        return type("Resp", (), {"upserted": len(request.capabilities)})()

    async def search_capabilities(self, request):
        self.searches.append(request)
        results = []
        if self.include_excel:
            results.append(
                CapabilitySearchResult(
                    id="skill:excel",
                    capability_type="skill",
                    name="excel",
                    description="Spreadsheet guidance",
                    search_text="Excel Skill Spreadsheet guidance",
                    data={
                        "skill_id": "excel",
                        "title": "Excel Skill",
                        "description": "Spreadsheet guidance",
                        "body": "Inspect spreadsheet headers and merged cells.",
                    },
                    score=4.2,
                )
            )
        return CapabilitySearchResponse(results=results)


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
async def test_publish_skill_capabilities_uses_searcher(tmp_path):
    (tmp_path / "excel.md").write_text("# Excel Skill\n\nInspect spreadsheets.")
    skill_dir = tmp_path / "slack"
    skill_dir.mkdir()
    (skill_dir / "SKILL.md").write_text("# Slack Skill\n\nThreads.")
    searcher = _FakeSearcherClient()
    handler = SkillHandler(tmp_path, searcher_client=searcher)

    await handler.publish_skill_capabilities()

    assert searcher.upserts
    assert {c.id for c in searcher.upserts[0].capabilities} == {
        "skill:excel",
        "skill:slack",
    }


@pytest.mark.asyncio
async def test_skill_search_uses_searcher_without_republishing(tmp_path):
    (tmp_path / "excel.md").write_text("# Excel Skill\n\nInspect spreadsheets.")
    searcher = _FakeSearcherClient()
    handler = SkillHandler(tmp_path, searcher_client=searcher)

    await handler.publish_skill_capabilities()
    result = await handler.execute("skill_search", {"query": "spreadsheet"}, _ctx())
    await handler.execute("skill_search", {"query": "spreadsheet"}, _ctx())

    assert not result.is_error
    assert "excel" in result.content[0]["text"]
    assert len(searcher.upserts) == 1
    assert len(searcher.searches) == 2
    assert searcher.searches[0].capability_type == "skill"
    assert searcher.searches[0].allowed_ids == ["skill:excel"]


@pytest.mark.asyncio
async def test_skill_search_empty_searcher_results_do_not_fall_back(tmp_path):
    (tmp_path / "excel.md").write_text("# Excel Skill\n\nSpreadsheet formulas.")
    handler = SkillHandler(tmp_path, searcher_client=_FakeSearcherClient(False))

    result = await handler.execute("skill_search", {"query": "formulas"}, _ctx())

    assert not result.is_error
    assert "No skills matched" in result.content[0]["text"]


def test_google_workspace_skills_are_not_local_ai_skills() -> None:
    handler = SkillHandler(SKILLS_DIR)

    assert "google-drive" not in handler._available
    assert "gmail" not in handler._available


@pytest.mark.asyncio
@respx.mock
async def test_connector_skill_loads() -> None:
    respx.get("http://cm.test/skills").mock(
        return_value=Response(
            200,
            json={
                "skills": [
                    {
                        "id": "google-drive",
                        "title": "Google Drive Skill",
                        "description": "Drive guidance",
                        "source_type": "google_drive",
                    }
                ]
            },
        )
    )
    respx.post("http://cm.test/skill").mock(
        return_value=Response(
            200,
            json={
                "skill_id": "google-drive",
                "title": "Google Drive Skill",
                "content": "# Google Drive Skill\n\nUse connector tools.",
            },
        )
    )
    handler = SkillHandler(SKILLS_DIR, connector_manager_url="http://cm.test")

    result = await handler.execute(
        "load_skill",
        {"skill": "google-drive"},
        ToolContext(chat_id="chat-1", user_id="user-1"),
    )

    assert result.is_error is False
    text = result.content[0]["text"]
    assert "Google Drive Skill" in text
    assert "connector tools" in text


def test_google_workspace_connector_skills_do_not_instruct_local_gws_auth_or_install() -> (
    None
):
    forbidden = [
        "gws auth login",
        "gws auth setup",
        "cargo install",
        "npm install",
    ]
    connector_skills_dir = SKILLS_DIR.parents[2] / "connectors" / "google" / "skills"

    for name in ["google-drive.md", "gmail.md"]:
        text = (connector_skills_dir / name).read_text()
        for phrase in forbidden:
            assert phrase not in text
        assert "Do not run local `gws` commands" in text
