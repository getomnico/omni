"""Integration tests for AI read-only SkillsRepository."""

from __future__ import annotations

import pytest
from ulid import ULID

from db.skills import SkillsRepository
from tests.helpers import create_test_user

pytestmark = pytest.mark.integration


@pytest.mark.asyncio
async def test_list_visible_returns_own_private_and_public_skills(db_pool):
    user1, _ = await create_test_user(db_pool)
    user2, _ = await create_test_user(db_pool)

    # Seed skills directly (web service normally owns writes)
    async with db_pool.acquire() as conn:
        # user1 private skill
        p1 = str(ULID())
        await conn.execute(
            """INSERT INTO skills (id, owner_id, name, description, instructions, visibility)
               VALUES ($1, $2, 'User1 Private', 'Only user1', 'Only user1', 'private')""",
            p1, user1,
        )
        # user1 public skill
        pub1 = str(ULID())
        await conn.execute(
            """INSERT INTO skills (id, owner_id, name, description, instructions, visibility)
               VALUES ($1, $2, 'User1 Public', 'Everyone sees this', 'Everyone sees this', 'public')""",
            pub1, user1,
        )
        # user2 public skill
        pub2 = str(ULID())
        await conn.execute(
            """INSERT INTO skills (id, owner_id, name, description, instructions, visibility)
               VALUES ($1, $2, 'User2 Public', 'Also public', 'Also public', 'public')""",
            pub2, user2,
        )
        # user2 private skill
        p2 = str(ULID())
        await conn.execute(
            """INSERT INTO skills (id, owner_id, name, description, instructions, visibility)
               VALUES ($1, $2, 'User2 Private', 'Only user2', 'Only user2', 'private')""",
            p2, user2,
        )

    repo = SkillsRepository(pool=db_pool)

    # user1 sees: own private, own public, user2's public (3 skills)
    visible1 = await repo.list_visible(user1)
    names1 = sorted(s.name for s in visible1)
    assert names1 == ['User1 Private', 'User1 Public', 'User2 Public']
    descriptions1 = {skill.name: skill.description for skill in visible1}
    assert descriptions1["User1 Private"] == "Only user1"
    assert descriptions1["User2 Public"] == "Also public"

    # user2 sees: own private, own public, user1's public (3 skills)
    visible2 = await repo.list_visible(user2)
    names2 = sorted(s.name for s in visible2)
    assert names2 == ['User2 Private', 'User2 Public', 'User1 Public']


@pytest.mark.asyncio
async def test_get_visible_by_id_respects_visibility(db_pool):
    user1, _ = await create_test_user(db_pool)
    user2, _ = await create_test_user(db_pool)

    async with db_pool.acquire() as conn:
        private_id = str(ULID())
        await conn.execute(
            """INSERT INTO skills (id, owner_id, name, description, instructions, visibility)
               VALUES ($1, $2, 'Secret', 'Shhh', 'Shhh', 'private')""",
            private_id, user1,
        )
        public_id = str(ULID())
        await conn.execute(
            """INSERT INTO skills (id, owner_id, name, description, instructions, visibility)
               VALUES ($1, $2, 'Open', 'Available', 'Available', 'public')""",
            public_id, user1,
        )

    repo = SkillsRepository(pool=db_pool)

    # Owner can see both
    private_skill = await repo.get_visible_by_id(private_id, user1)
    public_skill = await repo.get_visible_by_id(public_id, user1)
    assert private_skill is not None
    assert private_skill.description == "Shhh"
    assert public_skill is not None
    assert public_skill.description == "Available"

    # Other user can only see public
    assert await repo.get_visible_by_id(private_id, user2) is None
    assert await repo.get_visible_by_id(public_id, user2) is not None


@pytest.mark.asyncio
async def test_get_visible_by_id_returns_none_for_nonexistent(db_pool):
    user1, _ = await create_test_user(db_pool)
    repo = SkillsRepository(pool=db_pool)

    result = await repo.get_visible_by_id(str(ULID()), user1)
    assert result is None
