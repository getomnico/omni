#!/usr/bin/env python3
"""
Unit tests for the conversation compaction service.
"""
import pytest
from unittest.mock import AsyncMock, MagicMock, patch

from anthropic.types import MessageParam

from providers import TokenUsage
from services.compaction import ConversationCompactor


@pytest.mark.unit
class TestTokenEstimation:
    """Test cases for token estimation."""

    @pytest.fixture
    def compactor(self):
        """Create a compactor with mocked LLM provider."""
        mock_llm = AsyncMock()
        return ConversationCompactor(llm_provider=mock_llm)

    def test_estimate_tokens_simple_text(self, compactor):
        """Test token estimation for simple text messages."""
        messages = [
            MessageParam(
                role="user", content="Hello, how are you?"
            ),  # 19 chars -> 4 tokens
        ]
        tokens = compactor.estimate_tokens(messages)
        assert tokens == 4

    def test_estimate_tokens_multiple_messages(self, compactor):
        """Test token estimation for multiple messages."""
        messages = [
            MessageParam(role="user", content="Hello"),  # 5 chars -> 1 token
            MessageParam(
                role="assistant", content="Hi there, how can I help?"
            ),  # 26 chars -> 6 tokens
        ]
        tokens = compactor.estimate_tokens(messages)
        assert tokens == 7

    def test_estimate_tokens_with_content_blocks(self, compactor):
        """Test token estimation for messages with content blocks."""
        messages = [
            MessageParam(
                role="assistant",
                content=[
                    {"type": "text", "text": "Let me search for that."},  # 23 chars
                    {
                        "type": "tool_use",
                        "id": "123",
                        "name": "search",
                        "input": {"query": "test"},
                    },  # ~17 chars for input
                ],
            ),
        ]
        tokens = compactor.estimate_tokens(messages)
        # 23 + len('{"query": "test"}') = 23 + 17 = 40 -> 10 tokens
        assert tokens == 10

    def test_estimate_tokens_with_tool_results(self, compactor):
        """Test token estimation for messages with tool results."""
        messages = [
            MessageParam(
                role="user",
                content=[
                    {
                        "type": "tool_result",
                        "tool_use_id": "123",
                        "content": "This is the result of the search",
                    },
                ],
            ),
        ]
        tokens = compactor.estimate_tokens(messages)
        # "This is the result of the search" = 33 chars -> 8 tokens
        assert tokens == 8

    def test_estimate_tokens_empty_messages(self, compactor):
        """Test token estimation for empty message list."""
        messages = []
        tokens = compactor.estimate_tokens(messages)
        assert tokens == 0


@pytest.mark.unit
class TestNeedsCompaction:
    """Test cases for compaction threshold detection."""

    @pytest.fixture
    def compactor(self):
        """Create a compactor with mocked LLM provider."""
        mock_llm = AsyncMock()
        return ConversationCompactor(llm_provider=mock_llm)

    def test_needs_compaction_below_threshold(self, compactor):
        """Test that short conversations don't need compaction."""
        messages = [
            MessageParam(role="user", content="Hello"),
            MessageParam(role="assistant", content="Hi there!"),
        ]
        assert not compactor.needs_compaction(messages)

    @patch("services.compaction.MAX_CONVERSATION_INPUT_TOKENS", 100)
    def test_needs_compaction_above_threshold(self, compactor):
        """Test that long conversations trigger compaction."""
        # Create a message with > 80 tokens (> 320 chars for threshold of 100)
        long_content = "A" * 400  # 400 chars = 100 tokens, above 80% of 100 = 80
        messages = [
            MessageParam(role="user", content=long_content),
        ]
        assert compactor.needs_compaction(messages)

    @patch("services.compaction.ENABLE_CONVERSATION_COMPACTION", False)
    def test_needs_compaction_disabled(self, compactor):
        """Test that compaction can be disabled via feature flag."""
        long_content = "A" * 10000
        messages = [
            MessageParam(role="user", content=long_content),
        ]
        assert not compactor.needs_compaction(messages)


@pytest.mark.unit
class TestMessageSplitting:
    """Test cases for message splitting logic."""

    @pytest.fixture
    def compactor(self):
        """Create a compactor with mocked LLM provider."""
        mock_llm = AsyncMock()
        return ConversationCompactor(llm_provider=mock_llm)

    @patch("services.compaction.COMPACTION_RECENT_MESSAGES_COUNT", 5)
    def test_split_messages_basic(self, compactor):
        """Test basic message splitting."""
        messages = [
            MessageParam(role="user", content=f"Message {i}") for i in range(10)
        ]
        old, recent = compactor.split_messages_for_compaction(messages)

        assert len(old) == 5
        assert len(recent) == 5
        assert recent[0]["content"] == "Message 5"

    @patch("services.compaction.COMPACTION_RECENT_MESSAGES_COUNT", 5)
    def test_split_messages_short_conversation(self, compactor):
        """Test that short conversations aren't split."""
        messages = [
            MessageParam(role="user", content="Message 1"),
            MessageParam(role="assistant", content="Response 1"),
        ]
        old, recent = compactor.split_messages_for_compaction(messages)

        assert len(old) == 0
        assert len(recent) == 2

    @patch("services.compaction.COMPACTION_RECENT_MESSAGES_COUNT", 3)
    def test_split_preserves_tool_pairs(self, compactor):
        """Test that tool_use/tool_result pairs are kept together."""
        messages = [
            MessageParam(role="user", content="Find something"),
            MessageParam(
                role="assistant",
                content=[
                    {"type": "text", "text": "Searching..."},
                    {"type": "tool_use", "id": "123", "name": "search", "input": {}},
                ],
            ),
            MessageParam(
                role="user",
                content=[
                    {"type": "tool_result", "tool_use_id": "123", "content": "Results"},
                ],
            ),
            MessageParam(role="assistant", content="Here are the results."),
            MessageParam(role="user", content="Thanks!"),
        ]

        old, recent = compactor.split_messages_for_compaction(messages)

        # The tool_use and tool_result should be kept together in recent
        # Split point should be adjusted to keep the pair together
        assert len(recent) >= 3
        # Check that tool_result message is in recent (not split from tool_use)
        has_tool_result = any(
            isinstance(msg.get("content"), list)
            and any(
                b.get("type") == "tool_result"
                for b in msg.get("content", [])
                if isinstance(b, dict)
            )
            for msg in recent
        )
        has_tool_use = any(
            isinstance(msg.get("content"), list)
            and any(
                b.get("type") == "tool_use"
                for b in msg.get("content", [])
                if isinstance(b, dict)
            )
            for msg in recent
        )
        # If tool_result is in recent, tool_use should also be in recent
        if has_tool_result:
            assert has_tool_use


@pytest.mark.unit
class TestSummaryGeneration:
    """Test cases for summary generation."""

    @pytest.fixture
    def mock_llm(self):
        """Create a mock LLM provider."""
        mock = AsyncMock()
        mock.generate_response.return_value = (
            "This is a summary of the conversation.",
            TokenUsage(input_tokens=50, output_tokens=10),
        )
        return mock

    @pytest.fixture
    def compactor(self, mock_llm):
        """Create a compactor with mocked LLM provider."""
        return ConversationCompactor(llm_provider=mock_llm)

    @pytest.mark.asyncio
    async def test_create_summary(self, compactor, mock_llm):
        """Test summary generation calls LLM correctly."""
        messages = [
            MessageParam(role="user", content="What is Python?"),
            MessageParam(role="assistant", content="Python is a programming language."),
        ]

        summary = await compactor.create_summary(messages)

        assert summary == "This is a summary of the conversation."
        mock_llm.generate_response.assert_called_once()

        call_args = mock_llm.generate_response.call_args
        assert "max_tokens" in call_args.kwargs
        assert "temperature" in call_args.kwargs
        assert call_args.kwargs["temperature"] == 0.3


@pytest.mark.unit
class TestRedisCaching:
    """Test cases for Redis caching."""

    @pytest.fixture
    def mock_redis(self):
        """Create a mock Redis client."""
        mock = AsyncMock()
        mock.hgetall.return_value = {}
        return mock

    @pytest.fixture
    def mock_llm(self):
        """Create a mock LLM provider."""
        mock = AsyncMock()
        mock.generate_response.return_value = (
            "Summary",
            TokenUsage(input_tokens=50, output_tokens=5),
        )
        return mock

    @pytest.fixture
    def compactor(self, mock_llm, mock_redis):
        """Create a compactor with mocked providers."""
        return ConversationCompactor(llm_provider=mock_llm, redis_client=mock_redis)

    @pytest.mark.asyncio
    async def test_cache_miss(self, compactor, mock_redis):
        """Test cache miss returns None."""
        mock_redis.hgetall.return_value = {}

        result = await compactor.get_cached_summary("chat-123", 5)

        assert result is None
        mock_redis.hgetall.assert_called_once_with("chat:chat-123:compaction")

    @pytest.mark.asyncio
    async def test_cache_hit_same_count(self, compactor, mock_redis):
        """Test cache hit with matching message count."""
        mock_redis.hgetall.return_value = {
            "summary": "Cached summary",
            "message_count": "5",
        }

        result = await compactor.get_cached_summary("chat-123", 5)

        assert result == "Cached summary"

    @pytest.mark.asyncio
    async def test_cache_invalidation_different_count(self, compactor, mock_redis):
        """Test cache invalidation when message count differs."""
        mock_redis.hgetall.return_value = {
            "summary": "Cached summary",
            "message_count": "5",
        }

        # Request with different message count
        result = await compactor.get_cached_summary("chat-123", 10)

        assert result is None

    @pytest.mark.asyncio
    async def test_cache_summary(self, compactor, mock_redis):
        """Test caching a summary."""
        await compactor.cache_summary("chat-123", "New summary", 10)

        mock_redis.hset.assert_called_once()
        mock_redis.expire.assert_called_once()


@pytest.mark.unit
class TestCompactConversation:
    """Test cases for the main compact_conversation method."""

    @pytest.fixture
    def mock_redis(self):
        """Create a mock Redis client."""
        mock = AsyncMock()
        mock.hgetall.return_value = {}
        return mock

    @pytest.fixture
    def mock_llm(self):
        """Create a mock LLM provider."""
        mock = AsyncMock()
        mock.generate_response.return_value = (
            "Summary of earlier messages.",
            TokenUsage(input_tokens=100, output_tokens=20),
        )
        return mock

    @pytest.fixture
    def compactor(self, mock_llm, mock_redis):
        """Create a compactor with mocked providers."""
        return ConversationCompactor(llm_provider=mock_llm, redis_client=mock_redis)

    @pytest.mark.asyncio
    @patch("services.compaction.COMPACTION_RECENT_MESSAGES_COUNT", 3)
    async def test_compact_conversation_basic(self, compactor, mock_llm, mock_redis):
        """Test basic conversation compaction."""
        messages = [MessageParam(role="user", content=f"Message {i}") for i in range(6)]

        result = await compactor.compact_conversation("chat-123", messages)

        # Should have summary message + 3 recent messages = 4
        assert len(result) == 4
        # First message should be the summary
        assert "[CONVERSATION SUMMARY" in result[0]["content"]
        assert "Summary of earlier messages." in result[0]["content"]

    @pytest.mark.asyncio
    @patch("services.compaction.COMPACTION_RECENT_MESSAGES_COUNT", 10)
    async def test_compact_conversation_no_old_messages(self, compactor, mock_llm):
        """Test that short conversations aren't compacted."""
        messages = [
            MessageParam(role="user", content="Hello"),
            MessageParam(role="assistant", content="Hi!"),
        ]

        result = await compactor.compact_conversation("chat-123", messages)

        # Should return original messages unchanged
        assert len(result) == 2
        assert result == messages

    @pytest.mark.asyncio
    @patch("services.compaction.COMPACTION_RECENT_MESSAGES_COUNT", 3)
    async def test_compact_conversation_uses_cache(
        self, compactor, mock_llm, mock_redis
    ):
        """Test that cached summary is used when available."""
        mock_redis.hgetall.return_value = {
            "summary": "Cached summary from earlier",
            "message_count": "3",
        }

        messages = [MessageParam(role="user", content=f"Message {i}") for i in range(6)]

        result = await compactor.compact_conversation("chat-123", messages)

        # Should use cached summary, not generate new one
        mock_llm.generate_response.assert_not_called()
        assert "Cached summary from earlier" in result[0]["content"]

    @pytest.mark.asyncio
    @patch("services.compaction.ENABLE_CONVERSATION_COMPACTION", False)
    async def test_compact_conversation_disabled(self, compactor):
        """Test that compaction returns original when disabled."""
        messages = [
            MessageParam(role="user", content=f"Message {i}") for i in range(100)
        ]

        result = await compactor.compact_conversation("chat-123", messages)

        assert result == messages


@pytest.mark.unit
class TestCompactorWithoutRedis:
    """Test cases for compactor without Redis (no caching)."""

    @pytest.fixture
    def mock_llm(self):
        """Create a mock LLM provider."""
        mock = AsyncMock()
        mock.generate_response.return_value = (
            "Summary",
            TokenUsage(input_tokens=50, output_tokens=5),
        )
        return mock

    @pytest.fixture
    def compactor(self, mock_llm):
        """Create a compactor without Redis."""
        return ConversationCompactor(llm_provider=mock_llm, redis_client=None)

    @pytest.mark.asyncio
    async def test_get_cached_summary_no_redis(self, compactor):
        """Test that cache operations gracefully handle no Redis."""
        result = await compactor.get_cached_summary("chat-123", 5)
        assert result is None

    @pytest.mark.asyncio
    async def test_cache_summary_no_redis(self, compactor):
        """Test that caching is skipped without Redis."""
        # Should not raise
        await compactor.cache_summary("chat-123", "Summary", 5)

    @pytest.mark.asyncio
    @patch("services.compaction.COMPACTION_RECENT_MESSAGES_COUNT", 2)
    async def test_compact_conversation_no_redis(self, compactor, mock_llm):
        """Test compaction works without Redis."""
        messages = [MessageParam(role="user", content=f"Message {i}") for i in range(5)]

        result = await compactor.compact_conversation("chat-123", messages)

        # Should still work, just without caching
        assert len(result) == 3  # summary + 2 recent
        mock_llm.generate_response.assert_called_once()


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
