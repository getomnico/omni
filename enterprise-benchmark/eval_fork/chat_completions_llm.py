"""Chat-Completions client for OpenAI-compatible providers (DeepSeek, Moonshot, etc.).

EnterpriseRAG-Bench's bundled openai_llm.py uses the OpenAI Responses API
(`client.responses.create`), which only OpenAI itself implements. This client
talks the lowest-common-denominator OpenAI-compatible Chat Completions API,
unlocking DeepSeek / Moonshot / vLLM / etc. as the eval judge.

The eval flows in metrics_based_eval.py and comparative_eval.py do not pass
tools to the LLM, so this client does not implement function calling — keeping
it small and provider-agnostic.

Reasoning content is streamed via the `reasoning_content` field in the
chunk delta when the upstream model supports it (DeepSeek-R1, Kimi K2.x with
thinking enabled). We print but don't yield it, mirroring the Responses-API
behaviour where the reasoning summary is side-channel.

Activated by `LLM_PROVIDER=openai_compat`.

Required env:
    LLM_API_KEY     — provider API key
    LLM_BASE_URL    — provider base URL (e.g. https://api.deepseek.com/v1)
    LLM_MODEL_NAME  — model to use (e.g. deepseek-chat, kimi-k2.6)
"""

import os
from collections.abc import Generator
from typing import Any

from openai import OpenAI

from src.llm.interface import LLMInterface, Message, ReasoningLevel, ToolCall


LLM_API_KEY = os.environ.get("LLM_API_KEY")
LLM_BASE_URL = os.environ.get("LLM_BASE_URL", "https://api.deepseek.com/v1")
LLM_MODEL_NAME = os.environ.get("LLM_MODEL_NAME", "deepseek-chat")
CHEAP_LLM_MODEL_NAME = os.environ.get("CHEAP_LLM_MODEL_NAME", LLM_MODEL_NAME)


class ChatCompletionsLLM(LLMInterface):
    """OpenAI-compatible Chat Completions client. No tool-use support."""

    def __init__(
        self,
        api_key: str | None = None,
        model: str | None = None,
        base_url: str | None = None,
        tools: list[dict] | None = None,
        quiet: bool = False,
        reasoning_level: ReasoningLevel = "medium",
    ):
        self.api_key = api_key or LLM_API_KEY
        if not self.api_key:
            raise ValueError(
                "LLM API key required. Set LLM_API_KEY env var or pass api_key."
            )
        self.model = model or LLM_MODEL_NAME
        self.base_url = base_url or LLM_BASE_URL
        self.tools = tools  # accepted for interface parity, ignored
        self.quiet = quiet
        self.reasoning_level = reasoning_level
        self.client = OpenAI(api_key=self.api_key, base_url=self.base_url)

    def _build_messages(self, messages: list[Message]) -> list[dict[str, Any]]:
        """Convert internal Message objects to Chat Completions format.

        tool_call/tool_result roles are skipped — the eval doesn't use them.
        """
        out: list[dict[str, Any]] = []
        for msg in messages:
            if msg.role in ("system", "user", "assistant"):
                out.append({"role": msg.role, "content": msg.content})
        return out

    def generate(
        self, messages: list[Message]
    ) -> Generator[str | ToolCall, None, None]:
        if not self.quiet:
            print("Waiting on LLM...", flush=True)

        stream = self.client.chat.completions.create(
            model=self.model,
            messages=self._build_messages(messages),
            stream=True,
        )

        in_reasoning = False
        for chunk in stream:
            if not chunk.choices:
                continue
            delta = chunk.choices[0].delta

            # Reasoning-capable models (DeepSeek-R1, Kimi K2.x thinking) put
            # the chain-of-thought in `reasoning_content` rather than `content`.
            reasoning = getattr(delta, "reasoning_content", None)
            if reasoning:
                if not in_reasoning and not self.quiet:
                    print("\n[Reasoning]", flush=True)
                    in_reasoning = True
                if not self.quiet:
                    print(reasoning, end="", flush=True)

            content = getattr(delta, "content", None)
            if content:
                if in_reasoning and not self.quiet:
                    print("\n[/Reasoning]\n", flush=True)
                    in_reasoning = False
                yield content

        if in_reasoning and not self.quiet:
            print("\n[/Reasoning]\n", flush=True)
