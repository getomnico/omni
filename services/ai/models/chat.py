from pydantic import BaseModel


class SearchToolParams(BaseModel):
    query: str
    sources: list[str] | None = None
    content_types: list[str] | None = None
    limit: int | None = 20


class ReadDocumentParams(BaseModel):
    id: str  # Document ID
    name: str  # Document Name
    query: str | None = None  # Optional query to retrieve specific relevant sections
    start_line: int | None = None
    end_line: int | None = None
