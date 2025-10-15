"""
Anthropic Claude Provider.
"""

import json
import logging
from typing import AsyncIterator, Optional, List, Dict, Any

from anthropic import AsyncAnthropic, AsyncStream, MessageStreamEvent

from . import LLMProvider

logger = logging.getLogger(__name__)


class AnthropicProvider(LLMProvider):
    """Provider for Anthropic Claude API."""

    def __init__(self, api_key: str, model: str):
        self.client = AsyncAnthropic(api_key=api_key)
        self.model = model

    async def stream_response(
        self,
        prompt: str,
        max_tokens: Optional[int] = None,
        temperature: Optional[float] = None,
        top_p: Optional[float] = None,
        tools: Optional[List[Dict[str, Any]]] = None,
        messages: Optional[List[Dict[str, Any]]] = None,
    ) -> AsyncIterator[MessageStreamEvent]:
        """Stream response from Anthropic Claude API."""
        try:
            # Use provided messages or create from prompt
            msg_list = messages or [{"role": "user", "content": prompt}]

            # Prepare request parameters
            request_params = {
                "model": self.model,
                "messages": msg_list,
                "max_tokens": max_tokens or 4096,
                "temperature": temperature or 0.7,
                "top_p": top_p or 0.9,
                "stream": True,
            }

            # Add tools if provided
            if tools:
                request_params["tools"] = tools
                logger.info(f"[ANTHROPIC] Sending request with {len(tools)} tools: {[t['name'] for t in tools]}")
            else:
                logger.info(f"[ANTHROPIC] Sending request without tools")

            logger.info(f"[ANTHROPIC] Model: {self.model}, Messages: {len(msg_list)}, Max tokens: {request_params['max_tokens']}")
            logger.debug(f"[ANTHROPIC] Full request params: {json.dumps({k: v for k, v in request_params.items() if k != 'messages'}, indent=2)}")
            logger.debug(f"[ANTHROPIC] Messages: {json.dumps(msg_list, indent=2)}")

            stream: AsyncStream[MessageStreamEvent] = await self.client.messages.create(**request_params)
            logger.info(f"[ANTHROPIC] Stream created successfully, starting to process events")

            event_count = 0
            async for event in stream:
                event_count += 1
                logger.debug(f"[ANTHROPIC] Event {event_count}: {event.type}")
                if event.type == 'content_block_start':
                    logger.info(f"[ANTHROPIC] Content block start: type={event.content_block.type}")
                    if event.content_block.type == 'tool_use':
                        logger.info(f"[ANTHROPIC] Tool use started: {event.content_block.name} (id: {event.content_block.id}) (input: {json.dumps(event.content_block.input)})")
                elif event.type == 'content_block_delta':
                    if event.delta.type == 'text_delta':
                        logger.debug(f"[ANTHROPIC] Text delta: '{event.delta.text}'")
                    elif event.delta.type == 'input_json_delta':
                        logger.debug(f"[ANTHROPIC] JSON delta: {event.delta.partial_json}")
                elif event.type == 'citation':
                    logger.info(f"[ANTHROPIC] Citation: {event.citation}")
                elif event.type == 'content_block_stop':
                    logger.info(f"[ANTHROPIC] Content block stop at index {getattr(event, 'index', '<unknown>')}")
                elif event.type == 'message_delta':
                    logger.info(f"[ANTHROPIC] Message delta stop reason: {event.delta.stop_reason}")
                elif event.type == 'message_stop':
                    logger.info(f"[ANTHROPIC] Message completed after {event_count} events")

                yield event

        except Exception as e:
            logger.error(f"[ANTHROPIC] Failed to stream from Anthropic: {str(e)}", exc_info=True)

    async def generate_response(
        self,
        prompt: str,
        max_tokens: Optional[int] = None,
        temperature: Optional[float] = None,
        top_p: Optional[float] = None,
    ) -> str:
        """Generate non-streaming response from Anthropic Claude API."""
        try:
            response = await self.client.messages.create(
                model=self.model,
                messages=[{"role": "user", "content": prompt}],
                max_tokens=max_tokens or 4096,
                temperature=temperature or 0.7,
                top_p=top_p or 0.9,
                stream=False,
            )

            # Extract text content from response
            content = ""
            for block in response.content:
                if hasattr(block, "text"):
                    content += block.text

            return content

        except Exception as e:
            logger.error(f"Failed to generate response from Anthropic: {str(e)}")
            raise Exception(f"Failed to generate response: {str(e)}")

    async def health_check(self) -> bool:
        """Check if Anthropic API is accessible."""
        try:
            # Try a minimal request to check API accessibility
            response = await self.client.messages.create(
                model=self.model,
                messages=[{"role": "user", "content": "Hello"}],
                max_tokens=1,
                stream=False,
            )
            return True
        except Exception:
            return False

