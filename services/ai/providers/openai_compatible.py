"""
OpenAI-compatible provider — talks to any endpoint that implements the OpenAI
Chat Completions API (vLLM, Ollama, LM Studio, LiteLLM, OpenRouter, etc.).

Uses the OpenAI SDK with a custom base_url, giving us full tool/function-calling
support without provider-specific glue.
"""

import json
import logging
import time
from collections.abc import AsyncIterator, Mapping
from typing import Any, ClassVar, cast
from urllib.parse import urlsplit

import httpx
from openai import APIStatusError, AsyncOpenAI
from openai.types.chat import (
    ChatCompletionAssistantMessageParam,
    ChatCompletionChunk,
    ChatCompletionMessageParam,
    ChatCompletionMessageToolCallParam,
    ChatCompletionSystemMessageParam,
    ChatCompletionToolMessageParam,
    ChatCompletionToolParam,
    ChatCompletionUserMessageParam,
)
from openai.types.chat.chat_completion_content_part_image_param import (
    ChatCompletionContentPartImageParam,
    ImageURL,
)
from openai.types.chat.chat_completion_content_part_text_param import (
    ChatCompletionContentPartTextParam,
)
from openai.types.chat.chat_completion_message_tool_call_param import Function
from anthropic.types import (
    DocumentBlockParam,
    ImageBlockParam,
    Message,
    MessageDeltaUsage,
    MessageParam,
    TextBlockParam,
    ToolParam,
    ToolResultBlockParam,
    ToolUseBlockParam,
    Usage,
    RawMessageStartEvent,
    RawMessageDeltaEvent,
    RawContentBlockStartEvent,
    RawContentBlockDeltaEvent,
    RawContentBlockStopEvent,
    RawMessageStopEvent,
    ToolUseBlock,
    TextBlock,
    TextDelta,
    InputJSONDelta,
)
from anthropic.types.message_stream_event import MessageStreamEvent
from anthropic.types.raw_message_delta_event import Delta

from . import LLMProvider, LLMProviderEmptyResponseError, TokenUsage
from .anthropic_message_adapter import extract_text_document
from .types import ProviderError, ProviderType


def _openai_compat_status_code(e: BaseException) -> int | None:
    return e.status_code if isinstance(e, APIStatusError) else None


def _openai_compat_error_code(e: BaseException) -> str | None:
    code = getattr(e, "code", None)
    if isinstance(code, str):
        return code
    body = getattr(e, "body", None)
    if isinstance(body, dict):
        error = body.get("error")
        if isinstance(error, dict):
            body_code = error.get("code")
            if isinstance(body_code, str):
                return body_code
    return None


def _openai_compat_context_overflow(e: BaseException) -> bool:
    return _openai_compat_error_code(e) == "context_length_exceeded"


def _image_data_source(block: Mapping[str, Any]) -> tuple[str, str] | None:
    """Return (media_type, base64_data) for a base64-sourced Anthropic image block."""
    source = block.get("source")
    if not isinstance(source, dict) or source.get("type") != "base64":
        return None
    media_type = source.get("media_type")
    data = source.get("data")
    if not isinstance(media_type, str) or not isinstance(data, str):
        return None
    return media_type, data


logger = logging.getLogger(__name__)

# --- Endpoint vision-capability metadata ---------------------------------
#
# Resolution order for image support on OpenAI-compatible endpoints:
#
# 1. Host metadata: OpenRouter publishes per-model input modalities on its
#    public catalog, Ollama reports model capabilities on /api/show.
# 2. Live probe: a one-shot chat request carrying a tiny 1x1 PNG. Any
#    completion means the model accepts images; a 4xx rejection that clearly
#    blames the image means it does not; auth/quota/model-not-found errors
#    stay unknown (vision stays off). Results are cached in-process.
# Connections can always force an answer with visionMode: on/off.

_PROBE_TIMEOUT_S = 3.0
_OPENROUTER_CATALOG_TTL_S = 6 * 3600
_OLLAMA_RESULT_TTL_S = 3600
_LIVE_PROBE_TIMEOUT_S = 15.0
_LIVE_PROBE_TTL_S = 7 * 24 * 3600
_LIVE_PROBE_UNKNOWN_TTL_S = 300
_openrouter_catalog_cache: dict[str, tuple[float, frozenset[str], frozenset[str]]] = {}
_ollama_vision_cache: dict[str, tuple[float, bool]] = {}
_live_vision_cache: dict[str, tuple[float, bool | None]] = {}

# 1x1 transparent PNG.
_PROBE_PNG_BASE64 = (
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJ"
    "AAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=="
)
_IMAGE_UNSUPPORTED_MARKERS = ("image", "vision", "multimodal", "modalit")


def _endpoint_origin(base_url: str) -> str:
    parts = urlsplit(base_url)
    return f"{parts.scheme}://{parts.netloc}"


def _is_openrouter(base_url: str) -> bool:
    host = (urlsplit(base_url).hostname or "").lower()
    return host == "openrouter.ai" or host.endswith(".openrouter.ai")


def _is_ollama(base_url: str) -> bool:
    return urlsplit(base_url).port == 11434


def _vision_and_all_ids_from_catalog(
    payload: object,
) -> tuple[frozenset[str], frozenset[str]]:
    """(ids accepting images, all catalog ids) from an OpenRouter catalog payload."""
    if not isinstance(payload, dict) or not isinstance(payload.get("data"), list):
        return frozenset(), frozenset()
    vision: set[str] = set()
    all_ids: set[str] = set()
    for entry in payload["data"]:
        if not isinstance(entry, dict):
            continue
        model_id = entry.get("id")
        if not isinstance(model_id, str):
            continue
        all_ids.add(model_id)
        architecture = entry.get("architecture")
        if not isinstance(architecture, dict):
            continue
        modalities = architecture.get("input_modalities")
        if isinstance(modalities, list) and "image" in modalities:
            vision.add(model_id)
    return frozenset(vision), frozenset(all_ids)


def _catalog_supports_images(
    vision_ids: frozenset[str], all_ids: frozenset[str], model_id: str
) -> bool | None:
    """Tri-state catalog answer: True/False when the id is known, else None."""
    if model_id in vision_ids:
        return True
    base = model_id.split(":", 1)[0]
    if base != model_id and base in vision_ids:
        return True
    if model_id in all_ids or (base != model_id and base in all_ids):
        return False
    return None


def _ollama_supports_vision(payload: object) -> bool:
    capabilities = payload.get("capabilities") if isinstance(payload, dict) else None
    return isinstance(capabilities, list) and "vision" in capabilities


def _probe_error_mentions_images(e: BaseException) -> bool:
    """True when a 4xx rejection clearly blames the image input."""
    status = _openai_compat_status_code(e)
    if status is None or status >= 500:
        return False
    text = str(e).lower()
    body = getattr(e, "body", None)
    if isinstance(body, dict):
        error = body.get("error")
        if isinstance(error, dict) and isinstance(error.get("message"), str):
            text += " " + error["message"].lower()
        elif isinstance(error, str):
            text += " " + error.lower()
    return any(marker in text for marker in _IMAGE_UNSUPPORTED_MARKERS)


async def _openrouter_supports_images(base_url: str, model_id: str) -> bool | None:
    origin = _endpoint_origin(base_url)
    now = time.monotonic()
    cached = _openrouter_catalog_cache.get(origin)
    if cached is not None and cached[0] >= now:
        vision_ids, all_ids = cached[1], cached[2]
    else:
        try:
            async with httpx.AsyncClient(timeout=_PROBE_TIMEOUT_S) as client:
                resp = await client.get(f"{origin}/api/v1/models")
                resp.raise_for_status()
            vision_ids, all_ids = _vision_and_all_ids_from_catalog(resp.json())
        except Exception as e:
            logger.info("OpenRouter vision catalog probe failed for %s: %s", origin, e)
            return None
        _openrouter_catalog_cache[origin] = (
            now + _OPENROUTER_CATALOG_TTL_S,
            vision_ids,
            all_ids,
        )
    return _catalog_supports_images(vision_ids, all_ids, model_id)


async def _ollama_model_supports_vision(base_url: str, model_id: str) -> bool | None:
    origin = _endpoint_origin(base_url)
    cache_key = f"{origin}|{model_id}"
    now = time.monotonic()
    cached = _ollama_vision_cache.get(cache_key)
    if cached is not None and cached[0] >= now:
        return cached[1]
    try:
        async with httpx.AsyncClient(timeout=_PROBE_TIMEOUT_S) as client:
            resp = await client.post(f"{origin}/api/show", json={"model": model_id})
            resp.raise_for_status()
        result = _ollama_supports_vision(resp.json())
    except Exception as e:
        logger.info("Ollama vision capability probe failed for %s: %s", cache_key, e)
        return None
    _ollama_vision_cache[cache_key] = (now + _OLLAMA_RESULT_TTL_S, result)
    return result


# Some OpenAI-compatible providers expose non-standard assistant-message fields
# that must be round-tripped in later requests. Keep this as a narrow allowlist:
# only fields we have actually received from a provider are persisted/sent back.
REASONING_CONTENT_KEY = "reasoning_content"
ASSISTANT_MESSAGE_PASSTHROUGH_KEYS = (REASONING_CONTENT_KEY,)


def _get_passthrough_delta_value(delta: object, key: str) -> str | None:
    value = getattr(delta, key, None)
    if isinstance(value, str):
        return value

    model_extra = getattr(delta, "model_extra", None)
    if isinstance(model_extra, dict):
        extra_value = model_extra.get(key)
        if isinstance(extra_value, str):
            return extra_value

    return None


def _convert_tools_to_openai(tools: list[ToolParam]) -> list[ChatCompletionToolParam]:
    """Convert Anthropic tool schema to OpenAI Chat Completions function-calling format."""
    result: list[ChatCompletionToolParam] = []
    for tool in tools:
        result.append(
            ChatCompletionToolParam(
                type="function",
                function={
                    "name": tool["name"],
                    "description": tool.get("description", ""),
                    "parameters": cast(dict[str, object], tool["input_schema"]),
                },
            )
        )
    return result


def _convert_messages_to_openai(
    messages: list[MessageParam],
) -> list[ChatCompletionMessageParam]:
    """Convert Anthropic-style messages to OpenAI Chat Completions format."""
    result: list[ChatCompletionMessageParam] = []

    for msg in messages:
        role = msg["role"]
        content = msg.get("content", "")

        if isinstance(content, str):
            if role == "assistant":
                result.append(
                    ChatCompletionAssistantMessageParam(
                        role="assistant", content=content
                    )
                )
            else:
                result.append(
                    ChatCompletionUserMessageParam(role="user", content=content)
                )
            continue

        if not isinstance(content, list):
            if role == "assistant":
                result.append(
                    ChatCompletionAssistantMessageParam(
                        role="assistant", content=str(content)
                    )
                )
            else:
                result.append(
                    ChatCompletionUserMessageParam(role="user", content=str(content))
                )
            continue

        # Handle block-based content (Anthropic format)
        text_parts: list[str] = []
        user_content_parts: list[
            ChatCompletionContentPartTextParam | ChatCompletionContentPartImageParam
        ] = []
        tool_calls: list[ChatCompletionMessageToolCallParam] = []
        tool_results: list[ChatCompletionToolMessageParam] = []

        reasoning_content: str | None = None

        for block in content:
            if not isinstance(block, dict):
                continue

            block = cast(
                DocumentBlockParam
                | ImageBlockParam
                | TextBlockParam
                | ToolUseBlockParam
                | ToolResultBlockParam,
                block,
            )
            block_reasoning_content = block.get(REASONING_CONTENT_KEY)
            if isinstance(block_reasoning_content, str):
                reasoning_content = block_reasoning_content

            if block["type"] == "text":
                block = cast(TextBlockParam, block)
                if block["text"]:
                    text_parts.append(block["text"])
                    if role == "user":
                        user_content_parts.append(
                            ChatCompletionContentPartTextParam(
                                type="text", text=block["text"]
                            )
                        )
            elif block["type"] == "document" and role == "user":
                document_text = extract_text_document(block)
                if document_text is not None:
                    text_parts.append(document_text)
                    user_content_parts.append(
                        ChatCompletionContentPartTextParam(
                            type="text", text=document_text
                        )
                    )
            elif block["type"] == "image" and role == "user":
                image_data = _image_data_source(block)
                if image_data is not None:
                    media_type, data = image_data
                    user_content_parts.append(
                        ChatCompletionContentPartImageParam(
                            type="image_url",
                            image_url=ImageURL(
                                url=f"data:{media_type};base64,{data}"
                            ),
                        )
                    )
            elif block["type"] == "tool_use":
                block = cast(ToolUseBlockParam, block)
                raw_input = block["input"]
                tool_calls.append(
                    ChatCompletionMessageToolCallParam(
                        id=block["id"],
                        type="function",
                        function=Function(
                            name=block["name"],
                            arguments=(
                                json.dumps(raw_input)
                                if isinstance(raw_input, dict)
                                else str(raw_input)
                            ),
                        ),
                    )
                )
            elif block["type"] == "tool_result":
                block = cast(ToolResultBlockParam, block)
                result_content = block.get("content", "")
                if isinstance(result_content, list):
                    parts: list[str] = []
                    for rb in result_content:
                        if not isinstance(rb, dict):
                            continue
                        if rb.get("type") == "text":
                            rb = cast(TextBlockParam, rb)
                            if rb["text"]:
                                parts.append(rb["text"])
                        elif rb.get("type") == "search_result":
                            title = rb.get("title", "")
                            source = rb.get("source", "")
                            inner = rb.get("content", [])
                            inner_text = "\n".join(
                                ib["text"]
                                for ib in inner
                                if isinstance(ib, dict) and ib.get("type") == "text"
                            )
                            parts.append(f"[{title}]({source})\n{inner_text}")
                    result_content = "\n\n".join(parts)
                tool_results.append(
                    ChatCompletionToolMessageParam(
                        role="tool",
                        tool_call_id=block["tool_use_id"],
                        content=str(result_content),
                    )
                )

        if role == "assistant":
            assistant_msg = ChatCompletionAssistantMessageParam(role="assistant")
            if text_parts:
                assistant_msg["content"] = "\n".join(text_parts)
            if tool_calls:
                assistant_msg["tool_calls"] = tool_calls
            if reasoning_content:
                # OpenAI-compatible provider extension: DeepSeek thinking mode,
                # for example, requires this exact field to be echoed back.
                assistant_msg[REASONING_CONTENT_KEY] = reasoning_content  # type: ignore[typeddict-unknown-key]
            result.append(assistant_msg)
        elif role == "user" and tool_results:
            result.extend(tool_results)
        else:
            has_images = any(
                part["type"] == "image_url" for part in user_content_parts
            )
            if has_images:
                result.append(
                    ChatCompletionUserMessageParam(
                        role="user",
                        content=cast(Any, user_content_parts),
                    )
                )
            elif text_parts:
                result.append(
                    ChatCompletionUserMessageParam(
                        role="user", content="\n".join(text_parts)
                    )
                )

    return result


def validate_openai_tool_message_sequence(
    messages: list[ChatCompletionMessageParam],
) -> None:
    """Validate every assistant ``tool_calls`` batch is fully answered by ``tool``
    messages before any non-tool message, and each ``tool`` message responds
    exactly once to a call in the nearest preceding assistant batch.

    Strict OpenAI-compatible endpoints (DeepSeek et al.) reject malformed
    sequences with an opaque 400; run this pre-dispatch so violations surface as
    a clear error naming the offending message instead.
    """
    pending_tool_call_ids: set[str] = set()
    for index, message in enumerate(messages):
        role = message.get("role")
        if role == "assistant":
            if pending_tool_call_ids:
                raise ValueError(
                    f"Invalid assistant message at index {index}: tool_calls "
                    f"{sorted(pending_tool_call_ids)} from the preceding assistant "
                    "message were never answered with tool messages"
                )
            pending_tool_call_ids = {
                tool_call["id"] for tool_call in message.get("tool_calls") or []
            }
            continue
        if role == "tool":
            tool_call_id = message.get("tool_call_id")
            if tool_call_id is None or tool_call_id not in pending_tool_call_ids:
                raise ValueError(
                    f"Invalid tool message at index {index}: tool_call_id={tool_call_id!r} "
                    "has no matching tool_calls entry in the preceding assistant message"
                )
            pending_tool_call_ids.discard(tool_call_id)
            continue
        # Any non-tool message (system/user/developer) closes the current batch:
        # every tool call of the preceding assistant message must already have
        # been answered, and later tool messages can no longer pair to it.
        if pending_tool_call_ids:
            raise ValueError(
                f"Invalid {role} message at index {index}: tool_calls "
                f"{sorted(pending_tool_call_ids)} from the preceding assistant "
                "message were never answered with tool messages"
            )
    if pending_tool_call_ids:
        raise ValueError(
            f"Invalid trailing messages: tool_calls {sorted(pending_tool_call_ids)} "
            "from the final assistant message were never answered with tool messages"
        )


class OpenAICompatibleProvider(LLMProvider):
    """Provider for any OpenAI-compatible Chat Completions endpoint.

    Uses the OpenAI SDK pointed at a user-supplied base URL, giving us Chat
    Completions with full tool/function-calling support.
    """

    provider_type: ClassVar[ProviderType] = ProviderType.OPENAI_COMPATIBLE
    PERSISTED_BLOCK_EXTRAS = ASSISTANT_MESSAGE_PASSTHROUGH_KEYS

    def __init__(
        self, base_url: str, api_key: str | None = None, model: str = "default"
    ):
        self.base_url = base_url.rstrip("/").removesuffix("/v1")
        self.api_key = api_key
        self.model = model
        self.model_name = model
        # Some keyless local endpoints (vLLM without --api-key, Ollama, etc.)
        # still require the SDK to send *something* — fall back to a placeholder.
        self.client = AsyncOpenAI(
            api_key=api_key or "unused",
            base_url=f"{self.base_url}/v1",
        )

    async def supports_vision(self, model_id: str) -> bool | None:
        """Ask the endpoint whether the model accepts image inputs.

        Host metadata first (OpenRouter catalog, Ollama), then a one-shot
        live image probe. Returns None only when the endpoint cannot answer
        (auth/quota/infra/model-not-found); callers must treat None as not
        vision-capable unless overridden.
        """
        base_url = self.base_url or ""
        if _is_openrouter(base_url):
            answer = await _openrouter_supports_images(base_url, model_id)
            if answer is not None:
                return answer
        elif _is_ollama(base_url):
            answer = await _ollama_model_supports_vision(base_url, model_id)
            if answer is not None:
                return answer
        return await self._probe_live_vision(model_id)

    async def _probe_live_vision(self, model_id: str) -> bool | None:
        """Send a tiny image ping and interpret the outcome.

        Any completion proves the model accepts images. A 4xx that clearly
        blames the image proves it does not. Everything else (auth, quota,
        model not found, connection trouble) stays unknown.
        """
        cache_key = f"{self.base_url}|{model_id}"
        now = time.monotonic()
        cached = _live_vision_cache.get(cache_key)
        if cached is not None and cached[0] >= now:
            return cached[1]
        try:
            await self.client.chat.completions.create(
                model=model_id,
                messages=[
                    {
                        "role": "user",
                        "content": [
                            {"type": "text", "text": "."},
                            {
                                "type": "image_url",
                                "image_url": {
                                    "url": f"data:image/png;base64,{_PROBE_PNG_BASE64}"
                                },
                            },
                        ],
                    }
                ],
                max_tokens=1,
                stream=False,
                timeout=_LIVE_PROBE_TIMEOUT_S,
            )
        except Exception as e:
            result: bool | None = (
                False if _probe_error_mentions_images(e) else None
            )
            logger.info(
                "Live vision probe for %s on %s -> %s (%s)",
                model_id,
                self.base_url,
                result,
                e,
            )
        else:
            result = True
        ttl = (
            _LIVE_PROBE_TTL_S
            if result is not None
            else _LIVE_PROBE_UNKNOWN_TTL_S
        )
        _live_vision_cache[cache_key] = (now + ttl, result)
        return result

    async def stream_response(
        self,
        prompt: str,
        max_tokens: int | None = None,
        tools: list[ToolParam] | None = None,
        messages: list[MessageParam] | None = None,
        system_prompt: str | None = None,
        *,
        model: str | None = None,
    ) -> AsyncIterator[MessageStreamEvent]:
        """Stream response, yielding Anthropic-compatible MessageStreamEvents."""
        try:
            active_model = model or self.model
            openai_messages = _convert_messages_to_openai(
                messages or [{"role": "user", "content": prompt}]
            )

            if system_prompt:
                system_msg = ChatCompletionSystemMessageParam(
                    role="system", content=system_prompt
                )
                openai_messages = [system_msg] + openai_messages

            params: dict[str, Any] = {
                "model": active_model,
                "messages": openai_messages,
                "max_tokens": max_tokens or 4096,
                "stream": True,
                "stream_options": {"include_usage": True},
            }

            if tools:
                params["tools"] = _convert_tools_to_openai(tools)
                logger.info(
                    f"Sending request with {len(tools)} tools: {[t['name'] for t in tools]}"
                )

            validate_openai_tool_message_sequence(openai_messages)

            stream = await self.client.chat.completions.create(**params)

            # Emit message_start
            yield RawMessageStartEvent(
                type="message_start",
                message=Message(
                    id=f"openai-compat-{time.time_ns()}",
                    type="message",
                    role="assistant",
                    content=[],
                    model=active_model,
                    usage=Usage(input_tokens=0, output_tokens=0),
                ),
            )

            text_started = False
            current_text_index = 0
            # tool_call index (from OpenAI) -> our content block index
            tool_block_indices: dict[int, int] = {}
            tool_call_ids: dict[int, str] = {}
            tool_call_names: dict[int, str] = {}
            next_block_index = 0

            stream_input_tokens = 0
            stream_output_tokens = 0
            reasoning_content_parts: list[str] = []

            chunk: ChatCompletionChunk
            async for chunk in stream:
                # Usage-only chunk (no choices) arrives at end of stream
                if chunk.usage:
                    stream_input_tokens = chunk.usage.prompt_tokens or 0
                    stream_output_tokens = chunk.usage.completion_tokens or 0

                if not chunk.choices:
                    continue

                delta = chunk.choices[0].delta
                reasoning_content = _get_passthrough_delta_value(
                    delta, REASONING_CONTENT_KEY
                )
                if reasoning_content:
                    reasoning_content_parts.append(reasoning_content)

                # Handle text content
                if delta.content:
                    if not text_started:
                        current_text_index = next_block_index
                        next_block_index += 1
                        text_started = True
                        yield RawContentBlockStartEvent(
                            type="content_block_start",
                            index=current_text_index,
                            content_block=TextBlock(type="text", text=""),
                        )
                    yield RawContentBlockDeltaEvent(
                        type="content_block_delta",
                        index=current_text_index,
                        delta=TextDelta(type="text_delta", text=delta.content),
                    )

                # Handle tool calls
                if delta.tool_calls:
                    for tc_delta in delta.tool_calls:
                        tc_index = tc_delta.index

                        # New tool call — emit content_block_start
                        if tc_index not in tool_block_indices:
                            # Close text block if open
                            if text_started:
                                yield RawContentBlockStopEvent(
                                    type="content_block_stop",
                                    index=current_text_index,
                                )
                                text_started = False

                            block_index = next_block_index
                            next_block_index += 1
                            tool_block_indices[tc_index] = block_index

                            call_id = tc_delta.id or f"call_{tc_index}"
                            tool_call_ids[tc_index] = call_id
                            name = (
                                tc_delta.function.name
                                if tc_delta.function and tc_delta.function.name
                                else ""
                            )
                            tool_call_names[tc_index] = name

                            yield RawContentBlockStartEvent(
                                type="content_block_start",
                                index=block_index,
                                content_block=ToolUseBlock(
                                    type="tool_use",
                                    id=call_id,
                                    name=name,
                                    input={},
                                ),
                            )

                        # Argument deltas
                        if tc_delta.function and tc_delta.function.arguments:
                            yield RawContentBlockDeltaEvent(
                                type="content_block_delta",
                                index=tool_block_indices[tc_index],
                                delta=InputJSONDelta(
                                    type="input_json_delta",
                                    partial_json=tc_delta.function.arguments,
                                ),
                            )

                # Handle finish_reason
                if chunk.choices[0].finish_reason is not None:
                    # Continue through the provider's final usage-only chunk.
                    continue

            # Close any open blocks
            if text_started:
                yield RawContentBlockStopEvent(
                    type="content_block_stop",
                    index=current_text_index,
                )
            for tc_index, block_index in tool_block_indices.items():
                yield RawContentBlockStopEvent(
                    type="content_block_stop",
                    index=block_index,
                )

            reasoning_content = "".join(reasoning_content_parts)
            if reasoning_content:
                reasoning_block_index = next_block_index
                next_block_index += 1
                reasoning_block_kwargs: dict[str, Any] = {
                    "type": "text",
                    "text": "",
                    REASONING_CONTENT_KEY: reasoning_content,
                }
                yield RawContentBlockStartEvent(
                    type="content_block_start",
                    index=reasoning_block_index,
                    content_block=TextBlock(**reasoning_block_kwargs),
                )
                yield RawContentBlockStopEvent(
                    type="content_block_stop",
                    index=reasoning_block_index,
                )

            if stream_input_tokens or stream_output_tokens:
                yield RawMessageDeltaEvent(
                    type="message_delta",
                    delta=Delta(stop_reason="end_turn"),
                    usage=MessageDeltaUsage(
                        input_tokens=stream_input_tokens,
                        output_tokens=stream_output_tokens,
                    ),
                )

            yield RawMessageStopEvent(type="message_stop")

        except Exception as e:
            logger.error(
                f"Failed to stream from OpenAI-compatible endpoint: {e}",
                exc_info=True,
            )
            raise ProviderError(
                str(e),
                provider_type=self.provider_type,
                model=model or self.model_name,
                status_code=_openai_compat_status_code(e),
                cause=e,
                is_context_overflow=_openai_compat_context_overflow(e),
            ) from e

    async def generate_response(
        self,
        prompt: str,
        max_tokens: int | None = None,
        *,
        model: str | None = None,
    ) -> tuple[str, TokenUsage]:
        """Generate non-streaming response."""
        try:
            active_model = model or self.model
            params: dict[str, Any] = {
                "model": active_model,
                "messages": [{"role": "user", "content": prompt}],
                "max_tokens": max_tokens or 4096,
                "stream": False,
            }

            response = await self.client.chat.completions.create(**params)

            usage = TokenUsage()
            if response.usage:
                usage = TokenUsage(
                    input_tokens=response.usage.prompt_tokens or 0,
                    output_tokens=response.usage.completion_tokens or 0,
                )

            choices = getattr(response, "choices", None) or []
            if not choices:
                raise LLMProviderEmptyResponseError(
                    "OpenAI-compatible endpoint returned no choices"
                )

            message = getattr(choices[0], "message", None)
            content = getattr(message, "content", None) if message else None
            if not content:
                raise LLMProviderEmptyResponseError(
                    "OpenAI-compatible endpoint returned an empty message"
                )

            return content, usage

        except LLMProviderEmptyResponseError:
            raise
        except Exception as e:
            logger.error(
                f"Failed to generate response from OpenAI-compatible endpoint: {e}"
            )
            raise ProviderError(
                str(e),
                provider_type=self.provider_type,
                model=model or self.model_name,
                status_code=_openai_compat_status_code(e),
                cause=e,
                is_context_overflow=_openai_compat_context_overflow(e),
            ) from e

    async def health_check(
        self,
        *,
        model: str | None = None,
    ) -> bool:
        """Liveness is determined by inference calls themselves; no separate probe
        since not every OpenAI-compatible server exposes a common health endpoint."""
        return True
