"""Conversation compaction helpers for long chats and agent runs.

Raw conversation history remains durable in Postgres. This module only builds the
provider-facing compacted view and creates summaries that can be stored by the
callers in durable compaction tables.
"""

import json
import logging
from collections.abc import Awaitable, Callable
from dataclasses import dataclass
from typing import Any

from anthropic.types import ContentBlockParam, MessageParam, ToolParam

from agents.models import AgentRunLog
from config import (
    COMPACTION_RECENT_MESSAGES_COUNT,
    COMPACTION_SUMMARY_MAX_TOKENS,
    ENABLE_CONVERSATION_COMPACTION,
    MAX_CONVERSATION_INPUT_TOKENS,
)
from db.compactions import Compaction, CompactionsRepository
from db.models import ChatMessage
from providers import ContextWindowInfo, LLMProvider, TokenUsage

logger = logging.getLogger(__name__)

# Safe default requested in the compaction plan for providers that do not expose
# reliable context-window metadata.
DEFAULT_CONTEXT_WINDOW_TOKENS = 128_000
DEFAULT_OUTPUT_RESERVE_TOKENS = 8_192
DEFAULT_CONTEXT_SAFETY_MARGIN_RATIO = 0.10
MIN_RECENT_MESSAGES_AFTER_COMPACTION = 3

SUMMARIZATION_SYSTEM_PROMPT = """You are a conversation summarizer. Given a list of messages from a conversation, create a concise summary that captures:
1. The main topics discussed
2. Key decisions or conclusions reached
3. Important information that was shared (search results, document contents, etc.)
4. Any unresolved questions or pending tasks

Keep the summary factual and preserve important details that would be needed to continue the conversation coherently.
Write the summary in a narrative format, not as a list."""


@dataclass(frozen=True)
class CompactionBudget:
    context_window_tokens: int
    max_output_tokens: int = DEFAULT_OUTPUT_RESERVE_TOKENS
    safety_margin_tokens: int | None = None

    @property
    def usable_input_tokens(self) -> int:
        margin = (
            self.safety_margin_tokens
            if self.safety_margin_tokens is not None
            else int(self.context_window_tokens * DEFAULT_CONTEXT_SAFETY_MARGIN_RATIO)
        )
        return max(1, self.context_window_tokens - self.max_output_tokens - margin)

    @classmethod
    def from_context_window(
        cls,
        context_window_tokens: int | None,
        max_output_tokens: int | None = None,
    ) -> "CompactionBudget":
        return cls(
            context_window_tokens=context_window_tokens or DEFAULT_CONTEXT_WINDOW_TOKENS,
            max_output_tokens=max_output_tokens or DEFAULT_OUTPUT_RESERVE_TOKENS,
        )


@dataclass(frozen=True)
class CompactionSplit:
    old_messages: list[MessageParam]
    recent_messages: list[MessageParam]
    anchor_index: int | None

    @property
    def anchor_exists(self) -> bool:
        return self.anchor_index is not None


@dataclass(frozen=True)
class SummaryResult:
    summary: str
    usage: TokenUsage
    estimated_input_tokens: int
    estimated_summary_tokens: int
    passes: int = 1


@dataclass(frozen=True)
class PreparedConversation:
    messages: list[MessageParam]
    latest_compaction: Compaction | None
    model_context: ContextWindowInfo
    summarizer_context: ContextWindowInfo


class ConversationCompactor:
    """Handles trigger checks, split selection, and summary creation."""

    def __init__(
        self,
        llm_provider: LLMProvider,
        redis_client: Any | None = None,
        on_usage: Callable[[TokenUsage], None] | None = None,
    ):
        self.llm_provider = llm_provider
        # Kept temporarily for constructor compatibility; Redis is no longer used
        # as compaction source of truth.
        self.redis = redis_client
        self._on_usage = on_usage

    def estimate_tokens(self, messages: list[MessageParam]) -> int:
        """Estimate token count using character heuristic (~4 chars/token).

        Provider-reported usage is the source of truth after a successful call,
        but compaction needs a preflight estimate before sending the request.
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
            else:
                total_chars += len(str(content))

        return total_chars // 4

    def estimate_text_tokens(self, text: str | None) -> int:
        return len(text or "") // 4

    def estimate_tools_tokens(self, tools: list[ToolParam]) -> int:
        """Estimate tokens used by tool definitions."""
        return len(json.dumps(tools)) // 4

    def estimate_request_tokens(
        self,
        messages: list[MessageParam],
        tools: list[ToolParam] | None = None,
        system_prompt: str | None = None,
    ) -> int:
        return (
            self.estimate_tokens(messages)
            + (self.estimate_tools_tokens(tools) if tools else 0)
            + self.estimate_text_tokens(system_prompt)
        )

    def needs_compaction(
        self,
        messages: list[MessageParam],
        tools: list[ToolParam] | None = None,
        system_prompt: str | None = None,
        context_window_tokens: int | None = None,
        max_output_tokens: int | None = None,
    ) -> bool:
        """Check if a provider request is near the usable context budget."""
        if not ENABLE_CONVERSATION_COMPACTION:
            return False

        total_tokens = self.estimate_request_tokens(messages, tools, system_prompt)
        if context_window_tokens is None:
            # Preserve env override semantics until provider-specific resolution is
            # wired everywhere. Later calls pass provider context explicitly.
            context_window_tokens = MAX_CONVERSATION_INPUT_TOKENS or DEFAULT_CONTEXT_WINDOW_TOKENS
        budget = CompactionBudget.from_context_window(
            context_window_tokens, max_output_tokens
        )
        threshold = int(budget.usable_input_tokens * 0.85)

        needs_compact = total_tokens > threshold
        if needs_compact:
            logger.info(
                "Conversation needs compaction: %s estimated input tokens "
                "(threshold=%s usable=%s context=%s)",
                total_tokens,
                threshold,
                budget.usable_input_tokens,
                budget.context_window_tokens,
            )

        return needs_compact

    def _content_blocks(self, message: MessageParam) -> list[ContentBlockParam]:
        content = message["content"]
        if isinstance(content, str):
            return []
        return list(content)

    def _has_content_block_type(self, message: MessageParam, block_type: str) -> bool:
        return any(block["type"] == block_type for block in self._content_blocks(message))

    def _counts_toward_recent_user_floor(self, message: MessageParam) -> bool:
        if message["role"] != "user":
            return False
        content = message["content"]
        if isinstance(content, str):
            return True
        return any(block["type"] != "tool_result" for block in content)

    def _adjust_split_point_for_tool_boundaries(
        self,
        messages: list[MessageParam],
        split_point: int,
    ) -> int:
        while split_point > 0 and split_point < len(messages):
            if not self._has_content_block_type(messages[split_point], "tool_result"):
                break
            split_point -= 1

        if split_point > 0 and split_point < len(messages) and self._has_content_block_type(
            messages[split_point - 1], "tool_use"
        ):
            split_point -= 1

        return split_point

    def _split_messages_at_recent_user_count(
        self,
        messages: list[MessageParam],
        recent_user_count: int,
    ) -> CompactionSplit:
        split_point: int | None = None
        seen_user_messages = 0
        for index in range(len(messages) - 1, -1, -1):
            if not self._counts_toward_recent_user_floor(messages[index]):
                continue
            seen_user_messages += 1
            if seen_user_messages == recent_user_count:
                split_point = index
                break

        if split_point is None:
            return CompactionSplit([], messages, None)

        split_point = self._adjust_split_point_for_tool_boundaries(messages, split_point)
        old_messages = list(messages[:split_point])
        recent_messages = list(messages[split_point:])
        anchor_index = split_point - 1 if old_messages else None
        logger.info(
            "Split messages: %s old, %s recent", len(old_messages), len(recent_messages)
        )
        return CompactionSplit(old_messages, recent_messages, anchor_index)

    def split_messages_for_compaction(
        self,
        messages: list[MessageParam],
    ) -> tuple[list[MessageParam], list[MessageParam]]:
        """Backward-compatible split API.

        Keeps the configured number of recent user turns, plus any assistant/tool
        messages that follow them, while avoiding orphaned tool results.
        """
        split = self.select_compaction_split(messages)
        return split.old_messages, split.recent_messages

    def select_compaction_split(
        self,
        messages: list[MessageParam],
        *,
        tools: list[ToolParam] | None = None,
        system_prompt: str | None = None,
        context_window_tokens: int | None = None,
        max_output_tokens: int | None = None,
        summary_token_estimate: int | None = None,
    ) -> CompactionSplit:
        recent_count = max(
            COMPACTION_RECENT_MESSAGES_COUNT, MIN_RECENT_MESSAGES_AFTER_COMPACTION
        )

        budget = (
            CompactionBudget.from_context_window(context_window_tokens, max_output_tokens)
            if context_window_tokens
            else None
        )
        summary_token_estimate = summary_token_estimate or COMPACTION_SUMMARY_MAX_TOKENS

        candidate = self._split_messages_at_recent_user_count(messages, recent_count)
        if not budget:
            return candidate

        fallback = candidate
        for count in range(recent_count, MIN_RECENT_MESSAGES_AFTER_COMPACTION - 1, -1):
            candidate = self._split_messages_at_recent_user_count(messages, count)
            if not candidate.old_messages:
                continue
            fallback = candidate
            compacted_estimate = summary_token_estimate + self.estimate_request_tokens(
                candidate.recent_messages, tools, system_prompt
            )
            if compacted_estimate <= budget.usable_input_tokens:
                return candidate

        return fallback

    def _tool_result_preview(self, content: object) -> str:
        if isinstance(content, str):
            return content[:500]
        if isinstance(content, list):
            search_count = sum(
                1 for item in content if item["type"] == "search_result"
            )
            if search_count > 0:
                return f"[{search_count} search results]"
            return f"[{len(content)} content blocks]"
        return "[tool result]"

    def _format_messages_for_summary(self, messages: list[MessageParam]) -> str:
        """Format messages into readable text for summarization."""
        formatted_parts: list[str] = []

        for message in messages:
            role = message["role"].upper()
            content = message["content"]

            if isinstance(content, str):
                formatted_parts.append(f"{role}: {content}")
                continue

            text_parts: list[str] = []
            for block in content:
                block_type = block["type"]
                if block_type == "text":
                    text_parts.append(block["text"])
                elif block_type == "tool_use":
                    text_parts.append(
                        f"[Called tool: {block['name']} with {json.dumps(block['input'])[:200]}...]"
                    )
                elif block_type == "tool_result":
                    text_parts.append(
                        f"[Tool result: {self._tool_result_preview(block['content'])}]"
                    )

            if text_parts:
                formatted_parts.append(f"{role}: {' '.join(text_parts)}")

        return "\n\n".join(formatted_parts)

    def _summary_prompt(
        self, messages: list[MessageParam], previous_summary: str | None = None
    ) -> str:
        formatted_messages = self._format_messages_for_summary(messages)
        previous = ""
        if previous_summary:
            previous = f"""\nExisting summary of the earlier conversation:\n\n{previous_summary}\n"""

        return f"""{SUMMARIZATION_SYSTEM_PROMPT}
{previous}
Here are the newer messages to incorporate into the summary:

{formatted_messages}

Summary:"""

    async def create_summary_result(
        self,
        messages: list[MessageParam],
        previous_summary: str | None = None,
        summarizer_context_window_tokens: int | None = None,
    ) -> SummaryResult:
        """Use the summarizer LLM to create a rolling summary.

        The normal path summarizes previous durable summary + the post-anchor
        prefix. A multi-pass fallback handles extremely large migrated histories.
        """
        if not messages and previous_summary:
            estimate = self.estimate_text_tokens(previous_summary)
            return SummaryResult(
                summary=previous_summary.strip(),
                usage=TokenUsage(),
                estimated_input_tokens=estimate,
                estimated_summary_tokens=estimate,
                passes=0,
            )

        prompt = self._summary_prompt(messages, previous_summary)
        estimated_input_tokens = self.estimate_text_tokens(prompt)
        context_window = summarizer_context_window_tokens or DEFAULT_CONTEXT_WINDOW_TOKENS
        budget = CompactionBudget.from_context_window(
            context_window, COMPACTION_SUMMARY_MAX_TOKENS
        )

        if estimated_input_tokens <= budget.usable_input_tokens:
            summary, usage = await self.llm_provider.generate_response(
                prompt=prompt,
                max_tokens=COMPACTION_SUMMARY_MAX_TOKENS,
                temperature=0.3,
            )
            if self._on_usage:
                self._on_usage(usage)
            cleaned = summary.strip()
            return SummaryResult(
                summary=cleaned,
                usage=usage,
                estimated_input_tokens=estimated_input_tokens,
                estimated_summary_tokens=self.estimate_text_tokens(cleaned),
                passes=1,
            )

        logger.warning(
            "Summarization input estimate %s exceeds usable budget %s; using multi-pass fallback",
            estimated_input_tokens,
            budget.usable_input_tokens,
        )
        return await self._create_summary_multipass(
            messages,
            previous_summary=previous_summary,
            summarizer_context_window_tokens=context_window,
        )

    async def _create_summary_multipass(
        self,
        messages: list[MessageParam],
        previous_summary: str | None,
        summarizer_context_window_tokens: int,
    ) -> SummaryResult:
        budget = CompactionBudget.from_context_window(
            summarizer_context_window_tokens, COMPACTION_SUMMARY_MAX_TOKENS
        )
        prompt_overhead = self.estimate_text_tokens(
            self._summary_prompt([], previous_summary=None)
        ) + self.estimate_text_tokens(previous_summary)
        chunk_budget = max(1_000, budget.usable_input_tokens - prompt_overhead)

        chunks: list[list[MessageParam]] = []
        current: list[MessageParam] = []
        current_tokens = 0
        for msg in messages:
            msg_tokens = self.estimate_tokens([msg])
            if current and current_tokens + msg_tokens > chunk_budget:
                chunks.append(current)
                current = []
                current_tokens = 0
            current.append(msg)
            current_tokens += msg_tokens
        if current:
            chunks.append(current)

        total_usage = TokenUsage()
        chunk_summaries: list[str] = []
        passes = 0
        rolling_previous = previous_summary
        for chunk in chunks:
            prompt = self._summary_prompt(chunk, rolling_previous)
            summary, usage = await self.llm_provider.generate_response(
                prompt=prompt,
                max_tokens=COMPACTION_SUMMARY_MAX_TOKENS,
                temperature=0.3,
            )
            if self._on_usage:
                self._on_usage(usage)
            total_usage.input_tokens += usage.input_tokens
            total_usage.output_tokens += usage.output_tokens
            total_usage.cache_read_tokens += usage.cache_read_tokens
            total_usage.cache_creation_tokens += usage.cache_creation_tokens
            rolling_previous = summary.strip()
            chunk_summaries.append(rolling_previous)
            passes += 1

        final_summary = rolling_previous or "\n\n".join(chunk_summaries)
        return SummaryResult(
            summary=final_summary.strip(),
            usage=total_usage,
            estimated_input_tokens=estimated_input_tokens
            if (estimated_input_tokens := self.estimate_tokens(messages))
            else 0,
            estimated_summary_tokens=self.estimate_text_tokens(final_summary),
            passes=passes,
        )

    async def create_summary(self, messages: list[MessageParam]) -> str:
        """Backward-compatible summary API."""
        result = await self.create_summary_result(messages)
        logger.info("Generated summary: %s chars", len(result.summary))
        return result.summary

    def make_summary_message(self, summary: str) -> MessageParam:
        return MessageParam(
            role="user",
            content=(
                "[CONVERSATION SUMMARY - The following summarizes the earlier "
                "part of our conversation]\n\n"
                f"{summary}\n\n"
                "[END SUMMARY - Recent messages follow]"
            ),
        )

    async def prepare_chat_conversation(
        self,
        *,
        chat_id: str,
        chat_messages: list[ChatMessage],
        messages: list[MessageParam],
        compactions_repo: CompactionsRepository,
        target_provider: LLMProvider,
        tools: list[ToolParam],
        system_prompt: str,
        max_output_tokens: int,
        on_compaction_start: Callable[[], Awaitable[None]] | None = None,
    ) -> PreparedConversation:
        model_context = await target_provider.get_context_window_tokens()
        summarizer_context = await self.llm_provider.get_context_window_tokens()
        active_path_ids = [row.id for row in chat_messages]
        latest_compaction = await compactions_repo.get_latest_for_chat_path(
            chat_id, active_path_ids
        )

        previous_anchor_index: int | None = None
        provider_messages = messages
        has_prior_summary = False
        if latest_compaction is not None:
            previous_anchor_index = next(
                (
                    i
                    for i, row in enumerate(chat_messages)
                    if row.id == latest_compaction.anchor_message_id
                ),
                None,
            )
            if previous_anchor_index is not None:
                has_prior_summary = True
                provider_messages = [
                    MessageParam(**latest_compaction.summary_message)
                ] + messages[previous_anchor_index + 1 :]

        if not self.needs_compaction(
            provider_messages,
            tools,
            system_prompt=system_prompt,
            context_window_tokens=model_context.tokens,
            max_output_tokens=max_output_tokens,
        ):
            return PreparedConversation(
                messages=provider_messages,
                latest_compaction=latest_compaction,
                model_context=model_context,
                summarizer_context=summarizer_context,
            )

        logger.info("Creating durable compaction for chat %s", chat_id)
        start_index = previous_anchor_index + 1 if has_prior_summary else 0
        segment_rows = chat_messages[start_index:]
        segment_messages = (
            provider_messages[1:] if has_prior_summary else provider_messages
        )
        split = self.select_compaction_split(
            segment_messages,
            tools=tools,
            system_prompt=system_prompt,
            context_window_tokens=model_context.tokens,
            max_output_tokens=max_output_tokens,
        )
        if split.anchor_index is None or not split.old_messages:
            logger.warning(
                "Chat %s needs compaction but no safe anchor was found", chat_id
            )
            return PreparedConversation(
                messages=provider_messages,
                latest_compaction=latest_compaction,
                model_context=model_context,
                summarizer_context=summarizer_context,
            )

        anchor_row = segment_rows[split.anchor_index]
        if on_compaction_start is not None:
            await on_compaction_start()
        summary_result = await self.create_summary_result(
            split.old_messages,
            previous_summary=(latest_compaction.summary if has_prior_summary else None),
            summarizer_context_window_tokens=summarizer_context.tokens,
        )
        summary_message = self.make_summary_message(summary_result.summary)
        latest_compaction = await compactions_repo.create_chat_compaction(
            chat_id=chat_id,
            anchor_message_id=anchor_row.id,
            compacted_through_seq_num=anchor_row.message_seq_num,
            previous_compaction_id=(latest_compaction.id if has_prior_summary else None),
            summary=summary_result.summary,
            summary_message=dict(summary_message),
            estimated_input_tokens=summary_result.estimated_input_tokens,
            actual_input_tokens=summary_result.usage.input_tokens or None,
            estimated_summary_tokens=summary_result.estimated_summary_tokens,
            actual_summary_tokens=summary_result.usage.output_tokens or None,
            metadata={"passes": summary_result.passes},
        )
        return PreparedConversation(
            messages=[summary_message] + split.recent_messages,
            latest_compaction=latest_compaction,
            model_context=model_context,
            summarizer_context=summarizer_context,
        )

    async def prepare_agent_conversation(
        self,
        *,
        run_id: str,
        log_rows: list[AgentRunLog],
        compactions_repo: CompactionsRepository,
        target_provider: LLMProvider,
        tools: list[ToolParam],
        system_prompt: str,
        max_output_tokens: int,
        coalesce_messages: Callable[[list[AgentRunLog]], list[MessageParam]],
    ) -> PreparedConversation:
        model_context = await target_provider.get_context_window_tokens()
        summarizer_context = await self.llm_provider.get_context_window_tokens()
        latest_compaction = await compactions_repo.get_latest_for_agent_run(run_id)

        previous_anchor_index: int | None = None
        has_prior_summary = False
        if latest_compaction is not None:
            previous_anchor_index = next(
                (
                    i
                    for i, row in enumerate(log_rows)
                    if row.id == latest_compaction.anchor_log_id
                ),
                None,
            )

        if latest_compaction is not None and previous_anchor_index is not None:
            has_prior_summary = True
            provider_messages = [
                MessageParam(**latest_compaction.summary_message)
            ] + coalesce_messages(log_rows[previous_anchor_index + 1 :])
        else:
            provider_messages = coalesce_messages(log_rows)

        if not self.needs_compaction(
            provider_messages,
            tools,
            system_prompt=system_prompt,
            context_window_tokens=model_context.tokens,
            max_output_tokens=max_output_tokens,
        ):
            return PreparedConversation(
                messages=provider_messages,
                latest_compaction=latest_compaction,
                model_context=model_context,
                summarizer_context=summarizer_context,
            )

        logger.info("Creating durable compaction for agent run %s", run_id)
        start_index = previous_anchor_index + 1 if has_prior_summary else 0
        segment_rows = log_rows[start_index:]
        segment_messages = [row.message for row in segment_rows]
        split = self.select_compaction_split(
            segment_messages,
            tools=tools,
            system_prompt=system_prompt,
            context_window_tokens=model_context.tokens,
            max_output_tokens=max_output_tokens,
        )
        if split.anchor_index is None or not split.old_messages:
            logger.warning(
                "Agent run %s needs compaction but no safe anchor was found", run_id
            )
            return PreparedConversation(
                messages=provider_messages,
                latest_compaction=latest_compaction,
                model_context=model_context,
                summarizer_context=summarizer_context,
            )

        anchor_row = segment_rows[split.anchor_index]
        summary_result = await self.create_summary_result(
            split.old_messages,
            previous_summary=(latest_compaction.summary if has_prior_summary else None),
            summarizer_context_window_tokens=summarizer_context.tokens,
        )
        summary_message = self.make_summary_message(summary_result.summary)
        latest_compaction = await compactions_repo.create_agent_run_compaction(
            run_id=run_id,
            anchor_log_id=anchor_row.id,
            compacted_through_seq_num=anchor_row.message_seq_num,
            previous_compaction_id=(latest_compaction.id if has_prior_summary else None),
            summary=summary_result.summary,
            summary_message=dict(summary_message),
            estimated_input_tokens=summary_result.estimated_input_tokens,
            actual_input_tokens=summary_result.usage.input_tokens or None,
            estimated_summary_tokens=summary_result.estimated_summary_tokens,
            actual_summary_tokens=summary_result.usage.output_tokens or None,
            metadata={"passes": summary_result.passes},
        )
        recent_rows = segment_rows[split.anchor_index + 1 :]
        return PreparedConversation(
            messages=[summary_message] + coalesce_messages(recent_rows),
            latest_compaction=latest_compaction,
            model_context=model_context,
            summarizer_context=summarizer_context,
        )

    def select_legacy_compaction_split(
        self, messages: list[MessageParam]
    ) -> CompactionSplit | None:
        if not ENABLE_CONVERSATION_COMPACTION:
            return None

        split = self.select_compaction_split(messages)
        if not split.old_messages:
            return None
        return split

    async def compact_conversation(
        self,
        chat_id: str,
        messages: list[MessageParam],
        previous_summary: str | None = None,
        summarizer_context_window_tokens: int | None = None,
    ) -> list[MessageParam]:
        """Legacy entry point: compact a conversation if possible.

        New durable callers should use `select_compaction_split`,
        `create_summary_result`, and `make_summary_message` so they can store the
        returned summary and anchor in Postgres.
        """
        split = self.select_legacy_compaction_split(messages)
        if split is None:
            return messages

        result = await self.create_summary_result(
            split.old_messages,
            previous_summary=previous_summary,
            summarizer_context_window_tokens=summarizer_context_window_tokens,
        )
        compacted = [self.make_summary_message(result.summary)] + list(
            split.recent_messages
        )

        logger.info(
            "Compacted conversation %s from %s to %s messages",
            chat_id,
            len(messages),
            len(compacted),
        )
        return compacted
