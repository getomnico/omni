"""Data models for paperless-ngx API responses."""

from dataclasses import dataclass, field
from datetime import datetime


@dataclass
class PaperlessCustomField:
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
    custom_fields: list[PaperlessCustomField] = field(default_factory=list)
    correspondent_name: str | None = None
    document_type_name: str | None = None
    tag_names: list[str] = field(default_factory=list)
