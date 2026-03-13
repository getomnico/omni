from pydantic import BaseModel


class SearchToolParams(BaseModel):
    query: str
    document_id: str | None = None
    limit: int | None = 20
