"""Prompt endpoints."""

import logging

from fastapi import APIRouter, HTTPException, Request
from fastapi.responses import StreamingResponse

from schemas import PromptRequest, PromptResponse
from providers import LLMProvider
from provider_cache import ResolvedModel

logger = logging.getLogger(__name__)
router = APIRouter(tags=["prompts"])


async def _get_default_llm_provider(request: Request) -> ResolvedModel | None:
    """Return the default LLM provider from the database."""
    cache = getattr(request.app.state, "provider_cache", None)
    if cache is None:
        return None
    return await cache.resolve_default()


async def _get_secondary_llm_provider(request: Request) -> ResolvedModel | None:
    """Return the secondary (lightweight) LLM provider, falling back to default."""
    cache = getattr(request.app.state, "provider_cache", None)
    if cache is None:
        return None
    return await cache.resolve_secondary_or_default()


@router.post("/prompt")
async def generate_response(request: Request, body: PromptRequest):
    """Generate a response from the configured LLM provider with streaming support."""
    resolved = await _get_default_llm_provider(request)
    if not resolved:
        raise HTTPException(status_code=500, detail="LLM provider not initialized")
    llm_provider = resolved.provider
    logger.info(
        f"Generating response for prompt: {body.prompt[:50]}... (stream={body.stream})"
    )

    if not body.stream:
        # Non-streaming response (keep for backward compatibility)
        return await _generate_non_streaming_response(request, body)

    # Streaming response
    async def stream_generator():
        try:
            async for event in llm_provider.stream_response(
                body.prompt,
                max_tokens=body.max_tokens,
                model=resolved.model_name,
            ):
                # Extract text content from MessageStreamEvent
                if event.type == "content_block_delta":
                    if event.delta.text:
                        yield event.delta.text
        except Exception as e:
            logger.error(f"Failed to generate streaming response: {str(e)}")
            return

    return StreamingResponse(
        stream_generator(),
        media_type="text/plain",
        headers={"Cache-Control": "no-cache", "Connection": "keep-alive"},
    )


async def _generate_non_streaming_response(
    request: Request, body: PromptRequest
) -> PromptResponse:
    """Generate non-streaming response using the secondary (lightweight) model."""
    resolved = await _get_secondary_llm_provider(request)
    if not resolved:
        raise HTTPException(status_code=500, detail="LLM provider not initialized")
    llm_provider = resolved.provider
    try:
        generated_text, _ = await llm_provider.generate_response(
            body.prompt,
            max_tokens=body.max_tokens,
            model=resolved.model_name,
        )

        logger.info(f"Successfully generated response of length: {len(generated_text)}")
        return PromptResponse(response=generated_text)

    except Exception as e:
        logger.error(f"Failed to generate response: {str(e)}")
        raise HTTPException(
            status_code=500, detail=f"Failed to generate response: {str(e)}"
        )
