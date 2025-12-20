import asyncio
import json
import logging
from typing import cast

from fastapi import APIRouter, HTTPException, Path, Request
from fastapi.responses import StreamingResponse
from pydantic import ValidationError

from db import ChatsRepository, MessagesRepository
from tools import SearcherTool, SearchRequest, SearchResponse, SearchResult
from models.chat import SearchToolParams, ReadDocumentParams
from config import DEFAULT_MAX_TOKENS, DEFAULT_TEMPERATURE, DEFAULT_TOP_P, LLM_PROVIDER

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

SEARCH_TOOLS = [
    {
        "name": "search_documents",
        "description": "Search enterprise documents using hybrid text and semantic search. Use this when you need to find information to answer user questions.",
        "input_schema": {
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query to find relevant documents. Can search using keywords, or a natural language question to get semantic search results.",
                },
                "sources": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Optional: specific source types to search (e.g., google_drive, slack, confluence)",
                },
                "content_types": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Optional: file types to include (e.g., pdf, docx, txt)",
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default: 10)",
                },
            },
            "required": ["query"],
        },
    },
    {
        "name": "read_document",
        "description": "Read the content of a specific document by its URL. For small documents, returns the full content. For large documents, you can provide a query parameter to get the most relevant sections. Use this when you need detailed information from a specific document (e.g., from search results).",
        "input_schema": {
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "The ID of the document to read",
                },
                "name": {
                    "type": "string",
                    "description": "The name of the document to read",
                },
                "query": {
                    "type": "string",
                    "description": "Optional: specify what you're looking for to get the most relevant sections. If you specify line numbers, this this will be ignored.",
                },
                "start_line": {
                    "type": "integer",
                    "description": "Optional: start line number (inclusive) to read from.",
                },
                "end_line": {
                    "type": "integer",
                    "description": "Optional: end line number (inclusive) to read to",
                },
            },
            "required": ["id", "name"],
        },
    },
]


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


@router.get("/chat/{chat_id}/stream")
async def stream_chat(
    request: Request, chat_id: str = Path(..., description="Chat thread ID")
):
    """Stream AI response for a chat thread using Server-Sent Events"""
    if (
        not hasattr(request.app.state, "llm_provider")
        or not request.app.state.llm_provider
    ):
        raise HTTPException(status_code=500, detail="LLM provider not initialized")

    if (
        not hasattr(request.app.state, "searcher_tool")
        or not request.app.state.searcher_tool
    ):
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
    if last_message.message.get("role") != "user":
        logger.info(
            f"[ASK] Last message is not from user, no processing needed. Chat ID: {chat_id}"
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

    # Stream AI response with tool calling
    async def stream_generator():
        try:
            conversation_messages = messages.copy()
            max_iterations = 10  # Prevent infinite loops
            logger.info(
                f"[ASK] Starting conversation with {len(conversation_messages)} initial messages"
            )

            # Extract the first user message query for caching purposes
            # We only cache the initial question, not follow-ups, as follow-ups don't make sense in isolation
            original_user_query = None
            for msg in conversation_messages:
                if msg.get("role") == "user":
                    content = msg.get("content", "")
                    if isinstance(content, str):
                        original_user_query = content
                        break
                    elif isinstance(content, list):
                        # Extract text from content blocks
                        text_parts = [
                            block.get("text", "")
                            for block in content
                            if isinstance(block, dict) and block.get("type") == "text"
                        ]
                        if text_parts:
                            original_user_query = " ".join(text_parts)
                            break

            for iteration in range(max_iterations):
                # Check if client disconnected before starting expensive operations
                if await request.is_disconnected():
                    logger.info(
                        f"[ASK] Client disconnected, stopping stream for chat {chat_id}"
                    )
                    break

                logger.info(f"[ASK] Iteration {iteration + 1}/{max_iterations}")
                content_blocks: list[TextBlockParam | ToolUseBlockParam] = []

                logger.info(f"[ASK] Sending request to LLM provider ({LLM_PROVIDER})")
                logger.debug(
                    f"[ASK] Messages being sent: {json.dumps(conversation_messages, indent=2)}"
                )
                logger.debug(
                    f"[ASK] Tools available: {[tool['name'] for tool in SEARCH_TOOLS]}"
                )

                stream: AsyncStream[MessageStreamEvent] = (
                    request.app.state.llm_provider.stream_response(
                        prompt="",  # Not used when messages provided
                        messages=conversation_messages,
                        tools=SEARCH_TOOLS,
                        max_tokens=DEFAULT_MAX_TOKENS,
                        temperature=DEFAULT_TEMPERATURE,
                        top_p=DEFAULT_TOP_P,
                    )
                )

                event_index = 0
                message_stop_received = False
                async for event in stream:
                    logger.debug(
                        f"[ASK] Received event: {event} (index: {event_index})"
                    )
                    event_index += 1

                    if event.type == "message_start":
                        logger.info(f"[ASK] Message start received.")

                    if event.type == "content_block_delta":
                        # Amazon models send the first content_block_delta directly without sending the
                        # content_block_start event first, so we need to handle that case.
                        logger.debug(
                            f"[ASK] Content block delta received at index {event.index}: {event.delta}"
                        )
                        if event.delta.type == "text_delta":
                            if event.index >= len(content_blocks):
                                logger.warning(
                                    f"[ASK] Received text delta for unknown content block index {event.index}, creating new text block"
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
                                # This should never happen in the case of tool calls, because the start event will add a new entry in content blocks, but we handle it anyway
                                logger.warning(
                                    f"[ASK] Received input JSON delta for unknown content block index {event.index}, creating new tool use block"
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
                                    f"[ASK] Received citations delta for unknown content block index {event.index}, creating new citations block"
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
                            logger.info(
                                f"[ASK] Text block start: {event.content_block.text}"
                            )
                            content_blocks.append(
                                TextBlockParam(
                                    type="text", text=event.content_block.text
                                )
                            )
                        elif event.content_block.type == "tool_use":
                            logger.info(
                                f"[ASK] Tool use block start: {event.content_block.name} (id: {event.content_block.id})"
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
                        logger.info(f"[ASK] Citation received: {event.citation}")
                    elif event.type == "message_stop":
                        logger.info(f"[ASK] Message stop received.")
                        message_stop_received = True

                    logger.debug(
                        f"[ASK] Yielding event to client: {event.to_json(indent=None)}"
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
                            f"[ASK] Failed to parse tool call input as JSON: {tool_call['input']}. Error: {e}"
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
                        f"[ASK] No tool calls in iteration {iteration + 1}, completing response"
                    )
                    break

                logger.info(f"[ASK] Processing {len(tool_calls)} tool calls")

                # Check for disconnection before expensive tool execution
                if await request.is_disconnected():
                    logger.info(
                        f"[ASK] Client disconnected before tool execution, stopping stream for chat {chat_id}"
                    )
                    break

                # Execute each tool call and add results
                tool_results: list[ToolResultBlockParam] = []
                for tool_call in tool_calls:
                    if tool_call["name"] == "search_documents":
                        try:
                            tool_call_params = SearchToolParams.model_validate(
                                tool_call["input"]
                            )
                        except ValidationError as e:
                            logger.error(
                                f"[ASK] Failed to parse search_documents tool call input: {tool_call['input']}. Error: {e}"
                            )
                            continue

                        search_query = tool_call_params.query
                        logger.info(
                            f"[ASK] Executing search_documents tool with query: {search_query}"
                        )
                        search_results = await execute_search_tool(
                            searcher_tool=request.app.state.searcher_tool,
                            tool_input=tool_call_params,
                            user_id=chat.user_id,
                            original_user_query=original_user_query,
                        )
                        documents = [res.document for res in search_results]
                        logger.info(f"[ASK] Search returned {len(documents)} documents")
                        logger.debug(
                            f"[ASK] Document titles: {[doc.title for doc in documents]}..."
                        )

                        # Add each document as a document block for automatic citations
                        search_tool_result_content_blocks: list[
                            SearchResultBlockParam
                        ] = []
                        for result in search_results:
                            doc = result.document
                            doc_content_text_blocks = [
                                TextBlockParam(
                                    type="text",
                                    text=h,
                                )
                                for h in result.highlights
                            ]
                            search_tool_result_content_blocks.append(
                                SearchResultBlockParam(
                                    type="search_result",
                                    title=doc.title,
                                    source=doc.url or "<unknown>",
                                    content=[
                                        # Add a separate text block with the document ID, title, URL
                                        # This will help the model issue read_document calls later
                                        TextBlockParam(
                                            type="text",
                                            text=f"[Document ID: {doc.id}]",
                                        ),
                                        TextBlockParam(
                                            type="text",
                                            text=f"[Document Name: {doc.title}]",
                                        ),
                                        TextBlockParam(
                                            type="text",
                                            text=f"[URL: {doc.url or '<unknown>'}]",
                                        ),
                                        *doc_content_text_blocks,
                                    ],
                                    citations=CitationsConfigParam(enabled=True),
                                )
                            )

                        tool_result = ToolResultBlockParam(
                            type="tool_result",
                            tool_use_id=tool_call["id"],
                            content=search_tool_result_content_blocks,
                            is_error=False,
                        )
                        tool_results.append(tool_result)

                        yield f"event: message\ndata: {json.dumps(tool_result)}\n\n"

                    elif tool_call["name"] == "read_document":
                        try:
                            tool_call_params = ReadDocumentParams.model_validate(
                                tool_call["input"]
                            )
                        except ValidationError as e:
                            logger.error(
                                f"[ASK] Failed to parse read_document tool call input: {tool_call['input']}. Error: {e}"
                            )
                            continue

                        logger.info(
                            f"[ASK] Executing read_document tool with URL: {tool_call_params}"
                        )
                        read_results = await execute_read_document_tool(
                            searcher_tool=request.app.state.searcher_tool,
                            tool_input=tool_call_params,
                            user_id=chat.user_id,
                        )
                        logger.info(
                            f"[ASK] Read document returned {len(read_results)} chunks/content"
                        )

                        # Add document content as text blocks
                        read_tool_result_content_blocks: list[TextBlockParam] = []
                        for result in read_results:
                            doc = result.document
                            read_tool_result_content_blocks.append(
                                TextBlockParam(
                                    type="text",
                                    text="\n".join(result.highlights),
                                )
                            )

                        tool_result = ToolResultBlockParam(
                            type="tool_result",
                            tool_use_id=tool_call["id"],
                            content=read_tool_result_content_blocks,
                            is_error=False,
                        )
                        tool_results.append(tool_result)

                        yield f"event: message\ndata: {json.dumps(tool_result)}\n\n"

                tool_result_message = MessageParam(role="user", content=tool_results)
                conversation_messages.append(tool_result_message)

                # Send complete tool result message to omni-web for database persistence
                yield f"event: save_message\ndata: {json.dumps(tool_result_message)}\n\n"

            yield f"event: end_of_stream\ndata: Stream ended\n\n"

        except asyncio.CancelledError:
            logger.info(f"[ASK] Stream cancelled for chat {chat_id}")
            raise  # Re-raise to let FastAPI handle cleanup
        except Exception as e:
            logger.error(
                f"[ASK] Failed to generate AI response with tools: {e}", exc_info=True
            )
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
    user_email: str | None = None,
    original_user_query: str | None = None,
) -> list[SearchResult]:
    """Execute search_documents tool by calling omni-searcher"""
    logger.info(f"[SEARCH_TOOL] Executing search with query: {tool_input.query}")
    logger.debug(
        f"[SEARCH_TOOL] Full search parameters: query={tool_input.query}, sources={tool_input.sources}, content_types={tool_input.content_types}, limit={tool_input.limit}"
    )

    search_request = SearchRequest(
        query=tool_input.query,
        sources=tool_input.sources,
        content_types=tool_input.content_types,
        limit=tool_input.limit or 10,
        offset=0,
        mode="hybrid",
        user_id=user_id,
        user_email=user_email,
        is_generated_query=True,
        original_user_query=original_user_query,
        include_facets=False,
        ignore_typos=True,  # LLMs will generaly not generate typos, so we avoid typo handling
    )
    try:
        search_response: SearchResponse = await searcher_tool.handle(search_request)
    except Exception as e:
        logger.error(f"[SEARCH_TOOL] Search failed: {e}")
        return []

    logger.info(
        f"[SEARCH_TOOL] Search successful, processing {len(search_response.results)} results"
    )
    return search_response.results


async def execute_read_document_tool(
    searcher_tool: SearcherTool,
    tool_input: ReadDocumentParams,
    user_id: str,
    user_email: str | None = None,
) -> list[SearchResult]:
    """Execute read_document tool by calling omni-searcher with document_id filter"""
    logger.info(f"[READ_DOC_TOOL] Reading document: {tool_input}")
    document_id = tool_input.id
    document_content_start_line = tool_input.start_line
    document_content_end_line = tool_input.end_line

    # Create search request with document_id filter
    # Use query if provided for semantic search within document, otherwise use empty query
    search_request = SearchRequest(
        query=tool_input.query or "",
        document_id=document_id,
        document_content_start_line=document_content_start_line,
        document_content_end_line=document_content_end_line,
        limit=20,  # Get up to 20 chunks for large documents
        offset=0,
        mode="hybrid",
        user_id=user_id,
        user_email=user_email,
    )

    try:
        search_response: SearchResponse = await searcher_tool.handle(search_request)
    except Exception as e:
        logger.error(f"[READ_DOC_TOOL] Read document failed: {e}")
        return []

    logger.info(
        f"[READ_DOC_TOOL] Read successful, retrieved {len(search_response.results)} chunks"
    )
    return search_response.results


@router.post("/chat/{chat_id}/generate_title")
async def generate_chat_title(
    request: Request, chat_id: str = Path(..., description="Chat thread ID")
):
    """Generate a title for a chat thread based on its first messages"""
    if (
        not hasattr(request.app.state, "llm_provider")
        or not request.app.state.llm_provider
    ):
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

        logger.info(
            f"[TITLE_GEN] Extracted conversation text ({len(conversation_text)} chars)"
        )
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
        logger.error(
            f"[TITLE_GEN] Failed to generate title for chat {chat_id}: {e}",
            exc_info=True,
        )
        raise HTTPException(
            status_code=500, detail=f"Failed to generate title: {str(e)}"
        )
