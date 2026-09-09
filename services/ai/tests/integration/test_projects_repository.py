"""Integration tests for omni-ai's read-only view of projects.

Projects and their attachments are written by omni-web; the CRUD coverage
lives in web/src/lib/server/db/projects.test.ts. Here we create rows via SQL
and assert the read paths used by the chat stream: live-project lookup,
deleted-project filtering, and project-scoped chat search.
"""

import asyncpg
import pytest
from ulid import ULID

from db import ChatsRepository, ProjectAttachmentsRepository, ProjectsRepository, UsersRepository

pytestmark = pytest.mark.integration


@pytest.fixture
async def project_user(db_pool):
    return await UsersRepository(pool=db_pool).create(
        email=f"{ULID()}@test.local",
        password_hash="not-a-real-hash",
        full_name="Project User",
    )


async def _insert_project(db_pool, user_id: str, name: str) -> str:
    project_id = str(ULID())
    async with db_pool.acquire() as conn:
        await conn.execute(
            """
            INSERT INTO projects (id, user_id, name, description, instructions)
            VALUES ($1, $2, $3, $4, $5)
            """,
            project_id,
            user_id,
            name,
            "Project description",
            "Always answer in German.",
        )
    return project_id


async def _insert_document_attachment(db_pool, project_id: str) -> str:
    attachment_id = str(ULID())
    async with db_pool.acquire() as conn:
        await conn.execute(
            """
            INSERT INTO project_attachments
                (id, project_id, attachment_type, upload_id, document_id)
            VALUES ($1, $2, 'document', NULL, $3)
            """,
            attachment_id,
            project_id,
            str(ULID()),
        )
    return attachment_id


@pytest.mark.asyncio
async def test_get_returns_live_project_and_hides_deleted(db_pool, project_user):
    projects = ProjectsRepository(pool=db_pool)

    project_id = await _insert_project(db_pool, project_user.id, "Visible")
    fetched = await projects.get(project_id)
    assert fetched is not None
    assert fetched.name == "Visible"
    assert fetched.instructions == "Always answer in German."
    assert fetched.is_archived is False

    async with db_pool.acquire() as conn:
        await conn.execute("UPDATE projects SET is_deleted = TRUE WHERE id = $1", project_id)
    assert await projects.get(project_id) is None


@pytest.mark.asyncio
async def test_list_attachments_joins_live_projects_only(db_pool, project_user):
    attachments = ProjectAttachmentsRepository(pool=db_pool)

    project_id = await _insert_project(db_pool, project_user.id, "With docs")
    first = await _insert_document_attachment(db_pool, project_id)
    second = await _insert_document_attachment(db_pool, project_id)

    listed = await attachments.list_for_project(project_id)
    assert {a.id for a in listed} == {first, second}
    assert all(a.attachment_type.value == "document" for a in listed)

    # Deleted projects yield no attachments.
    async with db_pool.acquire() as conn:
        await conn.execute("UPDATE projects SET is_deleted = TRUE WHERE id = $1", project_id)
    assert await attachments.list_for_project(project_id) == []

    # The DB shape check still rejects mixed payloads (defense in depth).
    with pytest.raises(asyncpg.CheckViolationError):
        async with db_pool.acquire() as conn:
            await conn.execute(
                """
                INSERT INTO project_attachments
                    (id, project_id, attachment_type, upload_id, document_id)
                VALUES ($1, $2, 'document', $3, NULL)
                """,
                str(ULID()),
                project_id,
                first,
            )


@pytest.mark.asyncio
async def test_chat_search_scopes_to_project(db_pool, project_user):
    projects = ProjectsRepository(pool=db_pool)
    chats = ChatsRepository(pool=db_pool)

    project = await projects.get(
        await _insert_project(db_pool, project_user.id, "Scoped")
    )
    in_project = await chats.create(
        project_user.id, title="Zebra roadmap", project_id=project.id
    )
    outside = await chats.create(project_user.id, title="Zebra outside")
    other_project = await projects.get(
        await _insert_project(db_pool, project_user.id, "Other scope")
    )
    other_project_chat = await chats.create(
        project_user.id, title="Zebra elsewhere", project_id=other_project.id
    )

    hits = await chats.search(project_user.id, "zebra", limit=10, project_id=project.id)
    hit_ids = {hit.chat_id for hit in hits}
    assert hit_ids == {in_project.id}

    unscoped = await chats.search(project_user.id, "zebra", limit=10)
    assert {hit.chat_id for hit in unscoped} >= {
        in_project.id,
        outside.id,
        other_project_chat.id,
    }
