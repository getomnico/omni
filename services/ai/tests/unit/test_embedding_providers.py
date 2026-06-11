from __future__ import annotations

from unittest.mock import AsyncMock

import pytest

from embeddings import create_embedding_provider
from embeddings.bedrock import BedrockEmbeddingProvider
from embeddings.cohere import CohereEmbeddingProvider
from embeddings.jina import JinaEmbeddingProvider
from embeddings.openai import OpenAIEmbeddingProvider

pytestmark = pytest.mark.unit


@pytest.mark.asyncio
async def test_openai_embedding_provider_uses_custom_api_url():
    provider = create_embedding_provider(
        "openai",
        api_key="sk-test",
        model="text-embedding-3-small",
        api_url="https://embeddings.example.test/v1/",
    )

    try:
        assert isinstance(provider, OpenAIEmbeddingProvider)
        assert provider.base_url == "https://embeddings.example.test/v1"
        assert provider.client.embeddings_url == "https://embeddings.example.test/v1/embeddings"
    finally:
        await provider.client.close()


class _SyncEmbeddingClient:
    def generate_embeddings(self, texts: list[str]) -> list[list[float]]:
        return [[0.1, 0.2, 0.3] for _ in texts]


@pytest.mark.asyncio
@pytest.mark.parametrize(
    ("provider_cls", "provider_fields"),
    [
        (OpenAIEmbeddingProvider, {"model": "openai-test", "max_model_len": None}),
        (JinaEmbeddingProvider, {"model": "jina-test", "max_model_len": 8192}),
    ],
)
async def test_async_embedding_providers_allow_null_chunk_size_when_chunking_disabled(
    provider_cls, provider_fields
):
    provider = provider_cls.__new__(provider_cls)
    for field, value in provider_fields.items():
        setattr(provider, field, value)
    provider.client = AsyncMock()
    provider.client.generate_embeddings.return_value = [[0.1, 0.2, 0.3]]

    chunks = await provider.generate_embeddings("hello", "query", None, "none")

    assert len(chunks) == 1
    assert chunks[0].span == (0, 5)
    assert chunks[0].embedding == [0.1, 0.2, 0.3]


@pytest.mark.asyncio
async def test_cohere_allows_null_chunk_size_when_chunking_disabled():
    provider = CohereEmbeddingProvider.__new__(CohereEmbeddingProvider)
    provider.model = "cohere-test"
    provider.max_model_len = 8192
    provider.dimensions = None
    provider._embed_texts = AsyncMock(return_value=[[0.1, 0.2, 0.3]])

    chunks = await provider.generate_embeddings("hello", "query", None, "none")

    assert len(chunks) == 1
    assert chunks[0].span == (0, 5)
    assert chunks[0].embedding == [0.1, 0.2, 0.3]


@pytest.mark.asyncio
async def test_bedrock_allows_null_chunk_size_when_chunking_disabled():
    provider = BedrockEmbeddingProvider.__new__(BedrockEmbeddingProvider)
    provider.model_id = "bedrock-test"
    provider.max_model_len = 8192
    provider.client = _SyncEmbeddingClient()

    chunks = await provider.generate_embeddings("hello", "query", None, "none")

    assert len(chunks) == 1
    assert chunks[0].span == (0, 5)
    assert chunks[0].embedding == [0.1, 0.2, 0.3]
