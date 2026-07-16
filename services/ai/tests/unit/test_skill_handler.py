from __future__ import annotations

import datetime
from dataclasses import dataclass
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

    async def sync_capabilities(self, request):
        self.upserts.append(request)
        return type("Resp", (), {"upserted": len(request.capabilities), "deleted": 0})()

    async def search_capabilities(self, request):
        self.searches.append(request)
        results = []
        query = request.query.lower()
        allowed_ids = set(request.allowed_ids or [])
        if self.include_excel and (not allowed_ids or "skill:excel" in allowed_ids):
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
        for upsert in self.upserts:
            for capability in upsert.capabilities:
                if allowed_ids and capability.id not in allowed_ids:
                    continue
                searchable = f"{capability.id} {capability.name} {capability.search_text}".lower()
                if query not in searchable:
                    continue
                results.append(
                    CapabilitySearchResult(
                        id=capability.id,
                        capability_type=capability.capability_type,
                        name=capability.name,
                        description=capability.description,
                        search_text=capability.search_text,
                        data=capability.data,
                        score=3.0,
                    )
                )
        return CapabilitySearchResponse(results=results[: request.limit])


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


# =============================================================================
# Library skill tests
# =============================================================================


@dataclass
class _FakeSkill:
    """Minimal stand-in matching the fields SkillHandler reads from Skill."""

    id: str
    owner_id: str
    name: str
    description: str
    instructions: str
    visibility: str
    created_at: datetime.datetime
    updated_at: datetime.datetime


class _FakeSkillsRepository:
    """In-memory SkillsRepository that returns pre-configured library skill records."""

    def __init__(self, skills: list | None = None):
        self.skills = skills or []

    async def list_visible(self, user_id: str) -> list:
        return [
            s for s in self.skills
            if s.owner_id == user_id or s.visibility == "public"
        ]

    async def get_visible_by_id(self, skill_id: str, user_id: str):
        for s in self.skills:
            if s.id == skill_id and (s.owner_id == user_id or s.visibility == "public"):
                return s
        return None


@pytest.mark.asyncio
async def test_library_skill_loads_via_namespaced_id(tmp_path):
    skills_dir = tmp_path / "skills"
    skills_dir.mkdir()
    searcher = _FakeSearcherClient()

    skill_id = "01J00000000000000000000000"
    assert len(skill_id) == 26
    lib_skill = _FakeSkill(
        id=skill_id,
        owner_id="user-1",
        name="My Library Skill",
        description="Do the thing quickly.",
        instructions="# Library Skill\n\nDo the thing.",
        visibility="public",
        created_at=datetime.datetime.now(),
        updated_at=datetime.datetime.now(),
    )
    fake_repo = _FakeSkillsRepository(skills=[lib_skill])

    handler = SkillHandler(
        skills_dir,
        searcher_client=searcher,
        skills_repository=fake_repo,
    )

    result = await handler.execute(
        "load_skill",
        {"skill": f"library:{skill_id}"},
        ToolContext(chat_id="c1", user_id="user-1"),
    )

    assert not result.is_error
    assert result.content[0]["text"] == "# Library Skill\n\nDo the thing."


@pytest.mark.asyncio
async def test_library_skill_not_loadable_by_non_owner_when_private(tmp_path):
    skills_dir = tmp_path / "skills"
    skills_dir.mkdir()
    searcher = _FakeSearcherClient()

    lib_skill = _FakeSkill(
        id="01J00000000000000000000001",
        owner_id="user-1",
        name="Private Skill",
        description="Secret description.",
        instructions="Secret instructions",
        visibility="private",
        created_at=datetime.datetime.now(),
        updated_at=datetime.datetime.now(),
    )
    fake_repo = _FakeSkillsRepository(skills=[lib_skill])

    handler = SkillHandler(
        skills_dir,
        searcher_client=searcher,
        skills_repository=fake_repo,
    )

    # Non-owner user-2 tries to load user-1's private skill
    result = await handler.execute(
        "load_skill",
        {"skill": "library:01J00000000000000000000001"},
        ToolContext(chat_id="c1", user_id="user-2"),
    )

    assert result.is_error
    assert "Unknown skill" in result.content[0]["text"]


@pytest.mark.asyncio
async def test_library_skills_included_in_search_allowed_ids(tmp_path):
    skills_dir = tmp_path / "skills"
    skills_dir.mkdir()
    (skills_dir / "excel.md").write_text("# Excel Skill\n\nInspect spreadsheets.")

    lib_skill = _FakeSkill(
        id="01J00000000000000000000002",
        owner_id="user-1",
        name="Public Library Skill",
        description="Public library description.",
        instructions="Public library instructions",
        visibility="public",
        created_at=datetime.datetime.now(),
        updated_at=datetime.datetime.now(),
    )
    fake_repo = _FakeSkillsRepository(skills=[lib_skill])

    searcher = _FakeSearcherClient()
    handler = SkillHandler(
        skills_dir,
        searcher_client=searcher,
        skills_repository=fake_repo,
    )

    await handler.execute(
        "skill_search",
        {"query": "spreadsheet"},
        ToolContext(chat_id="c1", user_id="user-1"),
    )

    # The library skills should be in the allowed_ids
    assert len(searcher.searches) >= 1
    allowed_ids = searcher.searches[0].allowed_ids
    assert "skill:library:01J00000000000000000000002" in allowed_ids


@pytest.mark.asyncio
async def test_library_skills_published_as_capabilities(tmp_path):
    skills_dir = tmp_path / "skills"
    skills_dir.mkdir()
    searcher = _FakeSearcherClient()

    lib_skill = _FakeSkill(
        id="01J00000000000000000000003",
        owner_id="user-1",
        name="Lib Skill",
        description="Lib description.",
        instructions="Lib instructions",
        visibility="public",
        created_at=datetime.datetime.now(),
        updated_at=datetime.datetime.now(),
    )
    fake_repo = _FakeSkillsRepository(skills=[lib_skill])

    handler = SkillHandler(
        skills_dir,
        searcher_client=searcher,
        skills_repository=fake_repo,
    )

    await handler.refresh_library_skills("user-1")
    await handler.publish_skill_capabilities()

    assert searcher.upserts
    capability_ids = {c.id for c in searcher.upserts[0].capabilities}
    assert "skill:library:01J00000000000000000000003" in capability_ids


@pytest.mark.asyncio
async def test_library_skill_search_returns_exact_library_id(tmp_path):
    skills_dir = tmp_path / "skills"
    skills_dir.mkdir()
    searcher = _FakeSearcherClient(include_excel=False)
    skill_id = "01J00000000000000000000004"
    fake_repo = _FakeSkillsRepository(
        skills=[
            _FakeSkill(
                id=skill_id,
                owner_id="user-1",
                name="PR Review",
                description="Review pull requests with a checklist.",
                instructions="Review pull requests with a checklist.",
                visibility="public",
                created_at=datetime.datetime.now(),
                updated_at=datetime.datetime.now(),
            )
        ]
    )
    handler = SkillHandler(
        skills_dir,
        searcher_client=searcher,
        skills_repository=fake_repo,
        skill_user_id="user-1",
    )

    await handler.publish_skill_capabilities()
    result = await handler.execute(
        "skill_search",
        {"query": f"library:{skill_id}"},
        ToolContext(chat_id="c1", user_id="user-1"),
    )

    assert not result.is_error
    assert f"library:{skill_id} — PR Review" in result.content[0]["text"]


@pytest.mark.asyncio
async def test_library_skill_uses_configured_owner_when_context_has_no_user(tmp_path):
    skills_dir = tmp_path / "skills"
    skills_dir.mkdir()
    skill_id = "01J00000000000000000000005"
    fake_repo = _FakeSkillsRepository(
        skills=[
            _FakeSkill(
                id=skill_id,
                owner_id="agent-owner",
                name="Owner Private Skill",
                description="Private owner description.",
                instructions="Private owner instructions.",
                visibility="private",
                created_at=datetime.datetime.now(),
                updated_at=datetime.datetime.now(),
            )
        ]
    )
    handler = SkillHandler(
        skills_dir,
        searcher_client=_FakeSearcherClient(include_excel=False),
        skills_repository=fake_repo,
        skill_user_id="agent-owner",
    )

    result = await handler.execute(
        "load_skill",
        {"skill": f"library:{skill_id}"},
        ToolContext(chat_id="agent-run", user_id=None, skip_permission_check=True),
    )

    assert not result.is_error
    assert result.content[0]["text"] == "Private owner instructions."


@pytest.mark.asyncio
async def test_library_skill_capability_uses_explicit_description(tmp_path):
    skills_dir = tmp_path / "skills"
    skills_dir.mkdir()
    searcher = _FakeSearcherClient(include_excel=False)
    skill_id = "01J00000000000000000000006"
    lib_skill = _FakeSkill(
        id=skill_id,
        owner_id="user-1",
        name="Explicit Desc Skill",
        description="Explicit short description for this skill.",
        instructions="# Detailed Instructions\n\nStep-by-step guide.",
        visibility="public",
        created_at=datetime.datetime.now(),
        updated_at=datetime.datetime.now(),
    )
    fake_repo = _FakeSkillsRepository(skills=[lib_skill])
    handler = SkillHandler(
        skills_dir,
        searcher_client=searcher,
        skills_repository=fake_repo,
        skill_user_id="user-1",
    )

    await handler.publish_skill_capabilities()

    assert searcher.upserts
    cap = next(c for c in searcher.upserts[0].capabilities if c.id == f"skill:library:{skill_id}")
    assert cap.description == "Explicit short description for this skill."
    assert cap.data["description"] == "Explicit short description for this skill."
    assert "Explicit short description for this skill." in cap.search_text
    assert "# Detailed Instructions" not in cap.description


@pytest.mark.asyncio
async def test_library_skill_search_snippet_prefers_description(tmp_path):
    skills_dir = tmp_path / "skills"
    skills_dir.mkdir()
    searcher = _FakeSearcherClient(include_excel=False)
    skill_id = "01J00000000000000000000007"
    lib_skill = _FakeSkill(
        id=skill_id,
        owner_id="user-1",
        name="Snippet Skill",
        description="Snippet description content.",
        instructions="Long body content that should not appear in snippet when description is available.",
        visibility="public",
        created_at=datetime.datetime.now(),
        updated_at=datetime.datetime.now(),
    )
    fake_repo = _FakeSkillsRepository(skills=[lib_skill])
    handler = SkillHandler(
        skills_dir,
        searcher_client=searcher,
        skills_repository=fake_repo,
        skill_user_id="user-1",
    )

    await handler.publish_skill_capabilities()
    result = await handler.execute(
        "skill_search",
        {"query": "snippet"},
        ToolContext(chat_id="c1", user_id="user-1"),
    )

    assert not result.is_error
    text = result.content[0]["text"]
    assert "Snippet description content." in text
    assert "Long body content that should not appear" not in text
