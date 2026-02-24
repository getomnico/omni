"""Conversation compaction service for handling long conversations."""

import json
import logging
from typing import Any

import redis.asyncio as aioredis
from anthropic.types import MessageParam

from config import (
    MAX_CONVERSATION_INPUT_TOKENS,
    COMPACTION_RECENT_MESSAGES_COUNT,
    COMPACTION_SUMMARY_MAX_TOKENS,
    ENABLE_CONVERSATION_COMPACTION,
    COMPACTION_CACHE_TTL_SECONDS,
)
from providers import LLMProvider

logger = logging.getLogger(__name__)

SUMMARIZATION_SYSTEM_PROMPT = """You are a conversation summarizer. Given a list of messages from a conversation, create a concise summary that captures:
1. The main topics discussed
2. Key decisions or conclusions reached
3. Important information that was shared (search results, document contents, etc.)
4. Any unresolved questions or pending tasks

Keep the summary factual and preserve important details that would be needed to continue the conversation coherently.
Write the summary in a narrative format, not as a list."""


class ConversationCompactor:
    """Handles compaction of long conversations to fit within LLM context limits."""

    def __init__(
        self,
        llm_provider: LLMProvider,
        redis_client: aioredis.Redis | None = None,
    ):
        self.llm_provider = llm_provider
        self.redis = redis_client

    def estimate_tokens(self, messages: list[MessageParam]) -> int:
        """Estimate token count using character heuristic (~4 chars/token).

        This is a simple approximation. For more accurate counting,
        use a proper tokenizer, but this is sufficient for threshold detection.
        """
        total_chars = 0
        for msg in messages:
            content = msg.get("content", "")
            if isinstance(content, str):
                total_chars += len(content)
            elif isinstance(content, list):
                for block in content:
                    if isinstance(block, dict):
                        if block.get("type") == "text":
                            total_chars += len(block.get("text", ""))
                        elif block.get("type") == "tool_use":
                            total_chars += len(json.dumps(block.get("input", {})))
                        elif block.get("type") == "tool_result":
                            result_content = block.get("content", "")
                            if isinstance(result_content, str):
                                total_chars += len(result_content)
                            elif isinstance(result_content, list):
                                for item in result_content:
                                    if isinstance(item, dict):
                                        if item.get("type") == "text":
                                            total_chars += len(item.get("text", ""))
                                        elif item.get("type") == "search_result":
                                            total_chars += len(item.get("title", ""))
                                            total_chars += len(item.get("source", ""))
                                            for c in item.get("content", []):
                                                if (
                                                    isinstance(c, dict)
                                                    and c.get("type") == "text"
                                                ):
                                                    total_chars += len(
                                                        c.get("text", "")
                                                    )

        return total_chars // 4

    def estimate_tools_tokens(self, tools: list[dict[str, Any]]) -> int:
        """Estimate tokens used by tool definitions."""
        return len(json.dumps(tools)) // 4

    def needs_compaction(
        self,
        messages: list[MessageParam],
        tools: list[dict[str, Any]] | None = None,
    ) -> bool:
        """Check if conversation needs compaction (above 80% of threshold)."""
        if not ENABLE_CONVERSATION_COMPACTION:
            return False

        message_tokens = self.estimate_tokens(messages)
        tool_tokens = self.estimate_tools_tokens(tools) if tools else 0
        total_tokens = message_tokens + tool_tokens
        threshold = int(MAX_CONVERSATION_INPUT_TOKENS * 0.8)

        needs_compact = total_tokens > threshold
        if needs_compact:
            logger.info(
                f"Conversation needs compaction: {total_tokens} tokens "
                f"(threshold: {threshold}, max: {MAX_CONVERSATION_INPUT_TOKENS})"
            )

        return needs_compact

    def split_messages_for_compaction(
        self,
        messages: list[MessageParam],
        has_connector_actions: bool = False,
    ) -> tuple[list[MessageParam], list[MessageParam]]:
        """Split messages into old (to summarize) and recent (keep intact).

        Ensures tool_use/tool_result pairs are never split.
        When connector actions are present, keep more recent messages to
        preserve tool call context in longer agent conversations.
        """
        recent_count = COMPACTION_RECENT_MESSAGES_COUNT
        if has_connector_actions:
            recent_count = max(recent_count, 30)

        if len(messages) <= recent_count:
            return [], messages

        split_point = len(messages) - recent_count

        # Adjust split point to keep tool pairs together
        # If the message at split_point is a user message containing tool_results,
        # we need to include the previous assistant message (with tool_use) in recent
        while split_point > 0:
            msg_at_split = messages[split_point]
            if msg_at_split.get("role") == "user":
                content = msg_at_split.get("content", [])
                if isinstance(content, list):
                    has_tool_result = any(
                        isinstance(b, dict) and b.get("type") == "tool_result"
                        for b in content
                    )
                    if has_tool_result:
                        # Move split point back to include the assistant's tool_use
                        split_point -= 1
                        continue
            break

        # Also check if the message right before split has tool_use that needs its result
        if split_point > 0 and split_point < len(messages):
            msg_before_split = messages[split_point - 1]
            if msg_before_split.get("role") == "assistant":
                content = msg_before_split.get("content", [])
                if isinstance(content, list):
                    has_tool_use = any(
                        isinstance(b, dict) and b.get("type") == "tool_use"
                        for b in content
                    )
                    if has_tool_use:
                        # The assistant made tool calls, keep them with their results
                        split_point -= 1

        old_messages = messages[:split_point]
        recent_messages = messages[split_point:]

        logger.info(
            f"Split messages: {len(old_messages)} old, {len(recent_messages)} recent"
        )

        return old_messages, recent_messages

    def _format_messages_for_summary(self, messages: list[MessageParam]) -> str:
        """Format messages into a readable text for summarization."""
        formatted_parts = []

        for msg in messages:
            role = msg.get("role", "unknown").upper()
            content = msg.get("content", "")

            if isinstance(content, str):
                formatted_parts.append(f"{role}: {content}")
            elif isinstance(content, list):
                text_parts = []
                for block in content:
                    if isinstance(block, dict):
                        if block.get("type") == "text":
                            text_parts.append(block.get("text", ""))
                        elif block.get("type") == "tool_use":
                            tool_name = block.get("name", "unknown")
                            tool_input = block.get("input", {})
                            text_parts.append(
                                f"[Called tool: {tool_name} with {json.dumps(tool_input)[:200]}...]"
                            )
                        elif block.get("type") == "tool_result":
                            # Summarize tool results briefly
                            result_content = block.get("content", "")
                            if isinstance(result_content, str):
                                preview = result_content[:500]
                            elif isinstance(result_content, list):
                                # Count search results
                                search_count = sum(
                                    1
                                    for item in result_content
                                    if isinstance(item, dict)
                                    and item.get("type") == "search_result"
                                )
                                if search_count > 0:
                                    preview = f"[{search_count} search results]"
                                else:
                                    preview = f"[{len(result_content)} content blocks]"
                            else:
                                preview = "[tool result]"
                            text_parts.append(f"[Tool result: {preview}]")

                if text_parts:
                    formatted_parts.append(f"{role}: {' '.join(text_parts)}")

        return "\n\n".join(formatted_parts)

    async def create_summary(self, messages: list[MessageParam]) -> str:
        """Use LLM to summarize older messages."""
        formatted_messages = self._format_messages_for_summary(messages)

        prompt = f"""{SUMMARIZATION_SYSTEM_PROMPT}

Here are the messages to summarize:

{formatted_messages}

Summary:"""

        logger.info(f"Creating summary of {len(messages)} messages")

        summary = await self.llm_provider.generate_response(
            prompt=prompt,
            max_tokens=COMPACTION_SUMMARY_MAX_TOKENS,
            temperature=0.3,
        )

        logger.info(f"Generated summary: {len(summary)} chars")
        return summary.strip()

    async def get_cached_summary(self, chat_id: str, message_count: int) -> str | None:
        """Get cached summary if still valid (same message count in old portion)."""
        if not self.redis:
            return None

        cache_key = f"chat:{chat_id}:compaction"
        try:
            cached = await self.redis.hgetall(cache_key)
            if cached and int(cached.get("message_count", 0)) == message_count:
                logger.info(
                    f"Cache hit for chat {chat_id} with {message_count} messages"
                )
                return cached.get("summary")
        except Exception as e:
            logger.warning(f"Failed to get cached summary: {e}")

        return None

    async def cache_summary(
        self, chat_id: str, summary: str, message_count: int
    ) -> None:
        """Cache summary with message count for invalidation."""
        if not self.redis:
            return

        cache_key = f"chat:{chat_id}:compaction"
        try:
            await self.redis.hset(
                cache_key,
                mapping={
                    "summary": summary,
                    "message_count": str(message_count),
                },
            )
            await self.redis.expire(cache_key, COMPACTION_CACHE_TTL_SECONDS)
            logger.info(f"Cached summary for chat {chat_id}")
        except Exception as e:
            logger.warning(f"Failed to cache summary: {e}")

    async def compact_conversation(
        self,
        chat_id: str,
        messages: list[MessageParam],
    ) -> list[MessageParam]:
        """Main entry point: compact a conversation if needed.

        Returns a new message list with old messages summarized.
        """
        if not ENABLE_CONVERSATION_COMPACTION:
            return messages

        old_messages, recent_messages = self.split_messages_for_compaction(messages)

        if not old_messages:
            return messages

        # Check cache first
        cached_summary = await self.get_cached_summary(chat_id, len(old_messages))

        if cached_summary:
            summary = cached_summary
        else:
            summary = await self.create_summary(old_messages)
            await self.cache_summary(chat_id, summary, len(old_messages))

        # Create a system-like message with the summary
        summary_message = MessageParam(
            role="user",
            content=f"[CONVERSATION SUMMARY - The following summarizes the earlier part of our conversation]\n\n{summary}\n\n[END SUMMARY - Recent messages follow]",
        )

        # Return summary + recent messages
        compacted = [summary_message] + list(recent_messages)

        logger.info(
            f"Compacted conversation from {len(messages)} to {len(compacted)} messages"
        )

        return compacted
