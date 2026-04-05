"""Data models for paperless-ngx API responses."""

from dataclasses import dataclass, field
from datetime import datetime


@dataclass
class PaperlessTag:
    id: int
    name: str
    colour: int = 0


@dataclass
class PaperlessCorrespondent:
    id: int
    name: str


@dataclass
class PaperlessDocumentType:
    id: int
    name: str


@dataclass
class PaperlessCustomField:
    id: int
    name: str
    value: str | None


@dataclass
class PaperlessDocument:
    id: int
    title: str
    content: str
    created: datetime | None
    added: datetime | None
    modified: datetime | None
    original_file_name: str | None
    archived_file_name: str | None
    correspondent_id: int | None
    document_type_id: int | None
    tag_ids: list[int] = field(default_factory=list)
    custom_fields: list[PaperlessCustomField] = field(default_factory=list)

    # Resolved names (populated by the client after lookup)
    correspondent_name: str | None = None
    document_type_name: str | None = None
    tag_names: list[str] = field(default_factory=list)
