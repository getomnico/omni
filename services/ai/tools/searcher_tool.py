from .base import BaseToolHandler
from .searcher_client import SearcherClient, SearchRequest, SearchResponse


class SearcherTool(BaseToolHandler):
    """Invoke omni-searcher to retrieve relevant documents."""

    def __init__(self):
        super().__init__()
        self.client = SearcherClient()

    async def handle(self, request: SearchRequest) -> SearchResponse:
        """Handle the tool call and return a response."""
        return await self.client.search(request)
