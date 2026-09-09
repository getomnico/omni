"""Read-only access to projects for the chat stream.

Projects and their attachments are written by omni-web (drizzle) — the
established pattern for user-owned resources (chats, agents, skills).
omni-ai only reads them to enrich chat system prompts.
"""

import logging
from typing import Optional

from asyncpg import Pool

from .connection import get_db_pool
from .models import Project, ProjectAttachment

logger = logging.getLogger(__name__)

_PROJECT_COLUMNS = (
    "id, user_id, name, description, instructions, is_archived, created_at, updated_at"
)


class ProjectsRepository:
    def __init__(self, pool: Optional[Pool] = None):
        self.pool = pool

    async def _get_pool(self) -> Pool:
        if self.pool:
            return self.pool
        return await get_db_pool()

    async def get(self, project_id: str) -> Optional[Project]:
        pool = await self._get_pool()
        row = await pool.fetchrow(
            f"SELECT {_PROJECT_COLUMNS} FROM projects WHERE id = $1 AND is_deleted = FALSE",
            project_id,
        )
        return Project.from_row(dict(row)) if row else None


class ProjectAttachmentsRepository:
    def __init__(self, pool: Optional[Pool] = None):
        self.pool = pool

    async def _get_pool(self) -> Pool:
        if self.pool:
            return self.pool
        return await get_db_pool()

    async def list_for_project(self, project_id: str) -> list[ProjectAttachment]:
        pool = await self._get_pool()
        rows = await pool.fetch(
            """
            SELECT pa.id, pa.project_id, pa.attachment_type, pa.upload_id,
                   pa.document_id, pa.added_at
            FROM project_attachments pa
            JOIN projects p ON p.id = pa.project_id
            WHERE pa.project_id = $1 AND p.is_deleted = FALSE
            ORDER BY pa.added_at ASC
            """,
            project_id,
        )
        return [ProjectAttachment.from_row(dict(row)) for row in rows]
