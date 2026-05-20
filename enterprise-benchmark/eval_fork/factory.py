"""Patched EnterpriseRAG-Bench LLM factory.

Adds support for `LLM_PROVIDER=openai_compat` to route the eval through
ChatCompletionsLLM, enabling DeepSeek / Moonshot / vLLM as the judge instead
of OpenAI's Responses API (which is OpenAI-only).

Original behaviour for `openai` and `anthropic` providers is unchanged.
"""

import os

from src.llm.interface import LLMInterface, ReasoningLevel


LLM_PROVIDER = os.environ.get("LLM_PROVIDER", "openai")
LLM_MODEL_NAME = os.environ.get("LLM_MODEL_NAME")
CHEAP_LLM_MODEL_NAME = os.environ.get("CHEAP_LLM_MODEL_NAME")


def get_llm(
    tools: list[dict] | None = None,
    quiet: bool = False,
    reasoning_level: ReasoningLevel = "medium",
    model: str | None = None,
) -> LLMInterface:
    provider = LLM_PROVIDER.lower()

    if provider == "openai":
        from src.llm.openai_llm import OpenAILLM

        return OpenAILLM(
            model=model, tools=tools, quiet=quiet, reasoning_level=reasoning_level
        )
    elif provider == "anthropic":
        from src.llm.anthropic_llm import AnthropicLLM

        return AnthropicLLM(
            model=model, tools=tools, quiet=quiet, reasoning_level=reasoning_level
        )
    elif provider == "openai_compat":
        from src.llm.chat_completions_llm import ChatCompletionsLLM

        return ChatCompletionsLLM(
            model=model, tools=tools, quiet=quiet, reasoning_level=reasoning_level
        )
    else:
        raise ValueError(
            f"Unsupported LLM provider: {provider}. "
            "Supported providers: openai, anthropic, openai_compat"
        )


def get_cheap_llm(
    tools: list[dict] | None = None,
    quiet: bool = False,
    reasoning_level: ReasoningLevel = "medium",
    model: str | None = None,
) -> LLMInterface:
    provider = LLM_PROVIDER.lower()

    if provider == "openai":
        from src.llm.openai_llm import (
            CHEAP_LLM_MODEL_NAME as OPENAI_CHEAP_MODEL,
            OpenAILLM,
        )

        return OpenAILLM(
            model=model or OPENAI_CHEAP_MODEL,
            tools=tools,
            quiet=quiet,
            reasoning_level=reasoning_level,
        )
    elif provider == "anthropic":
        from src.llm.anthropic_llm import (
            CHEAP_LLM_MODEL_NAME as ANTHROPIC_CHEAP_MODEL,
            AnthropicLLM,
        )

        return AnthropicLLM(
            model=model or ANTHROPIC_CHEAP_MODEL,
            tools=tools,
            quiet=quiet,
            reasoning_level=reasoning_level,
        )
    elif provider == "openai_compat":
        from src.llm.chat_completions_llm import (
            CHEAP_LLM_MODEL_NAME as COMPAT_CHEAP_MODEL,
            ChatCompletionsLLM,
        )

        return ChatCompletionsLLM(
            model=model or COMPAT_CHEAP_MODEL,
            tools=tools,
            quiet=quiet,
            reasoning_level=reasoning_level,
        )
    else:
        raise ValueError(
            f"Unsupported LLM provider: {provider}. "
            "Supported providers: openai, anthropic, openai_compat"
        )
