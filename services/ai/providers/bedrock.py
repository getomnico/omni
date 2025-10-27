"""
AWS Bedrock Provider for Claude models.
"""

import time
import json
import logging
from typing import AsyncIterator, Optional, List, Dict, Any, cast

import boto3
from botocore.exceptions import ClientError
from anthropic.types import (
    MessageParam,
    Message,
    Usage,
    RawMessageStartEvent,
    RawContentBlockStartEvent,
    RawContentBlockDeltaEvent,
    RawContentBlockStopEvent,
    RawMessageStopEvent,
    ContentBlockDeltaEvent,
    ContentBlockStartEvent,
    ContentBlockStopEvent,
    MessageDeltaEvent,
    ToolUseBlock,
    TextBlock,
    TextDelta,
    InputJSONDelta,
    CitationsDelta,
    CitationCharLocation,
    CitationPageLocation,
    CitationContentBlockLocation,
)
from anthropic.types.message_stream_event import MessageStreamEvent
from anthropic import AnthropicBedrock

from . import LLMProvider

logger = logging.getLogger(__name__)


def sanitize_document_name(name: str) -> str:
    """
    Sanitize document name for AWS Bedrock requirements.

    AWS Bedrock rules:
    - Only alphanumeric characters, whitespace, hyphens, parentheses, and square brackets allowed
    - No more than one consecutive whitespace character

    Args:
        name: The original document name

    Returns:
        Sanitized document name that meets AWS Bedrock requirements
    """
    import re

    if not name:
        return "untitled document"

    # Replace any character that's not alphanumeric, whitespace, hyphen, parentheses, or square brackets
    # with a space (spaces are allowed, underscores are not)
    sanitized = re.sub(r'[^\w\s\-\(\)\[\]]', ' ', name)

    # Replace multiple consecutive whitespace characters with a single space
    sanitized = re.sub(r'\s+', ' ', sanitized)

    # Trim leading/trailing whitespace
    sanitized = sanitized.strip()

    # If the result is empty after sanitization, provide a default name
    if not sanitized:
        return "untitled document"

    # Cap length at 255 characters to be safe
    if len(sanitized) > 255:
        sanitized = sanitized[:255].rstrip()

    return sanitized


class BedrockProvider(LLMProvider):
    """Provider for AWS Bedrock Claude models."""

    MODEL_FAMILIES = ["anthropic", "amazon"]

    def __init__(self, model_id: str, secondary_model_id: str, region_name: Optional[str] = None):
        self.model_id = model_id
        self.secondary_model_id = secondary_model_id # Smaller, faster model
        self.model_family = self._determine_model_family(model_id)
        self.region_name = region_name

        if self.model_family == "anthropic":
            self.client = AnthropicBedrock()
        else:
            self.client = boto3.client('bedrock-runtime', region_name=region_name)

    def _determine_model_family(self, model_id: str) -> str:
        """Determine the model family from the model ID."""
        for family in self.MODEL_FAMILIES:
            if family in model_id.lower():
                return family
        raise ValueError(f"Unknown model family for model ID: {model_id}")
    
    def _adapt_messages_for_amazon_models(self, messages: list[MessageParam]) -> List[Dict[str, Any]]:
        """Adapt messages to the format expected by Amazon Bedrock models."""
        adapted_messages = []
        for msg in messages:
            content = msg.get('content', [])
            if isinstance(content, str):
                adapted_messages.append({
                    "role": msg["role"],
                    "content": [{"text": content}]
                })
            else:
                adapted_blocks = []
                for block in content:
                    if isinstance(block, dict):
                        # Handle TextBlockParam, ToolUseBlockParam, ToolResultBlockParam, etc.
                        if block['type'] == 'text':
                            adapted_blocks.append({
                                "text": block["text"]
                            })
                        elif block['type'] == 'tool_use':
                            adapted_blocks.append({
                                "toolUse": {
                                    "toolUseId": block["id"],
                                    "name": block["name"],
                                    "input": block["input"]
                                }
                            })
                        elif block['type'] == 'tool_result':
                            # We only expected SearchResultBlockParam here for now
                            search_result_blocks = []
                            for content_block in block.get('content', []):
                                if isinstance(content_block, str):
                                    search_result_blocks.append({
                                        "text": content_block
                                    })
                                elif isinstance(content_block, dict): 
                                    if content_block['type'] == 'text':
                                        search_result_blocks.append({
                                            "text": content_block['text']
                                        })
                                    elif content_block['type'] == 'search_result':
                                        search_result_blocks.append({
                                            "document": {
                                                "format": "txt", # TODO: map actual format if available
                                                "name": sanitize_document_name(content_block['title']),
                                                "source": {
                                                    "bytes": '\n'.join(p['text'] for p in content_block['content'] if 'text' in p)
                                                },
                                                "citations": {
                                                    "enabled": False # Citations are not supported on tool result documents on Amazon models yet
                                                }
                                            }
                                        })
                            
                            if len(search_result_blocks) == 0:
                                # We need to let the model know that no results were found
                                search_result_blocks.append({
                                    "text": "No search results found."
                                })

                            adapted_blocks.append({
                                "toolResult": {
                                    "toolUseId": block["tool_use_id"],
                                    "content": search_result_blocks[:5] # Amazon does not support more than 5 documenrs in a single request
                                }
                            })
                    else:
                        # Handle all types that come under ContentBlock: TextBlock, ToolUseBlock, etc.
                        if block.type == 'text':
                            adapted_blocks.append({
                                "text": block.text
                            })
                        elif block.type == 'tool_use':
                            adapted_blocks.append({
                                "toolUse": {
                                    "toolUseId": block.id,
                                    "name": block.name,
                                    "input": block.input
                                }
                            })
                
                adapted_messages.append({
                    "role": msg["role"],
                    "content": adapted_blocks
                })

        return adapted_messages
    
    def _dedupe_documents(self, messages: list[dict[str, Any]]):
        """Amazon Bedrock API does not support requests where the same document appears multiple times.

        So we need to deduplicate documents in the messages. We go through the messages in reverse order, keeping track of seen document names in
        tool result blocks.
        If we see a document name again, we remove it from the message content.
        """

        seen_documents = set()
        deduped_messages = []
        for msg in reversed(messages):
            if 'content' in msg:
                deduped_content = []
                for block in msg['content']:
                    if 'toolResult' in block:
                        tool_result = block['toolResult']
                        deduped_tool_result_content = []
                        for content_block in tool_result.get('content', []):
                            if 'document' in content_block:
                                doc_name = content_block['document']['name']
                                if doc_name not in seen_documents:
                                    seen_documents.add(doc_name)
                                    deduped_tool_result_content.append(content_block)
                                else:
                                    logger.debug(f"[BEDROCK-AMAZON] Deduplicating document '{doc_name}' in tool result")
                            else:
                                deduped_tool_result_content.append(content_block)
                        if deduped_tool_result_content:
                            tool_result['content'] = deduped_tool_result_content
                            deduped_content.append(block)
                    else:
                        deduped_content.append(block)
                msg['content'] = deduped_content
            deduped_messages.append(msg)
    
    def _limit_documents(self, messages: list[dict[str, Any]], max_documents: int = 5):
        """Limit the number of documents in tool result blocks to the specified maximum.

        Amazon Bedrock API supports a maximum of 5 documents in total, across all content blocks in all messages
        This function goes through the messages and limits the number of documents accordingly.
        It preferentially keeps documents in the later messages, removing documents from earlier messages first.
        """

        document_count = 0
        for msg in reversed(messages):
            if 'content' in msg:
                limited_content = []
                for block in msg['content']:
                    if 'toolResult' in block:
                        tool_result = block['toolResult']
                        limited_tool_result_content = []
                        for content_block in tool_result.get('content', []):
                            if 'document' in content_block:
                                if document_count < max_documents:
                                    document_count += 1
                                    limited_tool_result_content.append(content_block)
                                else:
                                    logger.debug(f"[BEDROCK-AMAZON] Limiting documents to {max_documents}, removing document '{content_block['document']['name']}'")
                            else:
                                limited_tool_result_content.append(content_block)
                        if limited_tool_result_content:
                            tool_result['content'] = limited_tool_result_content
                            limited_content.append(block)
                    else:
                        limited_content.append(block)

                # If after limiting, the message has no content blocks left, we should leave a text placeholder block
                if not limited_content:
                    limited_content = [{"text": "<Documents removed to meet API limits>"}]

                msg['content'] = limited_content

    def _adapt_tools_for_amazon_models(self, tools: list[Dict[str, Any]]) -> List[Dict[str, Any]]:
        """Adapt tools to the format expected by Amazon Bedrock models."""
        adapted_tools = []
        for tool in tools:
            adapted_tool = {
                "toolSpec": {
                    "name": tool['name'],
                    'description': tool['description'],
                    'inputSchema': {
                        "json": tool['input_schema']
                    }
                }
            }
            adapted_tools.append(adapted_tool)
        return adapted_tools

    def _convert_response_to_anthropic_events(self, event: Dict[str, Any]) -> MessageStreamEvent | None:
        """Convert Bedrock streaming response to Anthropic MessageStreamEvent format."""
        if 'messageStart' in event:
            return RawMessageStartEvent(
                type="message_start",
                message=Message(
                    id=str(time.time_ns()),
                    type="message",
                    role="assistant",
                    content=[],
                    model=self.model_id,
                    usage=Usage(input_tokens=0, output_tokens=0),
                )
            )
        elif 'contentBlockStart' in event:
            content_block_start = event['contentBlockStart']['start']
            return RawContentBlockStartEvent(
                type='content_block_start',
                index=event['contentBlockStart']['contentBlockIndex'],
                content_block=ToolUseBlock(
                    type="tool_use",
                    id=content_block_start['toolUse']['toolUseId'],
                    name=content_block_start['toolUse']['name'],
                    input={},
                ) if 'toolUse' in content_block_start else TextBlock(type='text', text="")
            )
        elif 'contentBlockDelta' in event:
            content_block_delta = event['contentBlockDelta']['delta']
            if 'text' in content_block_delta: 
                return RawContentBlockDeltaEvent(
                    type='content_block_delta',
                    index=event['contentBlockDelta']['contentBlockIndex'],
                    delta=TextDelta(type='text_delta', text=content_block_delta['text'])
                )
            elif 'toolUse' in content_block_delta:
                return RawContentBlockDeltaEvent(
                    type='content_block_delta',
                    index=event['contentBlockDelta']['contentBlockIndex'],
                    delta=InputJSONDelta(type='input_json_delta', partial_json=content_block_delta['toolUse']['input'])
                )
            elif 'citation' in content_block_delta:
                citation_block = content_block_delta['citation']
                citation_loc = citation_block['location']

                citation = None
                if 'documentChar' in citation_loc:
                    citation = CitationCharLocation(
                        type='char_location',
                        document_index=citation_loc['documentChar']['documentIndex'],
                        start_char_index=citation_loc['documentChar']['start'],
                        end_char_index=citation_loc['documentChar']['end'],
                        cited_text=citation_block['sourceContent'][0]['text'], # TODO: handle multiple source contents
                    )
                elif 'documentPage' in citation_loc:
                    citation = CitationPageLocation(
                        type='page_location',
                        document_index=citation_loc['documentPage']['documentIndex'],
                        start_page_number=citation_loc['documentPage']['start'],
                        end_page_number=citation_loc['documentPage']['end'],
                        cited_text=citation_block['sourceContent'][0]['text'], # TODO: handle multiple source contents
                    )
                elif 'documentChunk' in citation_loc:
                    citation = CitationContentBlockLocation(
                        type='content_block_location',
                        document_index=citation_loc['documentChunk']['documentIndex'],
                        start_block_index=citation_loc['documentChunk']['start'],
                        end_block_index=citation_loc['documentChunk']['end'],
                        cited_text=citation_block['sourceContent'][0]['text'], # TODO: handle multiple source contents
                    )

                if citation is None:
                    logger.debug(f"[BEDROCK] Skipping unknown citation location type: {list(citation_loc.keys())}")
                    return RawContentBlockDeltaEvent(
                        type='content_block_delta',
                        index=event['contentBlockDelta']['contentBlockIndex'],
                        delta=TextDelta(type='text_delta', text="")  # Placeholder empty delta
                    )

                return RawContentBlockDeltaEvent(
                    type='content_block_delta',
                    index=event['contentBlockDelta']['contentBlockIndex'],
                    delta=CitationsDelta(type='citations_delta', citation=citation)
                )
        elif 'contentBlockStop' in event:
            content_block_stop = event['contentBlockStop']
            return RawContentBlockStopEvent(
                type='content_block_stop',
                index=content_block_stop['contentBlockIndex'],
            )
        elif 'messageStop' in event:
            return RawMessageStopEvent(type='message_stop')
        
        logger.debug(f"[BEDROCK] Skipping unknown event type: {list(event.keys())}")
        return None

    async def stream_response(
        self,
        prompt: str,
        messages: Optional[List[Dict[str, Any]]] = None,
        tools: Optional[List[Dict[str, Any]]] = None,
        max_tokens: Optional[int] = None,
        temperature: Optional[float] = None,
        top_p: Optional[float] = None,
    ) -> AsyncIterator[MessageStreamEvent]:
        """Stream response from AWS Bedrock models."""
        try:
            if self.model_family == "anthropic":
                msg_list = messages or [{"role": "user", "content": prompt}]

                # Prepare the request body for Claude models
                request_params = {
                    "model": self.model_id,
                    "messages": msg_list,
                    "max_tokens": max_tokens or 4096,
                    "temperature": temperature or 0.7,
                    "stream": True,
                }

                # Add tools if provided
                if tools:
                    request_params["tools"] = tools
                    logger.info(f"[BEDROCK] Sending request with {len(tools)} tools: {[t['name'] for t in tools]}")
                else:
                    logger.info(f"[BEDROCK] Sending request without tools")

                logger.info(f"[BEDROCK] Model: {self.model_id}, Messages: {len(msg_list)}, Max tokens: {request_params['max_tokens']}")
                logger.debug(f"[BEDROCK] Full request body: {json.dumps({k: v for k, v in request_params.items() if k != 'messages'}, indent=2)}")
                logger.debug(f"[BEDROCK] Messages: {json.dumps(msg_list, indent=2)}")

                # Invoke with streaming response
                logger.info(f"[BEDROCK] Invoking model {self.model_id} with streaming response")

                stream = self.client.messages.create(**request_params)

                logger.info(f"[BEDROCK] Stream created successfully, starting to process events")
                event_count = 0
                for event in stream:
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

            elif self.model_family == "amazon":
                logger.info(f"[BEDROCK-AMAZON] Using Amazon model family with model: {self.model_id}")

                # Prepare messages for sending to Bedrock
                if messages:
                    messages = self._adapt_messages_for_amazon_models(cast(list[MessageParam], messages))
                    self._dedupe_documents(messages)
                    self._limit_documents(messages, max_documents=5)

                    logger.debug(f"[BEDROCK-AMAZON] Adapted messages: {json.dumps(messages, indent=2)}")
                    tools = self._adapt_tools_for_amazon_models(tools) if tools else None
                else:
                    messages = [{"role": "user", "content": [{"text": prompt}]}]

                request_params = {
                    "modelId": self.model_id,
                    "messages": messages,
                    "inferenceConfig": {
                        "maxTokens": max_tokens or 4096,
                        "temperature": temperature or 0.7,
                        "topP": top_p or 0.9,
                    }
                }

                if tools:
                    request_params["toolConfig"] = {
                        "tools": tools
                    }

                response = self.client.converse_stream(**request_params)

                logger.info(f"[BEDROCK-AMAZON] Stream created, processing chunks")
                chunk_count = 0
                for chunk in response["stream"]:
                    chunk_count += 1
                    logger.debug(f"[BEDROCK-AMAZON] Chunk {chunk_count}: {list(chunk.keys())}")
                    event = self._convert_response_to_anthropic_events(chunk)
                    if event:
                        yield event
                    else:
                        logger.debug(f"[BEDROCK-AMAZON] Skipping unknown chunk type: {list(chunk.keys())}")
                logger.info(f"[BEDROCK-AMAZON] Stream completed after {chunk_count} chunks")
            else:
                raise ValueError(f"Unsupported model family: {self.model_family}")

        except ClientError as e:
            error_code = e.response.get('Error', {}).get('Code', 'Unknown')
            logger.error(f"[BEDROCK] AWS Bedrock client error ({error_code}): {str(e)}", exc_info=True)
        except Exception as e:
            logger.error(f"[BEDROCK] Failed to stream from AWS Bedrock: {str(e)}", exc_info=True)

    async def generate_response(
        self,
        prompt: str,
        max_tokens: Optional[int] = None,
        temperature: Optional[float] = None,
        top_p: Optional[float] = None,
    ) -> str:
        """Generate non-streaming response from AWS Bedrock Claude models."""
        try:
            logger.info(f"[BEDROCK] Generating non-streaming response using model {self.model_id}")
            if self.model_family == "anthropic":
                # Prepare the request body for Claude models
                conversation = [
                    { "role": "user", "content": [{ "type": "text", "text": prompt }] }
                ]

                request_params = {
                    "model": self.secondary_model_id,
                    "messages": conversation,
                    "max_tokens": max_tokens or 4096,
                    "temperature": temperature or 0.7,
                }

                # Invoke the model
                message = self.client.messages.create(**request_params)
                response_text = message.content[0].text

                if not response_text:
                    raise Exception("Empty response from AWS Bedrock model")

                return response_text
            
            elif self.model_family == "amazon":
                conversation = [
                    { 
                        "role": "user",
                        "content": [{ "text": prompt }]
                    }
                ]

                response = self.client.converse(
                    modelId=self.model_id,
                    messages=conversation,
                    inferenceConfig={
                        "maxTokens": max_tokens or 512,
                        "temperature": temperature or 0.7,
                        "topP": top_p or 0.9,
                    }
                )
                logger.debug(f"generate_response: response from LLM -> {response}")

                response_text = response["output"]["message"]["content"][0]["text"]
                if not response_text:
                    raise Exception("Empty response from AWS Bedrock model")

                return response_text

        except ClientError as e:
            logger.error(f"AWS Bedrock client error: {str(e)}")
            raise Exception(f"AWS Bedrock service error: {e.response['Error']['Code']}")
        except Exception as e:
            logger.error(f"Failed to generate response from AWS Bedrock: {str(e)}")
            raise Exception(f"Failed to generate response: {str(e)}")

    async def health_check(self) -> bool:
        """Check if AWS Bedrock service is accessible."""
        try:
            # Try a minimal request to check service accessibility
            body = {
                "anthropic_version": "bedrock-2023-05-31",
                "max_tokens": 1,
                "messages": [{"role": "user", "content": "Hello"}],
            }

            response = self.client.invoke_model(
                modelId=self.model_id,
                body=json.dumps(body),
                contentType="application/json",
                accept="application/json",
            )
            return True
        except Exception:
            return False