import json
import logging
from typing import cast, List
from fastapi import APIRouter, HTTPException, Path, Request
from fastapi.responses import StreamingResponse
from pydantic import ValidationError

from db import ChatsRepository, MessagesRepository
from tools import SearcherTool, SearchRequest, SearchResponse, SearchResult
from models.chat import SearchToolParams
from config import DEFAULT_MAX_TOKENS, DEFAULT_TEMPERATURE, DEFAULT_TOP_P, LLM_PROVIDER

from anthropic import MessageStreamEvent, AsyncStream
from anthropic.types import (
    MessageParam, 
    TextBlockParam,
    ToolUseBlockParam,
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

SEARCH_TOOLS = [
    {
        "name": "search_documents",
        "description": "Search enterprise documents using hybrid text and semantic search. Use this when you need to find information to answer user questions.",
        "input_schema": {
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query to find relevant documents. Can search using keywords, or a natural language question to get semantic search results."
                },
                "sources": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Optional: specific source types to search (e.g., google_drive, slack, confluence)"
                },
                "content_types": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Optional: file types to include (e.g., pdf, docx, txt)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default: 20)"
                }
            },
            "required": ["query"]
        }
    }
]


@router.get("/chat/{chat_id}/stream")
async def stream_chat(request: Request, chat_id: str = Path(..., description="Chat thread ID")):
    """Stream AI response for a chat thread using Server-Sent Events"""
    if not hasattr(request.app.state, 'llm_provider') or not request.app.state.llm_provider:
        raise HTTPException(status_code=500, detail="LLM provider not initialized")

    if not hasattr(request.app.state, 'searcher_tool') or not request.app.state.searcher_tool:
        raise HTTPException(status_code=500, detail="Searcher tool not initialized")

    # Retrieve chat and messages from database
    chats_repo = ChatsRepository()
    chat = await chats_repo.get(chat_id)
    if not chat:
        raise HTTPException(status_code=404, detail="Chat thread not found")

    messages_repo = MessagesRepository()
    chat_messages = await messages_repo.get_by_chat(chat_id)
    if not chat_messages:
        raise HTTPException(status_code=404, detail="No messages found for chat")

    # Check if we need to process - only if last message is from user
    last_message = chat_messages[-1]
    if last_message.message.get('role') != 'user':
        logger.info(f"[ASK] Last message is not from user, no processing needed. Chat ID: {chat_id}")

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

    # Stream AI response with tool calling
    async def stream_generator():
        try:
            conversation_messages = messages.copy()
            max_iterations = 7  # Prevent infinite loops
            logger.info(f"[ASK] Starting conversation with {len(conversation_messages)} initial messages")

            for iteration in range(max_iterations):
                logger.info(f"[ASK] Iteration {iteration + 1}/{max_iterations}")
                content_blocks: list[TextBlockParam | ToolUseBlockParam] = []

                logger.info(f"[ASK] Sending request to LLM provider ({LLM_PROVIDER})")
                logger.debug(f"[ASK] Messages being sent: {json.dumps(conversation_messages, indent=2)}")
                logger.debug(f"[ASK] Tools available: {[tool['name'] for tool in SEARCH_TOOLS]}")

                stream: AsyncStream[MessageStreamEvent] = request.app.state.llm_provider.stream_response(
                    prompt="",  # Not used when messages provided
                    messages=conversation_messages,
                    tools=SEARCH_TOOLS,
                    max_tokens=DEFAULT_MAX_TOKENS,
                    temperature=DEFAULT_TEMPERATURE,
                    top_p=DEFAULT_TOP_P,
                )

                event_index = 0
                message_stop_received = False
                async for event in stream:
                    logger.debug(f"[ASK] Received event: {event} (index: {event_index})")
                    event_index += 1

                    if event.type == 'content_block_delta':
                        if event.delta.type == 'text_delta':
                            text_block = cast(TextBlockParam, content_blocks[event.index])
                            text_block['text'] += event.delta.text
                        elif event.delta.type == 'input_json_delta':
                            tool_use_block = cast(ToolUseBlockParam, content_blocks[event.index])
                            tool_use_block['input'] = cast(str, tool_use_block['input']) + event.delta.partial_json
                    elif event.type == 'content_block_start':
                        if event.content_block.type == 'text':
                            logger.info(f"[ASK] Text block start: {event.content_block.text}")
                            content_blocks.append(TextBlockParam(type='text', text=event.content_block.text))
                        elif event.content_block.type == 'tool_use':
                            logger.info(f"[ASK] Tool use block start: {event.content_block.name} (id: {event.content_block.id})")
                            content_blocks.append(
                                ToolUseBlockParam(
                                    type='tool_use', 
                                    id=event.content_block.id, 
                                    name=event.content_block.name, 
                                    input=''
                                )
                            )
                    elif event.type == 'citation':
                        logger.info(f"[ASK] Citation received: {event.citation}")
                    elif event.type == 'message_stop':
                        logger.info(f"[ASK] Message stop received.")
                        message_stop_received = True
                    
                    logger.info(f"[ASK] Yielding event to client: {event.to_json(indent=None)}")
                    yield f"event: message\ndata: {event.to_json(indent=None)}\n\n"

                    if message_stop_received:
                        break

                # Parse tool call inputs. Convert to JSON.
                tool_calls = [b for b in content_blocks if b['type'] == 'tool_use']
                for tool_call in tool_calls:
                    try:
                        tool_call['input'] = json.loads(cast(str, tool_call['input']))
                    except json.JSONDecodeError as e:
                        logger.error(f"[ASK] Failed to parse tool call input as JSON: {tool_call['input']}. Error: {e}")
                        tool_call['input'] = {}

                assistant_message = MessageParam(role='assistant', content=content_blocks)
                conversation_messages.append(assistant_message)

                # Send complete message to omni-web for database persistence
                yield f"event: save_message\ndata: {json.dumps(assistant_message)}\n\n"

                # If no tool calls, we're done
                if not tool_calls:
                    logger.info(f"[ASK] No tool calls in iteration {iteration + 1}, completing response")
                    break

                logger.info(f"[ASK] Processing {len(tool_calls)} tool calls")

                # Execute each tool call and add results
                tool_results: list[ToolResultBlockParam] = []
                for tool_call in tool_calls:
                    if tool_call['name'] == 'search_documents':
                        try:
                            tool_call_params = SearchToolParams.model_validate(tool_call['input'])
                        except ValidationError as e:
                            logger.error(f"[ASK] Failed to parse search_documents tool call input: {tool_call['input']}. Error: {e}")
                            continue

                        search_query = tool_call_params.query
                        logger.info(f"[ASK] Executing search_documents tool with query: {search_query}")
                        search_results = await execute_search_tool(
                            searcher_tool=request.app.state.searcher_tool,
                            tool_input=tool_call_params, 
                            user_id=chat.user_id
                        )
                        documents = [res.document for res in search_results]
                        logger.info(f"[ASK] Search returned {len(documents)} documents")
                        logger.debug(f"[ASK] Document titles: {[doc.title for doc in documents]}...")

                        # Add each document as a document block for automatic citations
                        tool_result_content_blocks: list[SearchResultBlockParam] = []
                        for result in search_results:
                            doc = result.document
                            tool_result_content_blocks.append(
                                SearchResultBlockParam(
                                    type='search_result',
                                    title=doc.title,
                                    source=cast(str, doc.url),
                                    content=[
                                        TextBlockParam(
                                            type='text',
                                            text='\n'.join(result.highlights),
                                        )
                                    ],
                                    citations=CitationsConfigParam(enabled=True),
                                )
                            )

                        tool_result = \
                            ToolResultBlockParam(
                                type='tool_result',
                                tool_use_id=tool_call['id'],
                                content=tool_result_content_blocks,
                                is_error=False,
                            )
                        tool_results.append(tool_result)

                        yield f"event: message\ndata: {json.dumps(tool_result)}\n\n"

                tool_result_message = MessageParam(role='user', content=tool_results)
                conversation_messages.append(tool_result_message)

                # Send complete tool result message to omni-web for database persistence
                yield f"event: save_message\ndata: {json.dumps(tool_result_message)}\n\n"
            
            yield f"event: end_of_stream\ndata: Stream ended\n\n"

        except Exception as e:
            logger.error(f"[ASK] Failed to generate AI response with tools: {e}", exc_info=True)
            yield f"event: error\ndata: Something went wrong, please try again later.\n\n"

    return StreamingResponse(
        stream_generator(),
        media_type="text/event-stream",
        headers={"Cache-Control": "no-cache", "Connection": "keep-alive"},
    )

async def execute_search_tool(
    searcher_tool: SearcherTool,
    tool_input: SearchToolParams, 
    user_id: str, 
    user_email: str | None = None
) -> List[SearchResult]:
    """Execute search_documents tool by calling omni-searcher"""
    logger.info(f"[SEARCH_TOOL] Executing search with query: {tool_input.query}")
    logger.debug(f"[SEARCH_TOOL] Full search parameters: query={tool_input.query}, sources={tool_input.sources}, content_types={tool_input.content_types}, limit={tool_input.limit}")

    search_request = SearchRequest(
        query=tool_input.query,
        sources=tool_input.sources,
        content_types=tool_input.content_types,
        limit=tool_input.limit or 20,
        offset=0,
        mode="hybrid",
        user_id=user_id,
        user_email=user_email,
    )
    try:
        search_response: SearchResponse = await searcher_tool.handle(search_request)
    except Exception as e:
        logger.error(f"[SEARCH_TOOL] Search failed: {e}")
        return []

    logger.info(f"[SEARCH_TOOL] Search successful, processing {len(search_response.results)} results")
    return search_response.results


@router.post("/chat/{chat_id}/generate_title")
async def generate_chat_title(request: Request, chat_id: str = Path(..., description="Chat thread ID")):
    """Generate a title for a chat thread based on its first messages"""
    if not hasattr(request.app.state, 'llm_provider') or not request.app.state.llm_provider:
        raise HTTPException(status_code=500, detail="LLM provider not initialized")

    logger.info(f"[TITLE_GEN] Generating title for chat: {chat_id}")

    try:
        # Get chat from database
        chats_repo = ChatsRepository()
        chat = await chats_repo.get(chat_id)
        if not chat:
            raise HTTPException(status_code=404, detail="Chat thread not found")

        # Check if title already exists
        if chat.title:
            logger.info(f"[TITLE_GEN] Chat already has a title: {chat.title}")
            return {"title": chat.title, "status": "existing"}

        # Get messages from database
        messages_repo = MessagesRepository()
        chat_messages = await messages_repo.get_by_chat(chat_id)
        if not chat_messages or len(chat_messages) < 2:
            raise HTTPException(status_code=400, detail="Not enough messages to generate title")

        # Build conversation context from first few messages
        # Take first user message and first assistant response
        conversation_text = ""
        for msg in chat_messages[:3]:
            role = msg.message.get('role', 'unknown')

            if role == 'user':
                content = msg.message.get('content', '')
                if isinstance(content, str):
                    conversation_text += f"User: {content}\n"
            elif role == 'assistant':
                content = msg.message.get('content', [])
                if isinstance(content, list):
                    text_parts = [block.get('text', '') for block in content if block.get('type') == 'text']
                    if text_parts:
                        conversation_text += f"Assistant: {' '.join(text_parts)}\n"
                elif isinstance(content, str):
                    conversation_text += f"Assistant: {content}\n"

        if not conversation_text.strip():
            raise HTTPException(status_code=400, detail="Could not extract conversation content")

        logger.info(f"[TITLE_GEN] Extracted conversation text ({len(conversation_text)} chars)")
        logger.debug(f"[TITLE_GEN] Conversation text: {conversation_text[:200]}...")

        # Generate title using LLM
        prompt = f"{TITLE_GENERATION_SYSTEM_PROMPT}\n\nConversation:\n{conversation_text}\n\nTitle:"

        generated_title = await request.app.state.llm_provider.generate_response(
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

        logger.info(f"[TITLE_GEN] Generated title: {title}")

        # Update chat with the new title
        updated_chat = await chats_repo.update_title(chat_id, title)
        if not updated_chat:
            raise HTTPException(status_code=500, detail="Failed to update chat title")

        return {"title": title, "status": "generated"}

    except HTTPException:
        raise
    except Exception as e:
        logger.error(f"[TITLE_GEN] Failed to generate title for chat {chat_id}: {e}", exc_info=True)
        raise HTTPException(status_code=500, detail=f"Failed to generate title: {str(e)}")
