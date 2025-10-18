"""
Embedding Provider abstraction layer for supporting multiple embedding providers.
"""

from abc import ABC, abstractmethod
from typing import List, Optional, Tuple
from dataclasses import dataclass
import re


@dataclass
class Chunk:
    """Represents a text chunk with its embedding and position in the original text."""
    span: tuple[int, int]  # (start_char, end_char) in original text
    embedding: List[float]


class EmbeddingProvider(ABC):
    """Abstract base class for embedding providers."""

    @abstractmethod
    def generate_embeddings_sync(
        self,
        texts: List[str],
        task: str,
        chunk_size: int,
        chunking_mode: str,
        n_sentences: Optional[int] = None,
    ) -> List[List[Chunk]]:
        """
        Generate embeddings for input texts with configurable chunking.

        Args:
            texts: List of input texts to embed
            task: Task type (e.g., 'retrieval.query', 'retrieval.passage')
            chunk_size: Number of tokens per chunk (for fixed/sentence modes)
            chunking_mode: One of 'none', 'fixed', 'sentence', 'semantic'
            n_sentences: Number of sentences per chunk (sentence mode only)

        Returns:
            List of chunk lists, one per input text. Each chunk contains:
            - span: (start_char, end_char) position in original text
            - embedding: The embedding vector as a list of floats
        """
        pass

    @abstractmethod
    def get_model_name(self) -> str:
        """Get the name/identifier of the embedding model being used."""
        pass


# Common chunking utilities
def chunk_by_sentences(
    text: str,
    chunk_size: int = 512,
    overlap: int = 50
) -> List[Tuple[int, int]]:
    """
    Simple sentence-based chunking for text.
    Returns list of character spans for each chunk.
    """
    # Split text into sentences
    sentence_pattern = r'[.!?]+[\s]+'
    sentences = re.split(sentence_pattern, text)

    chunks = []
    current_chunk_start = 0
    current_chunk_text = ""

    for i, sentence in enumerate(sentences):
        # Skip empty sentences
        if not sentence.strip():
            continue

        # Check if adding this sentence exceeds chunk size
        if len(current_chunk_text) + len(sentence) > chunk_size and current_chunk_text:
            # Save current chunk
            chunk_end = current_chunk_start + len(current_chunk_text)
            chunks.append((current_chunk_start, chunk_end))

            # Start new chunk with overlap
            overlap_start = max(0, chunk_end - overlap)
            current_chunk_start = overlap_start
            current_chunk_text = text[overlap_start:chunk_end][-overlap:] if overlap > 0 else ""

        current_chunk_text += sentence
        if i < len(sentences) - 1:
            current_chunk_text += ". "  # Add sentence separator

    # Add the last chunk
    if current_chunk_text.strip():
        chunk_end = min(current_chunk_start + len(current_chunk_text), len(text))
        chunks.append((current_chunk_start, chunk_end))

    # If no chunks were created, return the entire text as one chunk
    if not chunks:
        chunks = [(0, len(text))]

    return chunks


def generate_sentence_chunks(
    text: str,
    k_sentences: int = 5
) -> List[Tuple[int, int]]:
    """
    Generate overlapping chunks of K consecutive sentences.
    Returns list of character spans for each chunk.
    """
    # Split text into sentences with their positions
    sentence_pattern = r'[.!?]+[\s]+'
    sentences = []
    last_end = 0

    for match in re.finditer(sentence_pattern, text):
        sentence_end = match.end()
        sentence_text = text[last_end:sentence_end].strip()
        if sentence_text:
            sentences.append((last_end, sentence_end))
        last_end = sentence_end

    # Add the last sentence if there's remaining text
    if last_end < len(text):
        remaining = text[last_end:].strip()
        if remaining:
            sentences.append((last_end, len(text)))

    # Generate chunks of k consecutive sentences
    chunks = []
    for i in range(0, len(sentences), k_sentences):
        chunk_sentences = sentences[i:i + k_sentences]
        if chunk_sentences:
            chunk_start = chunk_sentences[0][0]
            chunk_end = chunk_sentences[-1][1]
            chunks.append((chunk_start, chunk_end))

    # If no chunks were created, return the entire text as one chunk
    if not chunks:
        chunks = [(0, len(text))]

    return chunks


# Import all providers after base class definition
from .jina import JinaEmbeddingProvider
from .bedrock import BedrockEmbeddingProvider

# Constants for task types
QUERY_TASK = "retrieval.query"
PASSAGE_TASK = "retrieval.passage"
DEFAULT_TASK = PASSAGE_TASK

# Factory function to create embedding providers
def create_embedding_provider(provider_type: str, **kwargs) -> EmbeddingProvider:
    """
    Factory function to create embedding provider based on type.

    Args:
        provider_type: Type of provider ('jina', 'bedrock')
        **kwargs: Provider-specific configuration
            For 'jina':
                - api_key: JINA API key
                - model: JINA model name
                - api_url: JINA API URL
            For 'bedrock':
                - model_id: AWS Bedrock model ID (e.g., 'amazon.titan-embed-text-v2:0')
                - region_name: AWS region
    """
    if provider_type.lower() == "jina":
        api_key = kwargs.get("api_key")
        if not api_key:
            raise ValueError("api_key is required for JINA provider")
        model = kwargs.get("model", "jina-embeddings-v3")
        api_url = kwargs.get("api_url", "https://api.jina.ai/v1/embeddings")
        return JinaEmbeddingProvider(api_key, model, api_url)

    elif provider_type.lower() == "bedrock":
        model_id = kwargs.get("model_id")
        if not model_id:
            raise ValueError("model_id is required for Bedrock provider")
        region_name = kwargs.get("region_name")
        if not region_name:
            raise ValueError("region_name is required for Bedrock provider")
        return BedrockEmbeddingProvider(model_id, region_name)

    else:
        raise ValueError(f"Unknown embedding provider type: {provider_type}")


__all__ = [
    "EmbeddingProvider",
    "Chunk",
    "JinaEmbeddingProvider",
    "BedrockEmbeddingProvider",
    "create_embedding_provider",
    "QUERY_TASK",
    "PASSAGE_TASK",
    "DEFAULT_TASK",
    "chunk_by_sentences",
    "generate_sentence_chunks",
]
