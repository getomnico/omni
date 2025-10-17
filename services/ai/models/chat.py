from pydantic import BaseModel
from typing import List, Optional

class SearchToolParams(BaseModel):
    query: str
    sources: Optional[List[str]] = None
    content_types: Optional[List[str]] = None
    limit: Optional[int] = 20

class ReadDocumentParams(BaseModel):
    id: str # Document ID
    name: str # Document Name
    query: Optional[str] = None # Optional query to retrieve specific relevant sections

