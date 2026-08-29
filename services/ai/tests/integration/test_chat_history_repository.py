import pytest
from ulid import ULID

from db import ChatsRepository, MessagesRepository, UsersRepository

pytestmark = pytest.mark.integration


@pytest.fixture
async def history_user(db_pool):
    users = UsersRepository(pool=db_pool)
    return await users.create(
        email=f"{ULID()}@test.local",
        password_hash="not-a-real-hash",
        full_name="History User",
    )


@pytest.mark.asyncio
async def test_chat_search_scopes_results_and_supports_title_and_message_matches(
    db_pool, history_user
):
    chats = ChatsRepository(pool=db_pool)
    messages = MessagesRepository(pool=db_pool)
    other_user = await UsersRepository(pool=db_pool).create(
        email=f"{ULID()}@test.local",
        password_hash="not-a-real-hash",
        full_name="Other User",
    )

    matching_chat = await chats.create(
        history_user.id,
        title="Launch planning",
    )
    await messages.create(
        matching_chat.id,
        {
            "role": "user",
            "content": "We decided the xylophone launch should happen in March.",
        },
    )

    title_only_chat = await chats.create(
        history_user.id,
        title="Quarterly launch retrospective",
    )
    await messages.create(
        title_only_chat.id,
        {"role": "user", "content": "A short unrelated note."},
    )

    other_chat = await chats.create(other_user.id, title="Launch planning private")
    await messages.create(
        other_chat.id,
        {"role": "user", "content": "Launch details for another user."},
    )

    deleted_chat = await chats.create(history_user.id, title="Deleted launch plan")
    await messages.create(
        deleted_chat.id,
        {"role": "user", "content": "Launch details that should be hidden."},
    )
    async with db_pool.acquire() as conn:
        await conn.execute(
            "UPDATE chats SET is_deleted = TRUE WHERE id = $1", deleted_chat.id
        )

    message_hits = await chats.search(
        history_user.id,
        "xylophone",
        limit=10,
        exclude_chat_id=title_only_chat.id,
    )
    hit_ids = {hit.chat_id for hit in message_hits}

    assert matching_chat.id in hit_ids
    assert other_chat.id not in hit_ids
    assert deleted_chat.id not in hit_ids
    matching_hit = next(hit for hit in message_hits if hit.chat_id == matching_chat.id)
    assert matching_hit.source == "message"
    assert matching_hit.matched_message_id is not None
    assert "xylophone" in (matching_hit.snippet or "")

    title_hits = await chats.search(
        history_user.id,
        "quarterly retrospective",
        limit=10,
    )
    assert [hit.chat_id for hit in title_hits] == [title_only_chat.id]
    assert title_hits[0].source == "title"
    assert title_hits[0].snippet == "A short unrelated note."


@pytest.mark.asyncio
async def test_chat_search_deduplicates_messages_before_limiting(db_pool, history_user):
    chats = ChatsRepository(pool=db_pool)
    messages = MessagesRepository(pool=db_pool)
    crowded_chat = await chats.create(history_user.id, title="Crowded matches")
    for index in range(60):
        await messages.create(
            crowded_chat.id,
            {"role": "user", "content": f"needle result {index}"},
        )

    other_chat = await chats.create(history_user.id, title="Another match")
    await messages.create(
        other_chat.id,
        {"role": "user", "content": "needle result in another chat"},
    )

    hits = await chats.search(history_user.id, "needle", limit=10)

    assert crowded_chat.id in {hit.chat_id for hit in hits}
    assert other_chat.id in {hit.chat_id for hit in hits}


@pytest.mark.asyncio
async def test_get_active_path_can_read_an_older_branch_by_anchor(
    db_pool, history_user
):
    chats = ChatsRepository(pool=db_pool)
    messages = MessagesRepository(pool=db_pool)
    chat = await chats.create(history_user.id, title="Branching chat")

    root = await messages.create(
        chat.id,
        {"role": "user", "content": "Original question"},
    )
    original_reply = await messages.create(
        chat.id,
        {"role": "assistant", "content": "Original answer"},
        parent_id=root.id,
    )
    edited_reply = await messages.create(
        chat.id,
        {"role": "assistant", "content": "Edited answer"},
        parent_id=root.id,
    )
    edited_followup = await messages.create(
        chat.id,
        {"role": "user", "content": "Follow-up on edited answer"},
        parent_id=edited_reply.id,
    )

    active_path = await messages.get_active_path(chat.id)
    anchored_path = await messages.get_active_path(chat.id, original_reply.id)
    anchored_root_path = await messages.get_active_path(chat.id, root.id)

    assert [message.id for message in active_path] == [
        root.id,
        edited_reply.id,
        edited_followup.id,
    ]
    assert [message.id for message in anchored_path] == [root.id, original_reply.id]
    assert [message.id for message in anchored_root_path] == [
        root.id,
        edited_reply.id,
        edited_followup.id,
    ]
