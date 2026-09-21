"""API request/response schemas for the Omni AI service."""

import asyncio
import time
from dataclasses import dataclass, field
from enum import IntEnum
from typing import Literal

from pydantic import BaseModel, Field

from message_types import UserMessageParam


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


ULID_PATTERN = r"^[0123456789ABCDEFGHJKMNPQRSTVWXYZ]{26}$"


class SteeringMessageRequest(BaseModel):
    message_id: str = Field(
        pattern=ULID_PATTERN,
        min_length=26,
        max_length=26,
    )
    message: UserMessageParam
