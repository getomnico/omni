from dataclasses import dataclass
from pydantic import BaseModel

class SearchToolParams(BaseModel):
    query: str
    document_id: str | None = None
    limit: int | None = 20

@dataclass
class MentionedDocumentContext:
    doc_id: str
    title: str
    content: str
