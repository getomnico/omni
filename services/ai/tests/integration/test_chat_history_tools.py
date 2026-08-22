import pytest
from ulid import ULID

from db import ChatsRepository, MessagesRepository, UsersRepository
from tools.chat_history_handler import ChatHistoryToolHandler
from tools.registry import ToolContext

pytestmark = pytest.mark.integration


@pytest.mark.asyncio
async def test_chat_history_tools_search_and_read_user_chat(db_pool):
    user = await UsersRepository(pool=db_pool).create(
        email=f"{ULID()}@test.local",
        password_hash="not-a-real-hash",
        full_name="Tool User",
    )
    chats = ChatsRepository(pool=db_pool)
    messages = MessagesRepository(pool=db_pool)
    previous_chat = await chats.create(user.id, title="Product decision")
    matching_message = await messages.create(
        previous_chat.id,
        {"role": "user", "content": "The product decision is to keep the legacy API."},
    )
    await messages.create(
        previous_chat.id,
        {"role": "assistant", "content": "Understood; I will keep that context."},
        parent_id=matching_message.id,
    )
    current_chat = await chats.create(user.id, title="Current conversation")

    handler = ChatHistoryToolHandler(chats_repo=chats, messages_repo=messages)
    context = ToolContext(chat_id=current_chat.id, user_id=user.id)

    search_result = await handler.execute(
        "search_chats", {"query": "legacy API"}, context
    )
    search_text = search_result.content[0]["text"]
    assert previous_chat.id in search_text
    assert current_chat.id not in search_text
    assert matching_message.id in search_text

    read_result = await handler.execute(
        "read_chat",
        {"chat_id": previous_chat.id, "message_id": matching_message.id},
        context,
    )
    read_text = read_result.content[0]["text"]
    assert "The product decision is to keep the legacy API." in read_text
    assert "Understood; I will keep that context." in read_text


@pytest.mark.asyncio
async def test_chat_history_tools_do_not_expose_other_users_chats(db_pool):
    owner = await UsersRepository(pool=db_pool).create(
        email=f"{ULID()}@test.local",
        password_hash="not-a-real-hash",
        full_name="Owner",
    )
    other_user = await UsersRepository(pool=db_pool).create(
        email=f"{ULID()}@test.local",
        password_hash="not-a-real-hash",
        full_name="Other",
    )
    chats = ChatsRepository(pool=db_pool)
    messages = MessagesRepository(pool=db_pool)
    private_chat = await chats.create(other_user.id, title="Private context")
    await messages.create(
        private_chat.id,
        {"role": "user", "content": "A private secret."},
    )

    handler = ChatHistoryToolHandler(chats_repo=chats, messages_repo=messages)
    result = await handler.execute(
        "read_chat",
        {"chat_id": private_chat.id},
        ToolContext(chat_id="01CURRENT", user_id=owner.id),
    )

    assert result.is_error is True
    assert result.content[0]["text"] == "Chat not found."
