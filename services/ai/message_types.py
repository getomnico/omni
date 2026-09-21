"""Shared message types used at the chat API and streaming boundaries."""

from __future__ import annotations

from typing import Annotated, Literal, NotRequired

from anthropic.types import TextBlockParam
from pydantic import Field
from typing_extensions import TypedDict


class OmniUploadSource(TypedDict):
    type: Literal["omni_upload"]
    upload_id: Annotated[str, Field(min_length=1, max_length=26)]


class OmniUploadBlock(TypedDict):
    type: Literal["document", "image"]
    source: OmniUploadSource


class OmniMentionSource(TypedDict):
    type: Literal["omni_mention"]
    document_id: Annotated[str, Field(min_length=1, max_length=26)]
    title: Annotated[str, Field(min_length=1, max_length=500)]
    source_type: NotRequired[Annotated[str, Field(max_length=100)] | None]
    content_type: NotRequired[Annotated[str, Field(max_length=255)] | None]


class OmniMentionBlock(TypedDict):
    type: Literal["document"]
    source: OmniMentionSource


class UserMessageParam(TypedDict):
    role: Literal["user"]
    content: str | list[TextBlockParam | OmniUploadBlock | OmniMentionBlock]


# ID of a row in the `uploads` table (ULID). Aliased for self-documenting dict keys.
UploadId = str

# ID of a document in the `documents` table (ULID). Aliased for self-documenting dict keys.
DocumentId = str
