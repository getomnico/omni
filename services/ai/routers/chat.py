import asyncio
import json
import logging
from typing import cast

from fastapi import APIRouter, HTTPException, Path, Request
from fastapi.responses import StreamingResponse
from pydantic import ValidationError

from db import ChatsRepository, MessagesRepository, SourcesRepository
from db.documents import DocumentsRepository
from db.models import Chat
from tools import (
    SearcherTool,
    ToolRegistry,
    ToolContext,
    SearchToolHandler,
    ConnectorToolHandler,
)
from tools.search_handler import SEARCH_TOOLS
from tools.sandbox_handler import SandboxToolHandler
from config import (
    DEFAULT_MAX_TOKENS,
    DEFAULT_TEMPERATURE,
    DEFAULT_TOP_P,
    AGENT_MAX_ITERATIONS,
    CONNECTOR_MANAGER_URL,
    APPROVAL_TIMEOUT_SECONDS,
    SANDBOX_URL,
)
from providers import LLMProvider
from prompts import build_chat_system_prompt
from services.compaction import ConversationCompactor
from state import AppState

from anthropic import MessageStreamEvent, AsyncStream
from anthropic.types import (
    MessageParam,
    TextBlockParam,
    ToolUseBlockParam,
    TextCitationParam,
    CitationCharLocationParam,
    CitationPageLocationParam,
    CitationContentBlockLocationParam,
    CitationSearchResultLocationParam,
    CitationWebSearchResultLocationParam,
    CitationsDelta,
    ToolResultBlockParam,
    SearchResultBlockParam,
    CitationsConfigParam,
)

router = APIRouter(tags=["chat"])
logger = logging.getLogger(__name__)

TITLE_GENERATION_SYSTEM_PROMPT = """You are a helpful assistant that generates concise, descriptive titles for chat conversations.
Based on the first message(s) of a conversation, generate a title that is:
- 3-7 words long
- Descriptive and specific
- Written in title case
- Does not include quotes or special formatting

Just respond with the title text, nothing else."""


def _resolve_llm_provider(state: AppState, chat: Chat) -> LLMProvider:
    """Resolve which LLM provider to use for a chat.
    Priority: chat's model -> default model -> first available.
    """
    models = state.models
    if not models:
        raise HTTPException(status_code=503, detail="No models configured")

    if chat.model_id and chat.model_id in models:
        return models[chat.model_id]

    if state.default_model_id and state.default_model_id in models:
        return models[state.default_model_id]

    return next(iter(models.values()))


def convert_citation_to_param(citation_delta: CitationsDelta) -> TextCitationParam:
    citation = citation_delta.citation
    if citation.type == "char_location":
        return CitationCharLocationParam(
            type="char_location",
            start_char_index=citation.start_char_index,
            end_char_index=citation.end_char_index,
            document_title=citation.document_title,
            document_index=citation.document_index,
            cited_text=citation.cited_text,
        )
    elif citation.type == "page_location":
        return CitationPageLocationParam(
            type="page_location",
            start_page_number=citation.start_page_number,
            end_page_number=citation.end_page_number,
            document_title=citation.document_title,
            document_index=citation.document_index,
            cited_text=citation.cited_text,
        )
    elif citation.type == "content_block_location":
        return CitationContentBlockLocationParam(
            type="content_block_location",
            start_block_index=citation.start_block_index,
            end_block_index=citation.end_block_index,
            document_title=citation.document_title,
            document_index=citation.document_index,
            cited_text=citation.cited_text,
        )
    elif citation.type == "search_result_location":
        return CitationSearchResultLocationParam(
            type="search_result_location",
            start_block_index=citation.start_block_index,
            end_block_index=citation.end_block_index,
            search_result_index=citation.search_result_index,
            title=citation.title,
            source=citation.source,
            cited_text=citation.cited_text,
        )
    elif citation.type == "web_search_result_location":
        return CitationWebSearchResultLocationParam(
            type="web_search_result_location",
            url=citation.url,
            title=citation.title,
            encrypted_index=citation.encrypted_index,
            cited_text=citation.cited_text,
        )
    else:
        raise ValueError(f"Unknown citation type: {citation.type}")


async def _build_registry(
    request: Request, chat: Chat
) -> tuple[ToolRegistry, list[dict] | None]:
    """Build a ToolRegistry with all available handlers.

    Returns the registry and the raw connector actions list (for system prompt).
    """
    registry = ToolRegistry()

    # Always register search tools
    registry.register(SearchToolHandler(searcher_tool=request.app.state.searcher_tool))

    connector_actions: list[dict] | None = None

    # Register connector tools if connector-manager is configured
    if CONNECTOR_MANAGER_URL:
        connector_handler = ConnectorToolHandler(
            connector_manager_url=CONNECTOR_MANAGER_URL,
            user_id=chat.user_id,
            redis_client=getattr(request.app.state, "redis_client", None),
        )
        await connector_handler._ensure_initialized()
        registry.register(connector_handler)

        # Collect action metadata for system prompt
        if connector_handler._actions:
            connector_actions = [
                {
                    "source_type": a.source_type,
                    "action_name": a.action_name,
                    "description": a.description,
                    "mode": a.mode,
                }
                for a in connector_handler._actions.values()
            ]

    # Register sandbox tools if sandbox service is configured
    if SANDBOX_URL:
        registry.register(
            SandboxToolHandler(
                sandbox_url=SANDBOX_URL,
                content_storage=request.app.state.content_storage,
                documents_repo=DocumentsRepository(),
            )
        )

    return registry, connector_actions


async def _save_pending_approval(
    redis_client,
    chat_id: str,
    tool_call: dict,
    conversation_messages: list[MessageParam],
    action_info: dict | None = None,
) -> str:
    """Save pending approval state to Redis."""
    import ulid

    approval_id = str(ulid.ULID())
    state = {
        "approval_id": approval_id,
        "tool_call": {
            "id": tool_call["id"],
            "name": tool_call["name"],
            "input": tool_call["input"],
        },
        "conversation_messages": conversation_messages,
        "source_id": action_info.get("source_id") if action_info else None,
        "source_type": action_info.get("source_type") if action_info else None,
        "action_name": action_info.get("action_name") if action_info else None,
    }

    key = f"chat:{chat_id}:pending_approval"
    await redis_client.set(
        key, json.dumps(state, default=str), ex=APPROVAL_TIMEOUT_SECONDS
    )
    logger.info(f"Saved pending approval {approval_id} for chat {chat_id}")
    return approval_id


async def _get_pending_approval(redis_client, chat_id: str) -> dict | None:
    """Get pending approval state from Redis."""
    key = f"chat:{chat_id}:pending_approval"
    try:
        data = await redis_client.get(key)
        if data:
            return json.loads(data)
    except Exception as e:
        logger.warning(f"Failed to get pending approval: {e}")
    return None


async def _clear_pending_approval(redis_client, chat_id: str) -> None:
    """Clear pending approval state from Redis."""
    key = f"chat:{chat_id}:pending_approval"
    await redis_client.delete(key)


@router.get("/chat/{chat_id}/stream")
async def stream_chat(
    request: Request, chat_id: str = Path(..., description="Chat thread ID")
):
    """Stream AI response for a chat thread using Server-Sent Events"""
    if not request.app.state.searcher_tool:
        raise HTTPException(status_code=500, detail="Searcher tool not initialized")

    # Retrieve chat and messages from database
    chats_repo = ChatsRepository()
    chat = await chats_repo.get(chat_id)
    if not chat:
        raise HTTPException(status_code=404, detail="Chat thread not found")

    llm_provider = _resolve_llm_provider(request.app.state, chat)

    messages_repo = MessagesRepository()
    chat_messages = await messages_repo.get_by_chat(chat_id)
    if not chat_messages:
        raise HTTPException(status_code=404, detail="No messages found for chat")

    # Build registry and discover connector actions
    registry, connector_actions = await _build_registry(request, chat)
    all_tools = registry.get_all_tools()

    # Check for pending approval resume flow
    redis_client = getattr(request.app.state, "redis_client", None)
    pending = None
    if redis_client:
        pending = await _get_pending_approval(redis_client, chat_id)

    # Check if we need to process - only if last message is from user (or resuming from approval)
    last_message = chat_messages[-1]
    if not pending and last_message.message.get("role") != "user":
        logger.info(
            f"Last message is not from user, no processing needed. Chat ID: {chat_id}"
        )

        async def empty_generator():
            yield b"event: end_of_stream\ndata: No new user message to process.\n\n"

        return StreamingResponse(
            empty_generator(),
            media_type="text/event-stream",
            headers={"Cache-Control": "no-cache", "Connection": "keep-alive"},
        )

    # Build messages for conversation from stored messages
    messages: list[MessageParam] = [
        MessageParam(**msg.message) for msg in chat_messages
    ]

    # Check if conversation needs compaction
    compactor = ConversationCompactor(
        llm_provider=llm_provider,
        redis_client=redis_client,
    )
    if compactor.needs_compaction(messages, all_tools):
        logger.info(f"Compacting conversation for chat {chat_id}")
        messages = await compactor.compact_conversation(chat_id, messages)

    # Build system prompt from active sources
    sources_repo = SourcesRepository()
    active_sources = await sources_repo.get_active_sources()
    system_prompt = build_chat_system_prompt(active_sources, connector_actions)

    # Stream AI response with tool calling
    async def stream_generator():
        try:
            conversation_messages = messages.copy()

            # Handle approval resume
            if pending:
                logger.info(f"Resuming from pending approval for chat {chat_id}")
                await _clear_pending_approval(redis_client, chat_id)

                tool_call = pending["tool_call"]

                # Check if this was approved or denied by looking at DB
                # For now, if we're resuming, it was approved (the frontend only
                # re-invokes the stream after approval)
                context = ToolContext(
                    chat_id=chat_id,
                    user_id=chat.user_id,
                    user_email=None,
                )
                result = await registry.execute(
                    tool_call["name"], tool_call["input"], context
                )

                tool_result = ToolResultBlockParam(
                    type="tool_result",
                    tool_use_id=tool_call["id"],
                    content=result.content,
                    is_error=result.is_error,
                )

                # Emit the tool result to the client
                yield f"event: message\ndata: {json.dumps(tool_result)}\n\n"

                tool_result_message = MessageParam(role="user", content=[tool_result])
                conversation_messages.append(tool_result_message)
                yield f"event: save_message\ndata: {json.dumps(tool_result_message)}\n\n"

            logger.info(
                f"Starting conversation with {len(conversation_messages)} initial messages"
            )

            # Extract the first user message query for caching purposes
            original_user_query = None
            for msg in conversation_messages:
                if msg.get("role") == "user":
                    content = msg.get("content", "")
                    if isinstance(content, str):
                        original_user_query = content
                        break
                    elif isinstance(content, list):
                        text_parts = [
                            block.get("text", "")
                            for block in content
                            if isinstance(block, dict) and block.get("type") == "text"
                        ]
                        if text_parts:
                            original_user_query = " ".join(text_parts)
                            break

            context = ToolContext(
                chat_id=chat_id,
                user_id=chat.user_id,
                user_email=None,
                original_user_query=original_user_query,
            )

            for iteration in range(AGENT_MAX_ITERATIONS):
                # Check if client disconnected before starting expensive operations
                if await request.is_disconnected():
                    logger.info(
                        f"Client disconnected, stopping stream for chat {chat_id}"
                    )
                    break

                logger.info(f"Iteration {iteration + 1}/{AGENT_MAX_ITERATIONS}")
                content_blocks: list[TextBlockParam | ToolUseBlockParam] = []

                logger.info(f"Sending request to LLM provider")
                logger.debug(
                    f"Messages being sent: {json.dumps(conversation_messages, indent=2)}"
                )
                logger.debug(f"Tools available: {[tool['name'] for tool in all_tools]}")

                stream: AsyncStream[MessageStreamEvent] = llm_provider.stream_response(
                    prompt="",  # Not used when messages provided
                    messages=conversation_messages,
                    tools=all_tools,
                    max_tokens=DEFAULT_MAX_TOKENS,
                    temperature=DEFAULT_TEMPERATURE,
                    top_p=DEFAULT_TOP_P,
                    system_prompt=system_prompt,
                )

                event_index = 0
                message_stop_received = False
                async for event in stream:
                    logger.debug(f"Received event: {event} (index: {event_index})")
                    event_index += 1

                    if event.type == "message_start":
                        logger.info(f"Message start received.")

                    if event.type == "content_block_delta":
                        logger.debug(
                            f"Content block delta received at index {event.index}: {event.delta}"
                        )
                        if event.delta.type == "text_delta":
                            if event.index >= len(content_blocks):
                                logger.warning(
                                    f"Received text delta for unknown content block index {event.index}, creating new text block"
                                )
                                content_blocks.append(
                                    TextBlockParam(type="text", text="")
                                )
                            text_block = cast(
                                TextBlockParam, content_blocks[event.index]
                            )
                            text_block["text"] += event.delta.text
                        elif event.delta.type == "input_json_delta":
                            if event.index >= len(content_blocks):
                                logger.warning(
                                    f"Received input JSON delta for unknown content block index {event.index}, creating new tool use block"
                                )
                                content_blocks.append(
                                    ToolUseBlockParam(
                                        type="tool_use", id="", name="", input=""
                                    )
                                )
                            tool_use_block = cast(
                                ToolUseBlockParam, content_blocks[event.index]
                            )
                            tool_use_block["input"] = (
                                cast(str, tool_use_block["input"])
                                + event.delta.partial_json
                            )
                        elif event.delta.type == "citations_delta":
                            if event.index >= len(content_blocks):
                                logger.warning(
                                    f"Received citations delta for unknown content block index {event.index}, creating new citations block"
                                )
                                content_blocks.append(
                                    TextBlockParam(type="text", text="", citations=[])
                                )
                            text_block = cast(
                                TextBlockParam, content_blocks[event.index]
                            )
                            if (
                                "citations" not in text_block
                                or not text_block["citations"]
                            ):
                                text_block["citations"] = []
                            citations = cast(
                                list[TextCitationParam], text_block["citations"]
                            )
                            citations.append(convert_citation_to_param(event.delta))
                    elif event.type == "content_block_start":
                        if event.content_block.type == "text":
                            logger.info(f"Text block start: {event.content_block.text}")
                            content_blocks.append(
                                TextBlockParam(
                                    type="text", text=event.content_block.text
                                )
                            )
                        elif event.content_block.type == "tool_use":
                            logger.info(
                                f"Tool use block start: {event.content_block.name} (id: {event.content_block.id})"
                            )
                            content_blocks.append(
                                ToolUseBlockParam(
                                    type="tool_use",
                                    id=event.content_block.id,
                                    name=event.content_block.name,
                                    input="",
                                )
                            )
                    elif event.type == "citation":
                        logger.info(f"Citation received: {event.citation}")
                    elif event.type == "message_stop":
                        logger.info(f"Message stop received.")
                        message_stop_received = True

                    logger.debug(
                        f"Yielding event to client: {event.to_json(indent=None)}"
                    )
                    yield f"event: message\ndata: {event.to_json(indent=None)}\n\n"

                    if message_stop_received:
                        break

                # Parse tool call inputs. Convert to JSON.
                tool_calls = [b for b in content_blocks if b["type"] == "tool_use"]
                for tool_call in tool_calls:
                    try:
                        tool_call["input"] = json.loads(cast(str, tool_call["input"]))
                    except json.JSONDecodeError as e:
                        logger.error(
                            f"Failed to parse tool call input as JSON: {tool_call['input']}. Error: {e}"
                        )
                        tool_call["input"] = {}

                assistant_message = MessageParam(
                    role="assistant", content=content_blocks
                )
                conversation_messages.append(assistant_message)

                # Send complete message to omni-web for database persistence
                yield f"event: save_message\ndata: {json.dumps(assistant_message)}\n\n"

                # If no tool calls, we're done
                if not tool_calls:
                    logger.info(
                        f"No tool calls in iteration {iteration + 1}, completing response"
                    )
                    break

                logger.info(f"Processing {len(tool_calls)} tool calls")

                # Check for disconnection before expensive tool execution
                if await request.is_disconnected():
                    logger.info(
                        f"Client disconnected before tool execution, stopping stream for chat {chat_id}"
                    )
                    break

                # Execute each tool call via the registry
                tool_results: list[ToolResultBlockParam] = []
                for tool_call in tool_calls:
                    tool_name = tool_call["name"]

                    # Check if this tool requires approval
                    if registry.requires_approval(tool_name):
                        logger.info(
                            f"Tool {tool_name} requires approval, pausing stream"
                        )

                        # Save state to Redis for resume
                        if redis_client:
                            approval_id = await _save_pending_approval(
                                redis_client,
                                chat_id,
                                tool_call,
                                conversation_messages,
                            )

                            # Emit approval_required event
                            approval_event = {
                                "approval_id": approval_id,
                                "tool_name": tool_name,
                                "tool_input": tool_call["input"],
                                "tool_call_id": tool_call["id"],
                            }
                            yield f"event: approval_required\ndata: {json.dumps(approval_event)}\n\n"
                            yield f"event: end_of_stream\ndata: Approval required\n\n"
                            return
                        else:
                            # No Redis, can't do approvals — treat as denied
                            tool_results.append(
                                ToolResultBlockParam(
                                    type="tool_result",
                                    tool_use_id=tool_call["id"],
                                    content=[
                                        {
                                            "type": "text",
                                            "text": "This action requires user approval, but the approval system is not available.",
                                        }
                                    ],
                                    is_error=True,
                                )
                            )
                            continue

                    # Execute the tool
                    result = await registry.execute(
                        tool_name, tool_call["input"], context
                    )

                    tool_result = ToolResultBlockParam(
                        type="tool_result",
                        tool_use_id=tool_call["id"],
                        content=result.content,
                        is_error=result.is_error,
                    )
                    tool_results.append(tool_result)

                    yield f"event: message\ndata: {json.dumps(tool_result)}\n\n"

                tool_result_message = MessageParam(role="user", content=tool_results)
                conversation_messages.append(tool_result_message)

                # Send complete tool result message to omni-web for database persistence
                yield f"event: save_message\ndata: {json.dumps(tool_result_message)}\n\n"

            yield f"event: end_of_stream\ndata: Stream ended\n\n"

        except asyncio.CancelledError:
            logger.info(f"Stream cancelled for chat {chat_id}")
            raise  # Re-raise to let FastAPI handle cleanup
        except Exception as e:
            logger.error(
                f"Failed to generate AI response with tools: {e}", exc_info=True
            )
            yield f"event: error\ndata: Something went wrong, please try again later.\n\n"

    return StreamingResponse(
        stream_generator(),
        media_type="text/event-stream",
        headers={"Cache-Control": "no-cache", "Connection": "keep-alive"},
    )


@router.post("/chat/{chat_id}/generate_title")
async def generate_chat_title(
    request: Request, chat_id: str = Path(..., description="Chat thread ID")
):
    """Generate a title for a chat thread based on its first messages"""
    logger.info(f"Generating title for chat: {chat_id}")

    try:
        # Get chat from database
        chats_repo = ChatsRepository()
        chat = await chats_repo.get(chat_id)
        if not chat:
            raise HTTPException(status_code=404, detail="Chat thread not found")

        llm_provider = _resolve_llm_provider(request.app.state, chat)

        # Check if title already exists
        if chat.title:
            logger.info(f"Chat already has a title: {chat.title}")
            return {"title": chat.title, "status": "existing"}

        # Get messages from database
        messages_repo = MessagesRepository()
        chat_messages = await messages_repo.get_by_chat(chat_id)
        if not chat_messages:
            raise HTTPException(
                status_code=400, detail="Not enough messages to generate title"
            )

        # Use only the user's first message to generate the title
        conversation_text = ""
        for msg in chat_messages:
            role = msg.message.get("role", "unknown")
            if role == "user":
                content = msg.message.get("content", "")
                if isinstance(content, str):
                    conversation_text += f"User: {content}\n"
                    break

        if not conversation_text.strip():
            raise HTTPException(
                status_code=400, detail="Could not extract conversation content"
            )

        logger.info(f"Extracted conversation text ({len(conversation_text)} chars)")
        logger.debug(f"Conversation text: {conversation_text[:200]}...")

        # Generate title using LLM
        prompt = f"{TITLE_GENERATION_SYSTEM_PROMPT}\n\nConversation:\n{conversation_text}\n\nTitle:"

        generated_title = await llm_provider.generate_response(
            prompt=prompt,
            max_tokens=20,
            temperature=0.7,
            top_p=0.9,
        )

        # Clean up the title
        title = generated_title.strip().strip('"').strip("'")

        # Limit title length just in case
        if len(title) > 100:
            title = title[:97] + "..."

        logger.info(f"Generated title: {title}")

        # Update chat with the new title
        updated_chat = await chats_repo.update_title(chat_id, title)
        if not updated_chat:
            raise HTTPException(status_code=500, detail="Failed to update chat title")

        return {"title": title, "status": "generated"}

    except HTTPException:
        raise
    except Exception as e:
        logger.error(
            f"Failed to generate title for chat {chat_id}: {e}",
            exc_info=True,
        )
        raise HTTPException(
            status_code=500, detail=f"Failed to generate title: {str(e)}"
        )
