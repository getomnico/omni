"""API request/response schemas for the Omni AI service."""

import asyncio
import time
from dataclasses import dataclass, field
from enum import IntEnum
from typing import Annotated, Literal

from pydantic import BaseModel, Field


class Priority(IntEnum):
    """Priority levels for embedding requests."""

    HIGH = 1  # Searcher requests
    NORMAL = 2  # Default
    LOW = 3  # Indexer bulk requests


@dataclass(order=True)
class PrioritizedRequest:
    """A prioritized embedding request for the queue."""

    priority: int
    request_id: str = field(compare=False)
    request: "EmbeddingRequest" = field(compare=False)
    future: asyncio.Future = field(compare=False)
    timestamp: float = field(default_factory=time.time, compare=False)


class EmbeddingRequest(BaseModel):
    """Request to generate embeddings for texts."""

    texts: list[str]
    task: str | None = "passage"
    chunk_size: int | None = 512  # Chunk size in tokens
    chunking_mode: str | None = "sentence"  # "sentence", "fixed", or "none"
    priority: Literal["high", "normal", "low"] | None = "normal"


class EmbeddingResponse(BaseModel):
    """Response containing generated embeddings."""

    embeddings: list[list[list[float]]]
    chunks_count: list[int]  # Number of chunks per text
    chunks: list[list[tuple[int, int]]]  # Character offset spans for each chunk
    model_name: str


class PromptRequest(BaseModel):
    """Request to generate a response from the LLM."""

    prompt: str
    max_tokens: int | None = 512
    stream: bool | None = True


class PromptResponse(BaseModel):
    """Response from the LLM."""

    response: str


class SteeringTextBlock(BaseModel):
    type: Literal["text"]
    text: str


class SteeringUploadSource(BaseModel):
    type: Literal["omni_upload"]
    upload_id: str = Field(min_length=1, max_length=26)


class SteeringMentionSource(BaseModel):
    type: Literal["omni_mention"]
    document_id: str = Field(min_length=1, max_length=26)
    title: str = Field(min_length=1, max_length=500)
    source_type: str | None = Field(default=None, max_length=100)
    content_type: str | None = Field(default=None, max_length=255)


SteeringDocumentSource = Annotated[
    SteeringUploadSource | SteeringMentionSource,
    Field(discriminator="type"),
]


class SteeringDocumentBlock(BaseModel):
    type: Literal["document"]
    source: SteeringDocumentSource


class SteeringUserMessage(BaseModel):
    role: Literal["user"]
    content: str | list[SteeringTextBlock | SteeringDocumentBlock]


class SteeringMessageRequest(BaseModel):
    message_id: str = Field(
        pattern=r"^[0123456789ABCDEFGHJKMNPQRSTVWXYZ]{26}$",
        min_length=26,
        max_length=26,
    )
    message: SteeringUserMessage
