"""Ensure memory bullets are fenced as untrusted and cannot impersonate system text."""

import pytest

from prompts import build_agent_system_prompt, build_chat_system_prompt


class _FakeAgent:
    instructions = "Do the thing."
    name = "TestAgent"


@pytest.mark.unit
class TestMemoryFencing:
    def test_agent_prompt_renders_memories_as_trusted_bullets(self):
        """Agent memory comes from admin-controlled instructions and prior run
        summaries — not connector data — so it is rendered as a plain trusted
        bullet list, with no <untrusted-memory> fence."""
        prompt = build_agent_system_prompt(
            _FakeAgent(),
            sources=[],
            connector_actions=None,
            user_name=None,
            user_email="agent@example.com",
            memories=["User likes brevity", "Prior run delivered 3 reports"],
        )
        assert "<untrusted-memory>" not in prompt
        assert "## Agent memory (from prior runs)" in prompt
        assert "User likes brevity" in prompt
        assert "Prior run delivered 3 reports" in prompt
        assert prompt.index("## Agent memory") > prompt.index("Execute this task now")

    def test_chat_prompt_wraps_memories_in_untrusted_fence(self):
        prompt = build_chat_system_prompt(
            sources=[],
            connector_actions=None,
            user_name=None,
            user_email="u@example.com",
            memories=["Prefers tables over prose"],
        )
        assert "<untrusted-memory>" in prompt
        assert "</untrusted-memory>" in prompt
        assert "Prefers tables over prose" in prompt

    def test_bullets_are_truncated_when_over_cap(self):
        huge = "x" * 10_000
        prompt = build_chat_system_prompt(
            sources=[],
            connector_actions=None,
            user_name=None,
            user_email="u@example.com",
            memories=[huge],
        )
        # The whole memory block stays under the cap (characters, not tokens).
        fence = prompt.split("<untrusted-memory>", 1)[1].split(
            "</untrusted-memory>", 1
        )[0]
        assert len(fence) < 5_000

    def test_no_fence_when_memories_empty(self):
        prompt = build_chat_system_prompt(
            sources=[],
            connector_actions=None,
            user_name=None,
            user_email="u@example.com",
            memories=None,
        )
        assert "<untrusted-memory>" not in prompt
