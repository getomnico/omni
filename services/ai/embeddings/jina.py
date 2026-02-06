import logging
import time
import httpx
import asyncio

from transformers import AutoTokenizer

from . import EmbeddingProvider, Chunk
from processing import Chunker

logger = logging.getLogger(__name__)


class JinaEmbeddingProvider(EmbeddingProvider):
    """Provider for JINA AI Embeddings API."""

    # Task mappings for JINA API
    QUERY_TASK = "retrieval.query"
    PASSAGE_TASK = "retrieval.passage"
    DEFAULT_TASK = PASSAGE_TASK

    # JINA API Configuration
    JINA_ORG = "jinaai"
    JINA_MAX_BATCH_SIZE = 2048
    JINA_MAX_RETRIES = 3
    JINA_RETRY_DELAY = 1.0

    def __init__(self, api_key: str, model: str, api_url: str, max_model_len: int):
        self.api_key = api_key
        self.model = model
        self.api_url = api_url
        self.max_model_len = max_model_len

        if not self.api_key:
            raise ValueError("api_key is required for JINA provider")

        self.client = JINAEmbeddingClient(self.api_key, self.model, self.api_url)

        hf_model_id = (
            model
            if model.startswith(f"{self.JINA_ORG}/")
            else f"{self.JINA_ORG}/{model}"
        )
        self.tokenizer = AutoTokenizer.from_pretrained(
            hf_model_id, trust_remote_code=True
        )
        self.chunker = Chunker("sentence")

        logger.info(
            f"Initialized JINA embedding provider - model: {model}, max_model_len: {max_model_len}"
        )

    def get_model_name(self) -> str:
        """Get the name of the JINA model being used."""
        return self.model

    async def generate_embeddings(
        self,
        text: str,
        task: str,
        chunk_size: int,
        chunking_mode: str,
    ) -> list[Chunk]:
        """Generate embeddings using JINA API with chunking support."""

        start_time = time.time()

        # Cap chunk_size at max_model_len
        effective_chunk_size = min(chunk_size, self.max_model_len)

        try:
            if chunking_mode == "none":
                embeddings = await self.client.generate_embeddings([text], task)
                chunks = [Chunk((0, len(text)), embeddings[0])]

            elif chunking_mode == "sentence":
                _, char_spans = await self.chunker.chunk_by_sentences_async(
                    text, effective_chunk_size, self.tokenizer
                )

                chunk_texts = [text[start:end] for start, end in char_spans]

                if chunk_texts:
                    embeddings = await self.client.generate_embeddings(
                        chunk_texts, task
                    )
                    chunks = [
                        Chunk(span, embedding)
                        for span, embedding in zip(char_spans, embeddings)
                    ]
                else:
                    embeddings = await self.client.generate_embeddings([text], task)
                    chunks = [Chunk((0, len(text)), embeddings[0])]

            elif chunking_mode == "fixed":
                _, char_spans = await self.chunker.chunk_by_tokens_async(
                    text, effective_chunk_size, self.tokenizer
                )

                chunk_texts = [text[start:end] for start, end in char_spans]

                embeddings = await self.client.generate_embeddings(chunk_texts, task)
                chunks = [
                    Chunk(span, embedding)
                    for span, embedding in zip(char_spans, embeddings)
                ]

            else:
                logger.warning(
                    f"Unsupported chunking mode: {chunking_mode}, using no chunking"
                )
                embeddings = await self.client.generate_embeddings([text], task)
                chunks = [Chunk((0, len(text)), embeddings[0])]

            end_time = time.time()
            total_time = end_time - start_time
            logger.info(
                f"JINA embedding generation complete - total_time: {total_time:.2f}s, "
                f"total_chunks: {len(chunks)}"
            )

            return chunks

        except Exception as e:
            logger.error(f"Error generating embeddings with JINA: {str(e)}")
            raise Exception(f"JINA embedding generation failed: {str(e)}")


class JINAEmbeddingClient:
    """Client for JINA AI Embedding API"""

    def __init__(self, api_key: str, model: str, api_url: str):
        self.api_url = api_url
        self.api_key = api_key
        self.model = model

        if not self.api_key:
            raise ValueError("JINA API key is required")

        # Create async HTTP client with timeout settings
        self.client = httpx.AsyncClient(
            timeout=httpx.Timeout(30.0, connect=5.0),
            limits=httpx.Limits(max_keepalive_connections=5, max_connections=10),
        )

    async def close(self):
        """Close the HTTP client"""
        await self.client.aclose()

    async def _make_request(
        self, texts: list[str], task: str, dimensions: int | None = None
    ) -> dict:
        """Make a request to JINA API with retry logic"""
        headers = {
            "Content-Type": "application/json",
            "Authorization": f"Bearer {self.api_key}",
        }

        # Prepare request payload
        payload = {
            "model": self.model,
            "task": task,
            "input": texts,
            "late_chunking": True,
        }

        # Add dimensions if specified (for Matryoshka representation)
        if dimensions:
            payload["dimensions"] = dimensions

        # Retry logic with exponential backoff
        for attempt in range(JINA_MAX_RETRIES):
            try:
                response = await self.client.post(
                    self.api_url, headers=headers, json=payload
                )

                if response.status_code == 200:
                    return response.json()
                elif response.status_code == 429:  # Rate limit
                    retry_after = float(
                        response.headers.get(
                            "Retry-After", JINA_RETRY_DELAY * (2**attempt)
                        )
                    )
                    logger.warning(
                        f"Rate limited, retrying after {retry_after} seconds"
                    )
                    await asyncio.sleep(retry_after)
                else:
                    error_msg = (
                        f"JINA API error: {response.status_code} - {response.text}"
                    )
                    if attempt < JINA_MAX_RETRIES - 1:
                        logger.warning(f"{error_msg}, retrying...")
                        await asyncio.sleep(JINA_RETRY_DELAY * (2**attempt))
                    else:
                        raise Exception(error_msg)

            except httpx.RequestError as e:
                if attempt < JINA_MAX_RETRIES - 1:
                    logger.warning(f"Request error: {e}, retrying...")
                    await asyncio.sleep(JINA_RETRY_DELAY * (2**attempt))
                else:
                    raise Exception(f"Failed to connect to JINA API: {e}")

        raise Exception(f"Failed after {JINA_MAX_RETRIES} retries")

    async def generate_embeddings(
        self,
        texts: list[str],
        task: str = JinaEmbeddingProvider.DEFAULT_TASK,
        dimensions: int | None = None,
    ) -> list[list[float]]:
        """Generate embeddings for a list of texts"""

        # Handle empty input
        if not texts:
            return []

        # Process in batches if necessary
        all_embeddings = []

        for i in range(0, len(texts), JINA_MAX_BATCH_SIZE):
            batch = texts[i : i + JINA_MAX_BATCH_SIZE]

            logger.info(
                f"Generating embeddings for batch {i//JINA_MAX_BATCH_SIZE + 1} ({len(batch)} texts)"
            )

            response = await self._make_request(batch, task, dimensions)

            # Extract embeddings from response
            embeddings = [item["embedding"] for item in response["data"]]
            all_embeddings.extend(embeddings)

        return all_embeddings
