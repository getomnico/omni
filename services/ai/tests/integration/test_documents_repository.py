"""Integration tests for DocumentsRepository.get_by_id permission filtering.

Verifies that the BM25 permission filter in get_by_id correctly gates
document access based on public/users/groups, using a real ParadeDB instance.
"""

import pytest

from db.documents import DocumentsRepository
from tests.helpers import create_test_user, create_test_source, create_test_document

pytestmark = pytest.mark.integration


@pytest.fixture
async def user_id(db_pool) -> str:
    uid, _ = await create_test_user(db_pool)
    return uid


@pytest.fixture
def repo(db_pool) -> DocumentsRepository:
    return DocumentsRepository(db_pool)


async def _doc(db_pool, user_id: str, permissions: dict) -> str:
    source_id = await create_test_source(db_pool, user_id, "google_drive")
    return await create_test_document(
        db_pool,
        source_id,
        "Test Doc",
        "content",
        permissions=permissions,
    )


class TestGetByIdPermissionFilter:
    @pytest.mark.asyncio
    async def test_no_email_returns_doc_without_filtering(self, db_pool, user_id, repo):
        doc_id = await _doc(
            db_pool, user_id, {"public": False, "users": ["alice@co.com"], "groups": []}
        )
        doc = await repo.get_by_id(doc_id)
        assert doc is not None
        assert doc.id == doc_id

    @pytest.mark.asyncio
    async def test_public_doc_accessible_to_anyone(self, db_pool, user_id, repo):
        doc_id = await _doc(
            db_pool, user_id, {"public": True, "users": [], "groups": []}
        )
        assert await repo.get_by_id(doc_id, user_email="anyone@co.com") is not None

    @pytest.mark.asyncio
    async def test_user_in_users_list(self, db_pool, user_id, repo):
        doc_id = await _doc(
            db_pool, user_id, {"public": False, "users": ["alice@co.com"], "groups": []}
        )
        assert await repo.get_by_id(doc_id, user_email="alice@co.com") is not None

    @pytest.mark.asyncio
    async def test_user_not_in_users_list(self, db_pool, user_id, repo):
        doc_id = await _doc(
            db_pool, user_id, {"public": False, "users": ["alice@co.com"], "groups": []}
        )
        assert await repo.get_by_id(doc_id, user_email="bob@co.com") is None

    @pytest.mark.asyncio
    async def test_user_in_groups_list(self, db_pool, user_id, repo):
        doc_id = await _doc(
            db_pool, user_id, {"public": False, "users": [], "groups": ["eng@co.com"]}
        )
        assert await repo.get_by_id(doc_id, user_email="eng@co.com") is not None

    @pytest.mark.asyncio
    async def test_user_not_in_groups_list(self, db_pool, user_id, repo):
        doc_id = await _doc(
            db_pool, user_id, {"public": False, "users": [], "groups": ["eng@co.com"]}
        )
        assert await repo.get_by_id(doc_id, user_email="sales@co.com") is None

    @pytest.mark.asyncio
    async def test_nonexistent_doc_returns_none(self, repo):
        assert await repo.get_by_id("nonexistent", user_email="alice@co.com") is None
