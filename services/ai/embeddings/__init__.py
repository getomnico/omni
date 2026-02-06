"""
Embedding Provider abstraction layer for supporting multiple embedding providers.
"""

from abc import ABC, abstractmethod
from dataclasses import dataclass


@dataclass
class Chunk:
    """Represents a text chunk with its embedding and position in the original text."""

    span: tuple[int, int]  # (start_char, end_char) in original text
    embedding: list[float]


class EmbeddingProvider(ABC):
    """Abstract base class for embedding providers."""

    @abstractmethod
    async def generate_embeddings(
        self,
        text: str,
        task: str,
        chunk_size: int,
        chunking_mode: str,
    ) -> list[Chunk]:
        """
        Generate embeddings for a single input text with configurable chunking.

        Args:
            text: Input text to embed
            task: Task type ('query' or 'passage')
            chunk_size: Number of tokens per chunk
            chunking_mode: One of 'none', 'fixed', 'sentence'

        Returns:
            List of chunks. Each chunk contains:
            - span: (start_char, end_char) position in original text
            - embedding: The embedding vector as a list of floats
        """
        pass

    @abstractmethod
    def get_model_name(self) -> str:
        """Get the name/identifier of the embedding model being used."""
        pass


from .jina import JinaEmbeddingProvider
from .bedrock import BedrockEmbeddingProvider
from .openai import OpenAIEmbeddingProvider
from .cohere import CohereEmbeddingProvider


# Factory function to create embedding providers
def create_embedding_provider(provider_type: str, **kwargs) -> EmbeddingProvider:
    """
    Factory function to create embedding provider based on type.

    Args:
        provider_type: Type of provider ('jina', 'bedrock', 'openai', 'local', 'cohere')
        **kwargs: Provider-specific configuration
            For 'jina':
                - api_key: JINA API key
                - model: JINA model name
                - api_url: JINA API URL
            For 'bedrock':
                - model_id: AWS Bedrock model ID (e.g., 'amazon.titan-embed-text-v2:0')
                - region_name: AWS region
            For 'openai':
                - api_key: OpenAI API key
                - model: OpenAI model name (e.g., 'text-embedding-3-small')
                - dimensions: Optional embedding dimensions
            For 'local':
                - base_url: Local embedding server URL (e.g., 'http://embeddings:8001/v1')
                - model: Model name (e.g., 'nomic-ai/nomic-embed-text-v1.5')
                - max_model_len: Maximum model context length in tokens
            For 'cohere':
                - api_key: Cohere API key
                - model: Cohere model name (e.g., 'embed-v4.0')
                - api_url: Cohere API URL
                - max_model_len: Maximum model context length in tokens
                - dimensions: Optional output dimensions
    """
    if provider_type.lower() == "jina":
        api_key = kwargs.get("api_key")
        if not api_key:
            raise ValueError("api_key is required for JINA provider")
        model = kwargs.get("model", "jina-embeddings-v3")
        api_url = kwargs.get("api_url", "https://api.jina.ai/v1/embeddings")
        max_model_len = kwargs.get("max_model_len")
        if not max_model_len:
            raise ValueError("max_model_len is required for JINA provider")
        return JinaEmbeddingProvider(api_key, model, api_url, max_model_len)

    elif provider_type.lower() == "bedrock":
        model_id = kwargs.get("model_id")
        if not model_id:
            raise ValueError("model_id is required for Bedrock provider")
        region_name = kwargs.get("region_name")
        if not region_name:
            raise ValueError("region_name is required for Bedrock provider")
        max_model_len = kwargs.get("max_model_len")
        if not max_model_len:
            raise ValueError("max_model_len is required for Bedrock provider")
        return BedrockEmbeddingProvider(model_id, region_name, max_model_len)

    elif provider_type.lower() == "openai":
        api_key = kwargs.get("api_key")
        if not api_key:
            raise ValueError("api_key is required for OpenAI provider")
        model = kwargs.get("model", "text-embedding-3-small")
        dimensions = kwargs.get("dimensions", 1024)
        return OpenAIEmbeddingProvider(
            api_key=api_key,
            model=model,
            base_url="https://api.openai.com/v1",
            dimensions=dimensions,
        )

    elif provider_type.lower() == "cohere":
        api_key = kwargs.get("api_key")
        if not api_key:
            raise ValueError("api_key is required for Cohere provider")
        max_model_len = kwargs.get("max_model_len")
        if not max_model_len:
            raise ValueError("max_model_len is required for Cohere provider")
        model = kwargs.get("model", "embed-v4.0")
        api_url = kwargs.get("api_url", "https://api.cohere.com/v2/embed")
        dimensions = kwargs.get("dimensions")
        return CohereEmbeddingProvider(
            api_key=api_key,
            model=model,
            api_url=api_url,
            max_model_len=max_model_len,
            dimensions=dimensions,
        )

    elif provider_type.lower() == "local":
        base_url = kwargs.get("base_url")
        model = kwargs.get("model")
        max_model_len = kwargs.get("max_model_len")
        if not base_url or not model:
            raise ValueError("base_url and model are required for local provider")
        if not max_model_len:
            raise ValueError("max_model_len is required for local provider")
        return OpenAIEmbeddingProvider(
            api_key="not-needed",
            model=model,
            base_url=base_url,
            dimensions=None,
            max_model_len=max_model_len,
        )

    else:
        raise ValueError(f"Unknown embedding provider type: {provider_type}")


__all__ = [
    "EmbeddingProvider",
    "Chunk",
    "JinaEmbeddingProvider",
    "BedrockEmbeddingProvider",
    "OpenAIEmbeddingProvider",
    "CohereEmbeddingProvider",
    "create_embedding_provider",
]
